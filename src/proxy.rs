use std::{ops::{DerefMut, Deref}, time::{Duration, SystemTime, UNIX_EPOCH}, str::FromStr};
use base64::decode;
use hyper::{Request, Client, client::HttpConnector, Body, Response, StatusCode, http::{HeaderValue, uri::PathAndQuery}, HeaderMap, Uri};
use hyper_tls::HttpsConnector;
use prometheus::HistogramVec;
use rand::{thread_rng, distributions::Alphanumeric, Rng};
use thiserror::Error;
use tokio::time::{Instant, sleep};

use crate::{redis::{RedisClient, RedisErrorWrapper}, discord::fetch_discord_global_ratelimit, buckets::{Resources, get_route_info, RouteInfo}};

#[derive(Clone)]
pub struct ProxyWrapper {
  pub proxy: DiscordProxy
}

impl ProxyWrapper {
  pub fn new(config: DiscordProxyConfig, metrics: &Metrics, redis: &RedisClient, client: &Client<HttpsConnector<HttpConnector>>) -> Self {
    Self { 
      proxy: DiscordProxy { 
        config: config.clone(), 
        metrics: metrics.clone(), 
        redis: redis.clone(), 
        client: client.clone() 
      }
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
  // Loose
}

#[derive(Clone)]
pub struct DiscordProxyConfig {
  global: NewBucketStrategy,
  buckets: NewBucketStrategy,

  lock_timeout: Duration,

  pub enable_metrics: bool,
}

impl DiscordProxyConfig {
  pub fn new(global: NewBucketStrategy, buckets: NewBucketStrategy, lock_timeout: Duration, enable_metrics: bool) -> Self {
    Self { global, buckets, lock_timeout, enable_metrics }
  }
}

#[derive(Clone)]
pub struct Metrics {
  pub requests: HistogramVec,
}

#[derive(Clone)]
pub struct DiscordProxy {
  redis: RedisClient,
  client: Client<HttpsConnector<HttpConnector>>,

  metrics: Metrics,

  pub config: DiscordProxyConfig
}

#[derive(PartialEq)]
pub enum RatelimitStatus {
  Ok(Option<String>),
  GlobalRatelimited,
  RouteRatelimited,
}

impl DiscordProxy {
  pub async fn proxy_request(&mut self, mut req: Request<Body>) -> Result<Response<Body>, ProxyError> {
    let start = Instant::now();

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    
    let mut route_info = get_route_info(&method, &path);

    let (token, bot_id) = match parse_headers(req.headers(), &route_info) {
      Ok((token, bot_id)) => (token, bot_id),
      Err(message) => return Ok(
        Response::builder().status(400).body(message.into()).unwrap()
      )
    };
    
    route_info.bucket.push_str(&format!("{}:{}/", method.as_str(), bot_id));

    let ratelimit_status = self.check_ratelimits(&bot_id, &token, &route_info).await?;

    match ratelimit_status {
      RatelimitStatus::GlobalRatelimited => {
        return Ok(generate_ratelimit_response(&bot_id.to_string()));
      },
      RatelimitStatus::RouteRatelimited => {
        return Ok(generate_ratelimit_response(&route_info.bucket));
      },
      _ => {}
    }

    // println!("[{}] {}ms - {} {}", bot_id, start.elapsed().as_millis(), method, path);

    req.headers_mut().insert("Host", HeaderValue::from_static("discord.com"));
    req.headers_mut().insert("User-Agent", HeaderValue::from_static("RockSolidRobots Discord Proxy/1.0"));
    
    let path_and_query = match req.uri().path_and_query() {
      Some(path_and_query) => path_and_query.clone(),
      None => PathAndQuery::from_static("/")
    };
    *req.uri_mut() = Uri::from_str(&format!("https://discord.com{}", path_and_query)).unwrap();

    let result = self.client.request(req).await.map_err(|e| ProxyError::RequestError(e))?;
    let sent_request_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();

    if let RatelimitStatus::Ok(bucket_lock) = ratelimit_status {
      match self.update_ratelimits(bot_id, result.headers(), route_info.bucket, bucket_lock, sent_request_at).await {
        Ok(_) => {},
        Err(e) => {
          eprintln!("Error updating ratelimits after proxying request: {}", e);
        }
      }
    }

    let status = result.status();
    match status {
      StatusCode::OK => {}
      StatusCode::TOO_MANY_REQUESTS => {
        eprintln!("Discord returned 429! Global: {:?}", result.headers().get("X-RateLimit-Global"));
      },
      _ => {}
    }

    if self.config.enable_metrics {
      self.metrics.requests.with_label_values(
        &[&bot_id.to_string(), &method.to_string(), status.as_str(), &path]
      ).observe(start.elapsed().as_secs_f64());
    }

    // println!("Proxied request in {}ms. Status Code: {}", start.elapsed().as_millis(), result.status());

    Ok(result)
  }

  async fn check_ratelimits(&mut self, bot_id: &u64, token: &str, route: &RouteInfo) -> Result<RatelimitStatus, RedisErrorWrapper> {  
    let use_global_rl = match route.resource {
      Resources::Interactions => false,
      Resources::Webhooks => false,
      _ => true
    };
    
    // println!("[{}] Using Global Ratelimit : {}", route.bucket, use_global_rl);

    if use_global_rl {
      Ok(self.is_global_or_bucket_ratelimited(&bot_id, &token, route).await?)
    } else {
      Ok(self.is_route_ratelimited(route).await?)
    }
  }

  async fn is_global_or_bucket_ratelimited(&mut self, bot_id: &u64, token: &str, route: &RouteInfo) -> Result<RatelimitStatus, RedisErrorWrapper> {
    let mut redis = self.redis.pool.get().await?;
    let global_rl_key = bot_id.to_string();

    loop {
      let ratelimits = RedisClient::check_global_and_route_ratelimits()
        .key(&bot_id)
        .key(&route.bucket)
      .invoke_async::<_,Vec<Option<u16>>>(&mut redis).await?;

      let global_ratelimit = ratelimits[0];
      let route_ratelimit = ratelimits[1];

      // println!("[{}] Ratelimit Status - Global: {:?} - [{}]: {:?}", bot_id, &global_ratelimit, &route.bucket, &route_ratelimit);

      match global_ratelimit {
        Some(count) => {
          let hit_ratelimit = count == 0;

          if hit_ratelimit {
            break Ok(RatelimitStatus::GlobalRatelimited)
          }
        },
        None => {
          // println!("[{}] Ratelimit not set, will try to acquire a lock and set it...", &bucket_rl_key);

          let lock_value = random_string(8);
          let lock = RedisClient::lock_bucket()
            .key(&global_rl_key)
              .arg(&lock_value)
          .invoke_async::<_, bool>(&mut redis).await?;

          if lock {
            // println!("[{}] Global ratelimit lock acquired, fetching from Discord.", &ratelimit_key);
            let ratelimit = fetch_discord_global_ratelimit(token).await?;
  
            if RedisClient::unlock_global_bucket()
              .key(&global_rl_key)
                .arg(&lock_value)
                .arg(ratelimit)
            .invoke_async::<_, bool>(&mut redis).await? {
              // println!("[{}] Global ratelimit set to {} and lock released.", ratelimit_key, &ratelimit);
            } else {
              // println!("[{}] Global ratelimit lock expired before we could release it.", &ratelimit_key);
            }

            continue;
          } else {
            if self.config.global == NewBucketStrategy::Strict {
              // println!("Lock is taken and ratelimit config is Strict, retrying in {}ms.", self.config.lock_timeout.as_millis());
              
              sleep(self.config.lock_timeout).await;
              continue;
            }

            // println!("Lock is taken and ratelimit config is Loose, skipping ratelimit check.");
          }
        }
      };

      match route_ratelimit {
        Some(count) => {
          let hit_ratelimit = count == 0;

          if hit_ratelimit {
            break Ok(RatelimitStatus::RouteRatelimited)
          }
        },
        None => {
          // println!("[{}] Ratelimit not set, will try to acquire a lock and set it...", &bucket_rl_key);

          let lock_value = random_string(8);
          let lock = RedisClient::lock_bucket()
            .key(&route.bucket)
              .arg(&lock_value)
          .invoke_async::<_, bool>(&mut redis).await?;

          if lock {
            // println!("[{}] Acquired bucket lock.", bucket_rl_key);

            break Ok(RatelimitStatus::Ok(Some(lock_value)))
          } else {
            if self.config.buckets == NewBucketStrategy::Strict {
              // println!("Lock is taken and ratelimit config is Strict, retrying in {}ms.", self.config.lock_timeout.as_millis());
              
              sleep(self.config.lock_timeout).await;
              continue;
            }

            // println!("Lock is taken and ratelimit config is Loose, skipping ratelimit check.");
          }
        }
      };

      break Ok(RatelimitStatus::Ok(None))
    }
  }

  async fn is_route_ratelimited(&mut self, route: &RouteInfo) -> Result<RatelimitStatus, RedisErrorWrapper> {
    let mut redis = self.redis.pool.get().await.unwrap();

    return loop {
      let ratelimit = RedisClient::check_route_ratelimit()
        .key(&route.bucket)
      .invoke_async::<_,Option<u16>>(&mut redis).await?;

      // println!("[{}] Bucket Ratelimit Status: {} - Count: {}", bot_id, &ratelimit&route.bucket);

      match ratelimit {
        Some(count) => {
          let hit_ratelimit = count == 0;

          if hit_ratelimit {
            break Ok(RatelimitStatus::RouteRatelimited)
          }
        },
        None => {
          // println!("[{}] Ratelimit not set, will try to acquire a lock and set it...", &bucket_rl_key);

          let lock_value = random_string(8);
          let lock = RedisClient::lock_bucket()
            .key(&route.bucket)
              .arg(&lock_value)
          .invoke_async::<_, bool>(&mut redis).await?;

          if lock {
            // println!("[{}] Acquired bucket lock.", route.bucket);

            break Ok(RatelimitStatus::Ok(Some(lock_value)))
          } else {
            if self.config.buckets == NewBucketStrategy::Strict {
              // println!("Lock is taken and ratelimit config is Strict, retrying in {}ms.", self.config.lock_timeout.as_millis());
              
              sleep(self.config.lock_timeout).await;
              continue;
            }

            // println!("Lock is taken and ratelimit config is Loose, skipping ratelimit check.");
          }
        }
      };

      break Ok(RatelimitStatus::Ok(None))
    };
  }

  async fn update_ratelimits(&mut self, bot_id: u64, headers: &HeaderMap, bucket: String, bucket_lock: Option<String>, sent_request_at: u128) -> Result<(), RedisErrorWrapper> {
    let bucket_limit = match headers.get("X-RateLimit-Limit") {
      Some(limit) => limit.clone().to_str().unwrap().parse::<u16>().unwrap(),
      None => 0
    };

    let reset_at = match headers.get("X-RateLimit-Reset") {
      Some(timestamp) => {
        timestamp.clone()
          .to_str().unwrap()
          .replace(".", "")
          .to_string()
        },
      None => "0".to_string()
    };

    let redis = self.redis.clone();
    let mut redis_conn = redis.pool.get().await?;

    if bucket_lock.is_some() {
      tokio::task::spawn(async move {
        // println!("[{}] New bucket! Setting ratelimit to {}, resetting at {}", route_info.bucket, bucket_limit, reset_at);
  
        RedisClient::expire_global_and_unlock_route_buckets()
          .key(&bot_id)
            .arg(&(sent_request_at as u64 + 1000))
          .key(bucket)
            .arg(&bucket_lock.unwrap())
            .arg(&bucket_limit)
            .arg(&reset_at)
        .invoke_async::<_, bool>(&mut redis_conn).await
        .expect("Failed to unlock route bucket and update expiry times.");
      });
    } else {
      tokio::task::spawn(async move {
        RedisClient::expire_global_and_route_buckets()
          .key(&bot_id)
            .arg(&(sent_request_at as u64 + 1000))
          .key(bucket)
            .arg(&reset_at)
        .invoke_async::<_, bool>(&mut redis_conn).await
        .expect("Failed to update bucket expiry times.");
      });
    }

    Ok(())
  }
}

fn parse_headers(headers: &HeaderMap, route: &RouteInfo) -> Result<(String, u64), String> {
  if route.resource == Resources::Webhooks || route.resource == Resources::Interactions {
    let mut path_segments = route.bucket.split("/").skip(1);

    let id = match path_segments.next() {
      Some(id) => id.parse::<u64>().map_err(|_| "Invalid ID")?,
      None => return Err("Invalid ID".to_string())
    };

    let token = match path_segments.next() {
      Some(token) => token.parse::<String>().map_err(|_| "Invalid Token")?,
      None => return Err("Invalid Token".to_string())
    };

    return Ok((token, id))
  }

  // if a token retrieval function is not configured, use that instead of this
  let token = match headers.get("Authorization") {
    Some(header) => {
      let token = match header.to_str() {
        Ok(token) => token,
        Err(_) => return Err("Invalid Authorization header".to_string())
      };

      if !token.starts_with("Bot ") {
        return Err("Invalid Authorization header".to_string())
      }

      token.to_string()
    },
    None => return Err("Missing Authorization header".to_string())
  };

  let base64_bot_id = match token[4..].split('.').nth(0) {
    Some(base64_bot_id) => base64_bot_id,
    None => return Err("Invalid Authorization header b64".to_string())
  };

  let bot_id_b = decode(base64_bot_id).map_err(|e| {
    eprintln!("Error decoding base64 bot id: {:?}", e);

    "Invalid Authorization header".to_string()
  })?;

  let bot_id_s = String::from_utf8(bot_id_b).map_err(|e| {
    eprintln!("Error decoding base64 bot as string: {:?}", e);

    "Invalid Authorization header".to_string()
  })?;

  let bot_id = bot_id_s.parse::<u64>().map_err(|e| {
    eprintln!("Error parsing bot id as u64: {:?}", e);

    "Invalid Authorization header".to_string()
  })?;

  Ok((token, bot_id))
}

fn random_string(n: usize) -> String {
  thread_rng().sample_iter(&Alphanumeric)
    .take(n)
    .map(char::from)
    .collect()
}

fn generate_ratelimit_response(bucket: &str) -> Response<Body> {
  let mut res = Response::new(Body::from("You are being ratelimited."));
  *res.status_mut() = hyper::StatusCode::TOO_MANY_REQUESTS;

  res.headers_mut().insert("x-sent-by-proxy", HeaderValue::from_static("true"));
  res.headers_mut().insert("x-ratelimit-bucket", HeaderValue::from_str(bucket).unwrap());

  res
}

#[derive(Error, Debug)]
pub enum ProxyError {
  #[error("Redis Error: {0}")]
  RedisError(#[from] RedisErrorWrapper),

  #[error("Error proxying request: {0}")]
  RequestError(#[from] hyper::Error)
}
