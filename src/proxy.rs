use std::{time::{SystemTime, UNIX_EPOCH}, str::FromStr, sync::{atomic::{AtomicBool, Ordering}, Arc}};
use base64::decode;
use fred::prelude::RedisError;
use http::header::{CONNECTION, TRANSFER_ENCODING, UPGRADE};
use hyper::{Body, Response, StatusCode, http::HeaderValue, HeaderMap, Uri, Client, client::{HttpConnector, connect::dns::GaiResolver}};
use hyper_rustls::{HttpsConnectorBuilder, HttpsConnector};
use prometheus::{HistogramVec, Registry};
use thiserror::Error;
use tokio::time::Instant;

#[cfg(feature = "trust-dns")]
use hyper_trust_dns::TrustDnsResolver;

use crate::{buckets::{Resources, get_route_info, RouteInfo}, ratelimits::RatelimitStatus, discord::DiscordError, redis::ProxyRedisClient, config::{ProxyEnvConfig, RedisEnvConfig}};

#[derive(Clone)]
pub struct Metrics {
  pub requests: HistogramVec,
}

#[derive(Clone)]
pub struct Proxy<Resolver = GaiResolver> {
  disabled: Arc<AtomicBool>,
  _metrics: Arc<Option<Metrics>>,

  pub redis: Arc<ProxyRedisClient>,
  pub http_client: Client<HttpsConnector<HttpConnector<Resolver>>, Body>,

  pub config: Arc<ProxyEnvConfig>,
}

impl Proxy {
  pub async fn new(config: Arc<ProxyEnvConfig>, redis_config: Arc<RedisEnvConfig>) -> Result<Self, ProxyError> {
    let redis_client = ProxyRedisClient::new(redis_config).await
      .map_err(|err| ProxyError::RedisInitFailed(err))?;

    let metrics = if config.enable_metrics {
      let prometheus_registry = Arc::new(Registry::new());

      let request_histogram = HistogramVec::new(prometheus::HistogramOpts::new(
        "request_results",
        "Results of attempted Discord API requests."
      ).buckets(
        vec![0.1, 0.25, 0.5, 1.0, 2.5]),
        &["bot_id", "method", "route", "status"]).unwrap();
  
      prometheus_registry.register(Box::new(request_histogram.clone())).unwrap();

      Some(Metrics {
        requests: request_histogram.clone()
      })
    } else {
      None
    };

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
      
      _metrics: Arc::new(metrics),
      config,
    })
  }
// }

// impl<TrustDnsResolver> Proxy<TrustDnsResolver>{
//   pub async fn new(config: Arc<ProxyEnvConfig>, redis_config: Arc<RedisEnvConfig>) -> Result<Self, ProxyError> {
//     let redis_client = ProxyRedisClient::new(redis_config).await
//       .map_err(|err| ProxyError::RedisInitFailed(err))?;

//     let metrics = if config.enable_metrics {
//       let prometheus_registry = Arc::new(Registry::new());

//       let request_histogram = HistogramVec::new(prometheus::HistogramOpts::new(
//         "request_results",
//         "Results of attempted Discord API requests."
//       ).buckets(
//         vec![0.1, 0.25, 0.5, 1.0, 2.5]),
//         &["bot_id", "method", "route", "status"]).unwrap();
  
//       prometheus_registry.register(Box::new(request_histogram.clone())).unwrap();

//       Some(Metrics {
//         requests: request_histogram.clone()
//       })
//     } else {
//       None
//     };

//     let mut http_connector = if config.disable_ipv6 {
//       HttpConnector::new()
//     } else {
      
//     };

//     http_connector.enforce_http(false);

//     let builder = HttpsConnectorBuilder::new()
//       .with_webpki_roots()
//       .https_only()
//       .enable_http1()
//       .enable_http2()
//     .wrap_connector(http_connector);

//     Ok(Self {
//       disabled: Arc::new(AtomicBool::new(false)),

//       redis: Arc::new(redis_client),
//       http_client: Client::builder().build(builder),
      
//       _metrics: Arc::new(metrics),
//       config,
//     })
//   }

  pub async fn handle_request(&self, mut req: http::Request<Body>) -> Result<Response<Body>, ProxyError> {
    let start = Instant::now();

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    
    let route_info = get_route_info(&method, &path);

    let auth = match parse_headers(req.headers(), &route_info) {
      Ok(auth) => auth,
      Err(message) => return Ok(
        Response::builder().status(400).body(message.into()).unwrap()
      )
    };

    let (id, token): (&str, Option<&str>) = match &auth {
      Some((id, token)) => (id, Some(token)),
      None => ("noauth", None)
    };

    let route_bucket = format!("{{{}:{}-{}}}", id, method.to_string(), &route_info.route);

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

    if self.config.enable_metrics {
      // self.metrics.requests.with_label_values(
      //   &[id, &method.to_string(), &path, status.as_str()]
      // ).observe(start.elapsed().as_secs_f64());
    }

    log::debug!("[{}] Proxied request in {}ms. Status Code: {}", &route_bucket, start.elapsed().as_millis(), result.status());

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
