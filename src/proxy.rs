use std::{ops::{DerefMut, Deref}, time::Duration};
use hyper::{Request, Client, client::HttpConnector, Body, Response, StatusCode, http::HeaderValue, HeaderMap, header::HeaderName};
use hyper_tls::HttpsConnector;
use rand::{thread_rng, distributions::Alphanumeric, Rng};
use redis::RedisError;
use thiserror::Error;
use tokio::time::{Instant, sleep};

use crate::{redis::client::RedisClient, global_rl::fetch_discord_global_ratelimit, buckets::{Resources, get_route_info, RouteInfo}};

#[derive(Clone)]
pub struct ProxyWrapper {
  pub proxy: DiscordProxy
}

impl ProxyWrapper {
  pub fn new(config: DiscordProxyConfig, redis: &RedisClient, client: &Client<HttpsConnector<HttpConnector>>) -> Self {
    Self { 
      proxy: DiscordProxy { config: config.clone(), redis: redis.clone(), client: client.clone() }
    }
  }
}

impl Deref for ProxyWrapper {
  type Target = DiscordProxy;

  fn deref(&self) -> &Self::Target {
    &self.proxy
  }
}

impl DerefMut for ProxyWrapper {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.proxy
  }
}

#[derive(Clone, PartialEq)]
pub enum NewBucketStrategy {
  Strict,
  Loose
}

#[derive(Clone)]
pub struct DiscordProxyConfig {
  global: NewBucketStrategy,
  buckets: NewBucketStrategy,
  lock_timeout: Duration
}

impl DiscordProxyConfig {
  pub fn new(global: NewBucketStrategy, buckets: NewBucketStrategy, lock_timeout: Duration) -> Self {
    Self { global, buckets, lock_timeout }
  }
}

#[derive(Clone)]
pub struct DiscordProxy {
  redis: RedisClient,
  client: Client<HttpsConnector<HttpConnector>>,
  config: DiscordProxyConfig
}

#[derive(PartialEq)]
pub enum RatelimitStatus {
  Ok,
  NewBucket(String),
  GlobalRatelimited,
  BucketRatelimited,
}

impl DiscordProxy {
  pub async fn proxy_request(&mut self, bot_id: &u64, token: &str, req: Request<Body>) -> Result<Response<Body>, ProxyError> {
    let _start = Instant::now();
    
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    let route_info = get_route_info(bot_id, &method, &path);
    
    let ratelimit_status = self.check_ratelimits(&bot_id, &token, &route_info).await?;
    match ratelimit_status {
      RatelimitStatus::GlobalRatelimited => {
        let mut res = Response::new(Body::from("You are being ratelimited."));
        *res.status_mut() = hyper::StatusCode::TOO_MANY_REQUESTS;

        res.headers_mut().insert("x-sent-by-proxy", HeaderValue::from_static("true"));
        res.headers_mut().insert("x-ratelimit-bucket", HeaderValue::from_str(&bot_id.to_string()).unwrap());

        return Ok(res);
      },
      RatelimitStatus::BucketRatelimited => {
        let mut res = Response::new(Body::from("You are being ratelimited."));
        *res.status_mut() = hyper::StatusCode::TOO_MANY_REQUESTS;

        res.headers_mut().insert("x-sent-by-proxy", HeaderValue::from_static("true"));
        res.headers_mut().insert("x-ratelimit-bucket", HeaderValue::from_str(&route_info.bucket).unwrap_or("N/A".parse().unwrap()));

        return Ok(res);
      },
      _ => {
        // println!("Ratelimits passed! Proxying request..");
      }
    }

    // println!("[{}] {}ms - {} {}", bot_id, _start.elapsed().as_millis(), method, path);

    let uri = format!("https://discord.com{}", path);

    let proxied_req = Request::builder()
      .header("User-Agent", "RockSolidRobots Discord Proxy/1.0")
      .uri(&uri)
      .method(&method)
      .header("Authorization", token)
      .body(req.into_body()).unwrap();

    let result = self.client.request(proxied_req).await.map_err(|e| ProxyError::RequestError(e))?;

    if let RatelimitStatus::NewBucket(lock_value) = ratelimit_status {
      let bucket_limit = match result.headers().get("X-RateLimit-Limit") {
        Some(limit) => limit.to_str().unwrap().parse::<u16>().unwrap(),
        None => 0
      };

      let reset_at = match result.headers().get("X-RateLimit-Reset") {
        Some(timestamp) => timestamp.to_str().unwrap().replace(".", "").to_string(),
        None => "0".to_string()
      };

      // println!("[{}] New bucket! Setting ratelimit to {}, resetting counter at {}", route_info.bucket, bucket_limit, reset_at);

      let mut redis = self.redis.pool.get().await.unwrap();
      match self.redis.unlock_ratelimit_script.key(&route_info.bucket).arg(&lock_value).arg(bucket_limit).arg(reset_at).invoke_async::<_, bool>(&mut redis).await? {
        true => {
          // println!("[{}] Bucket ratelimit set to {} and lock released.", &route_info.bucket, &bucket_limit);
        },
        false => {
          // println!("[{}] Bucket lock expired before we could release it.", &route_info.bucket);
        }
      }
    }

    let status = result.status();
    match status {
      StatusCode::TOO_MANY_REQUESTS => {
        println!("Discord returned 429! Global: {:?}", result.headers().get("X-RateLimit-Global"));
      },
      _ => {}
    }

    // println!("Proxied request in {}ms. Status Code: {}", _start.elapsed().as_millis(), result.status());

    Ok(result)
  }

  pub async fn check_ratelimits(&mut self, bot_id: &u64, token: &str, route: &RouteInfo) -> Result<RatelimitStatus, RedisError> {
    let use_global_rl = match route.resource {
      Resources::Interactions => false,
      Resources::Webhooks => false,
      _ => true
    };
    
    // println!("[{}] Global: {} - Bucket: {}", bot_id, use_global_rl, route.bucket);

    if use_global_rl {
      return Ok(self.is_global_or_bucket_ratelimited(&bot_id, &token, route).await?);
    } else {}

    Ok(RatelimitStatus::Ok)
  }

  async fn is_global_or_bucket_ratelimited(&mut self, bot_id: &u64, token: &str, route: &RouteInfo) -> Result<RatelimitStatus, RedisError> {
    let mut redis = self.redis.pool.get().await.unwrap();
    let global_rl_key = bot_id.to_string();
    
    let mut is_global_ratelimited = false;
    let mut is_bucket_ratelimited = false;

    let mut new_bucket_lock: Option<String> = None;

    return loop {
      let ratelimit_results = self.redis.check_global_and_bucket_ratelimit_script.key(&bot_id).key(&route.bucket).invoke_async::<_,Vec<Option<u16>>>(&mut redis).await?;

      let mut retry = false;
      for (index, ratelimit) in ratelimit_results.iter().enumerate() {
        let ratelimit_key: &str;
        let ratelimit_type: String;
        let ratelimit_config: &NewBucketStrategy;

        let is_ratelimited: &mut bool;

        match index {
          0 => {
            ratelimit_type = "Global".to_string();
            ratelimit_key = &global_rl_key;

            ratelimit_config = &self.config.global;

            is_ratelimited = &mut is_global_ratelimited;
          },
          1 => {
            ratelimit_type = "Bucket".to_string();
            ratelimit_key = &route.bucket;

            ratelimit_config = &self.config.buckets;

            is_ratelimited = &mut is_bucket_ratelimited;
          },
          _ => panic!("Too many ratelimit results.")
        };

        let _count = match ratelimit {
          Some(count) => count.to_string(),
          None => {
            "N/A".to_string()
          }
        };

        // println!("[{}] {} Ratelimit: {} - Count: {}", bot_id, ratelimit_type, ratelimit_key, _count);

        match ratelimit {
          Some(count) => {
            let hit_ratelimit = *count == 0;
            
            if hit_ratelimit {
              *is_ratelimited = true;
              break
            }
          },
          None => {
            println!("[{}] Ratelimit not set, will try to acquire a lock and set it...", &ratelimit_key);
  
            let lock_value = random_string(8);
            let lock = self.redis.lock_ratelimit_script.key(&ratelimit_key).arg(&lock_value).invoke_async::<_, bool>(&mut redis).await?;
  
            if lock {
              if ratelimit_type == "Bucket" {
                println!("[{}] Acquired bucket lock.", ratelimit_key);

                new_bucket_lock = Some(lock_value);
                break
              }

              println!("[{}] Global ratelimit lock acquired, fetching from Discord.", &ratelimit_key);
              let ratelimit = fetch_discord_global_ratelimit(token).await?;
  
              match self.redis.unlock_ratelimit_script.key(&ratelimit_key).arg(&lock_value).arg(ratelimit).arg(0).invoke_async::<_, bool>(&mut redis).await? {
                true => {
                  println!("[{}] Global ratelimit set to {} and lock released.", ratelimit_key, &ratelimit);
                },
                false => {
                  println!("[{}] Global ratelimit lock expired before we could release it.", &ratelimit_key);
                }
              }
            } else {
              if *ratelimit_config == NewBucketStrategy::Strict {
                println!("Lock is taken and ratelimit config is Strict, retrying in {}ms.", self.config.lock_timeout.as_millis());
                
                retry = true;
                break
              }

              println!("Lock is taken and ratelimit config is Loose, skipping ratelimit check.");
            }
          }
        };
      }
    
      if retry {
        sleep(self.config.lock_timeout).await;
        continue;
      }

      if let Some(lock_value) = new_bucket_lock {
        break Ok(RatelimitStatus::NewBucket(lock_value))
      }

      if is_global_ratelimited {
        break Ok(RatelimitStatus::GlobalRatelimited)
      }

      if is_bucket_ratelimited {
        break Ok(RatelimitStatus::BucketRatelimited)
      }

      break Ok(RatelimitStatus::Ok);
    };
  }
}

fn random_string(n: usize) -> String {
  thread_rng().sample_iter(&Alphanumeric)
    .take(n)
    .map(char::from)
    .collect()
}

#[derive(Error, Debug)]
pub enum ProxyError {
  #[error("Redis Error: {0}")]
  RedisError(#[from] RedisError),

  #[error("Error proxying request: {0}")]
  RequestError(#[from] hyper::Error)
}
