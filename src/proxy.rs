use std::{time::{SystemTime, UNIX_EPOCH}, str::FromStr, sync::{atomic::{AtomicBool, Ordering}, Arc}};
use base64::decode;
use fred::prelude::RedisError;
use http::header::{CONNECTION, TRANSFER_ENCODING, UPGRADE};
use hyper::{Body, Response, StatusCode, http::HeaderValue, HeaderMap, Uri, Client, client::{HttpConnector, connect::dns::GaiResolver}};
use hyper_rustls::{HttpsConnectorBuilder, HttpsConnector};
use thiserror::Error;
use std::time::Instant;

use crate::{buckets::{Resources, get_route_info, RouteInfo}, ratelimits::RatelimitStatus, discord::DiscordError, redis::ProxyRedisClient, config::{ProxyEnvConfig, RedisEnvConfig}};

#[cfg(feature = "trust-dns")]
use hyper_trust_dns::TrustDnsResolver;

#[cfg(feature = "metrics")]
use {
  prometheus::{TextEncoder, Encoder},
  crate::metrics::{register_metrics, REGISTRY, RESPONSE_TIME_COLLECTOR}
};

const INTERNAL_PROXY_ERROR: &'static str = "Internal Proxy Error";

#[derive(Clone)]
pub struct Proxy<Resolver = GaiResolver> {
  disabled: Arc<AtomicBool>,

  pub redis: Arc<ProxyRedisClient>,
  pub http_client: Client<HttpsConnector<HttpConnector<Resolver>>, Body>,

  pub config: Arc<ProxyEnvConfig>,
}

impl Proxy {
  pub async fn new(config: Arc<ProxyEnvConfig>, redis_config: Arc<RedisEnvConfig>) -> Result<Self, ProxyError> {
    let redis_client = ProxyRedisClient::new(redis_config).await
      .map_err(|err| ProxyError::RedisInitFailed(err))?;

    #[cfg(feature = "metrics")]
    register_metrics();

    let mut http_connector: HttpConnector<> = HttpConnector::new();

    http_connector.enforce_http(false);

    let builder = HttpsConnectorBuilder::new()
      .with_webpki_roots()
      .https_only()
      .enable_http1();

    let builder = if !config.disable_http2 {
      builder.enable_http2().wrap_connector(http_connector)
    } else {
      builder.wrap_connector(http_connector)
    };

    Ok(Self {
      disabled: Arc::new(AtomicBool::new(false)),

      redis: Arc::new(redis_client),
      http_client: Client::builder().build(builder),

      config,
    })
  }

  pub fn get_metrics(&self) -> Response<Body> {
    #[cfg(feature = "metrics")] {
      let mut buffer = Vec::new();
      if let Err(e) = TextEncoder::new().encode(&REGISTRY.gather(), &mut buffer) {
        eprintln!("Metrics could not be encoded: {}", e);
        return Response::new(Body::from("Internal Server Error"))
      };

      let res = match String::from_utf8(buffer.clone()) {
          Ok(v) => v,
          Err(e) => {
            eprintln!("Metrics buffer could not be converted to string: {}", e);
            return Response::new(Body::from("Internal Server Error"))
          }
      };
      buffer.clear();
    
      return Response::new(Body::from(res))
    }

    #[cfg(not(feature = "metrics"))]
    return Response::new(Body::from("Metrics are disabled."))
  }

  pub async fn process(&self, req: http::Request<Body>) -> Response<Body> {
    let start = Instant::now();

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    
    let route_info = get_route_info(&method, &path);

    let auth = match parse_headers(req.headers(), &route_info) {
      Ok(auth) => auth,
      Err(message) => return Response::builder().status(400).body(message.into()).unwrap()
    };

    let (id, token): (&str, Option<&str>) = match &auth {
      Some((id, token)) => (id, Some(token)),
      None => ("noauth", None)
    };

    let route_bucket = format!("{{{}:{}-{}}}", id, method.to_string(), &route_info.route);

    let res = match self.handle_request(req, &route_info, &route_bucket, id, token).await {
      Ok(response) => response,
      Err(err) => {
        log::error!("Internal Server Error: {:?}", err);
  
        Response::builder().status(500).body(Body::from(INTERNAL_PROXY_ERROR)).unwrap()
      }
    };

    #[cfg(feature = "metrics")]
    RESPONSE_TIME_COLLECTOR.with_label_values(
      &[id, &method.to_string(), &route_info.route, res.status().as_str()]
    ).observe(start.elapsed().as_secs_f64());

    log::debug!("[{}] Proxied request in {}ms. Status Code: {}", &route_bucket, start.elapsed().as_millis(), res.status());

    return res;
  }

  pub async fn handle_request(&self, mut req: http::Request<Body>, route_info: &RouteInfo, route_bucket: &str, id: &str, token: Option<&str>) -> Result<Response<Body>, ProxyError> {
    let ratelimit_status = self.check_ratelimits(id, &token, &route_info, &route_bucket).await?;

    match ratelimit_status {
      RatelimitStatus::GlobalRatelimited => {
        return Ok(generate_ratelimit_response(id));
      },
      RatelimitStatus::RouteRatelimited => {
        return Ok(generate_ratelimit_response(&route_info.route));
      },
      RatelimitStatus::ProxyOverloaded => {
        return Ok(Response::builder().status(503)
          .header("x-sent-by-proxy", "true")
          .body("Proxy Overloaded".into()).unwrap());
      },
      _ => {}
    }

    req.headers_mut().insert("Host", HeaderValue::from_static("discord.com"));
    req.headers_mut().insert("User-Agent", HeaderValue::from_static("limbo-labs/discord-api-proxy/1.0"));

    // Remove HTTP2 headers
    req.headers_mut().remove(CONNECTION);
    req.headers_mut().remove("keep-alive");
    req.headers_mut().remove("proxy-connection");
    req.headers_mut().remove(TRANSFER_ENCODING);
    req.headers_mut().remove(UPGRADE);
    
    let path_and_query = match req.uri().path_and_query() {
      Some(path_and_query) => path_and_query.as_str(),
      None => "/"
    };

    *req.uri_mut() = Uri::from_str(&format!("https://discord.com{}", path_and_query)).unwrap();

    if self.disabled.load(Ordering::Acquire) {
      return Ok(Response::builder().status(503).body("Temporarily Overloaded".into()).unwrap());
    }

    let result = self.http_client.request(req).await.map_err(|e| ProxyError::RequestError(e))?;
    let sent_request_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();

    if let RatelimitStatus::Ok(bucket_lock) = ratelimit_status {
      match self.update_ratelimits(id.to_string(), result.headers(), route_bucket.to_string(), bucket_lock, sent_request_at).await {
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
        let is_shared_ratelimit = result.headers().get("X-RateLimit-Scope").map(|v| v == "shared").unwrap_or(false);
        
        if is_shared_ratelimit {
          log::info!("Discord returned Shared 429!");
        } else {
          log::error!("Discord returned 429! Global: {:?} Scope: {:?} - ABORTING REQUESTS FOR {}ms!", result.headers().get("X-RateLimit-Global"), result.headers().get("X-RateLimit-Scope"), self.config.ratelimit_timeout.as_millis());
        
          self.disabled.store(true, Ordering::Release);
          tokio::time::sleep(self.config.ratelimit_timeout).await;
          self.disabled.store(false, Ordering::Release);
        }
      },
      _ => {
        log::warn!("Discord returned non 200 status code {}!", status.as_u16());
      }
    }

    Ok(result)
  }
}

fn parse_headers(headers: &HeaderMap, route_info: &RouteInfo) -> Result<Option<(String, String)>, String> {
  // Use auth header by default
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
    None => {
      if route_info.resource == Resources::Webhooks && route_info.route.split("/").count() != 2
       || route_info.resource == Resources::OAuth2
       || route_info.resource == Resources::Interactions { 
        return Ok(None)
      }
      
      return Err("Missing Authorization header".to_string())
    }
  };

  let base64_bot_id = match token[4..].split('.').nth(0) {
    Some(base64_bot_id) => base64_bot_id,
    None => return Err("Invalid Authorization header".to_string())
  };

  let bot_id = String::from_utf8(
    decode(base64_bot_id)
    .map_err(|_| "Invalid Authorization header".to_string())?
  ).map_err(|_| "Invalid Authorization header".to_string())?;

  Ok(Some((bot_id, token)))
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
  #[error("FATAL: Redis client could not be initialized. Is Redis running? {0}")]
  RedisInitFailed(RedisError),

  #[error("Redis Error: {0}")]
  RedisError(#[from] RedisError),

  #[error("Error Proxying Request: {0}")]
  RequestError(#[from] hyper::Error),
  
  #[error("Error fetching Global RL from Discord: {0}")]
  DiscordError(#[from] DiscordError),
}
