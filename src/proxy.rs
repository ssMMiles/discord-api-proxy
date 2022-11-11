use std::{ops::{DerefMut, Deref}, time::{Duration, SystemTime, UNIX_EPOCH}, str::FromStr, sync::{atomic::{AtomicBool, Ordering}, Arc}};
use base64::decode;
use hyper::{Request, Client, client::HttpConnector, Body, Response, StatusCode, http::{HeaderValue, uri::PathAndQuery}, HeaderMap, Uri};
use hyper_tls::HttpsConnector;
use prometheus::HistogramVec;
use thiserror::Error;
use tokio::time::Instant;

use crate::{redis::{RedisClient, RedisErrorWrapper}, buckets::{Resources, get_route_info, RouteInfo, get_route_bucket}, ratelimits::RatelimitStatus, discord::DiscordError};

#[derive(Clone)]
pub struct ProxyWrapper {
  pub proxy: DiscordProxy
}

impl ProxyWrapper {
  pub fn new(config: DiscordProxyConfig, metrics: &Metrics, redis: &RedisClient, client: &Client<HttpsConnector<HttpConnector>>) -> Self {
    Self { 
      proxy: DiscordProxy { 
        config: config.clone(), 
        disabled: Arc::new(AtomicBool::new(false)),
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
  pub global: NewBucketStrategy,
  pub buckets: NewBucketStrategy,

  pub lock_timeout: Duration,
  pub ratelimit_timeout: Duration,

  pub enable_metrics: bool,
}

impl DiscordProxyConfig {
  pub fn new(global: NewBucketStrategy, buckets: NewBucketStrategy, lock_timeout: Duration, ratelimit_timeout: Duration, enable_metrics: bool) -> Self {
    Self { global, buckets, lock_timeout, ratelimit_timeout, enable_metrics }
  }
}

#[derive(Clone)]
pub struct Metrics {
  pub requests: HistogramVec,
}

#[derive(Clone)]
pub struct DiscordProxy {
  pub redis: RedisClient,
  pub client: Client<HttpsConnector<HttpConnector>>,

  disabled: Arc<AtomicBool>,

  metrics: Metrics,

  pub config: DiscordProxyConfig
}

impl DiscordProxy {
  pub async fn proxy_request(&mut self, mut req: Request<Body>) -> Result<Response<Body>, ProxyError> {
    let start = Instant::now();

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    
    let route_info = get_route_info(&method, &path);

    let (token, bot_id) = match parse_headers(req.headers(), &route_info) {
      Ok((token, bot_id)) => (token, bot_id),
      Err(message) => return Ok(
        Response::builder().status(400).body(message.into()).unwrap()
      )
    };

    let route_bucket = get_route_bucket(bot_id, &method, &path);

    let ratelimit_status = self.check_ratelimits(&bot_id, &token, &route_info, &route_bucket).await?;

    match ratelimit_status {
      RatelimitStatus::GlobalRatelimited => {
        return Ok(generate_ratelimit_response(&bot_id.to_string()));
      },
      RatelimitStatus::RouteRatelimited => {
        return Ok(generate_ratelimit_response(&route_bucket));
      },
      RatelimitStatus::ProxyOverloaded => {
        return Ok(Response::builder().status(503).body("Proxy Overloaded".into()).unwrap());
      },
      _ => {}
    }

    log::debug!("[{}] {}ms - {} {}", bot_id, start.elapsed().as_millis(), method, path);

    req.headers_mut().insert("Host", HeaderValue::from_static("discord.com"));
    req.headers_mut().insert("User-Agent", HeaderValue::from_static("RockSolidRobots Discord Proxy/1.0"));
    
    let path_and_query = match req.uri().path_and_query() {
      Some(path_and_query) => path_and_query.clone(),
      None => PathAndQuery::from_static("/")
    };
    *req.uri_mut() = Uri::from_str(&format!("https://discord.com{}", path_and_query)).unwrap();

    if self.disabled.load(Ordering::Acquire) {
      return Ok(Response::builder().status(503).body("Temporarily Overloaded".into()).unwrap());
    }

    let result = self.client.request(req).await.map_err(|e| ProxyError::RequestError(e))?;
    let sent_request_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();

    if let RatelimitStatus::Ok(bucket_lock) = ratelimit_status {
      match self.update_ratelimits(bot_id, result.headers(), route_bucket, bucket_lock, sent_request_at).await {
        Ok(_) => {},
        Err(e) => {
          log::error!("Error updating ratelimits after proxying request: {}", e);
        }
      }
    }

    let status = result.status();
    match status {
      StatusCode::OK => {}
      StatusCode::TOO_MANY_REQUESTS => {
        log::error!("ABORTING REQUESTS FOR {}ms! - Discord returned 429! Global: {:?}", self.config.ratelimit_timeout.as_millis(), result.headers().get("X-RateLimit-Global"));
        self.disabled.store(true, Ordering::Release);
        tokio::time::sleep(self.config.ratelimit_timeout).await;
        self.disabled.store(false, Ordering::Release);
      },
      _ => {
        log::error!("Discord returned non 200 status code {}!", status.as_u16());
      }
    }

    if self.config.enable_metrics {
      self.metrics.requests.with_label_values(
        &[&bot_id.to_string(), &method.to_string(), status.as_str(), &path]
      ).observe(start.elapsed().as_secs_f64());
    }

    log::debug!("Proxied request in {}ms. Status Code: {}", start.elapsed().as_millis(), result.status());

    Ok(result)
  }
}

fn parse_headers(headers: &HeaderMap, route_info: &RouteInfo) -> Result<(String, u64), String> {
  if route_info.resource == Resources::Webhooks || route_info.resource == Resources::Interactions {
    let mut path_segments = route_info.route.split("/").skip(1);

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

  // TODO
  // if a token retrieval function is not configured, use the auth header
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

  // this is horrible
  let base64_bot_id = match token[4..].split('.').nth(0) {
    Some(base64_bot_id) => base64_bot_id,
    None => return Err("Invalid Authorization header".to_string())
  };

  let bot_id = String::from_utf8(
    decode(base64_bot_id)
    .map_err(|_| "Invalid Authorization header".to_string())?
  ).map_err(|_| "Invalid Authorization header".to_string())?
  .parse::<u64>().map_err(|_| "Invalid Authorization header".to_string())?;

  Ok((token, bot_id))
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
  RequestError(#[from] hyper::Error),
  
  #[error("Error fetching Global ratelimit: {0}")]
  DiscordError(#[from] DiscordError),
}
