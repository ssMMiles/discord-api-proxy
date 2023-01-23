use std::{convert::Infallible, sync::Arc};
use hyper::{Body, Request, Response, StatusCode};
use prometheus::{Registry, TextEncoder, Encoder};

use crate::proxy::{DiscordProxy};

pub async fn route(req: Request<Body>, mut proxy: DiscordProxy, registry: Arc<Registry>) -> Result<Response<Body>, Infallible> {
  let path = req.uri().path();

  if path.starts_with("/api/v") {
    match proxy.proxy_request(req).await {
      Ok(result) => Ok(result),
      Err(err) => {
        match err {
          _ => {
            log::error!("Internal Server Error: {:?}", err);
  
            return Ok(Response::builder().status(500).body("Internal Server Error".into()).unwrap());
          }
        }
      }
    }
  } else if path == "/health" {
    Ok(Response::new(Body::from("OK")))
  } else if path == "/metrics" {

    if !proxy.config.enable_metrics {
      return Ok(Response::new(Body::from("Metrics are disabled.")));
    }
    
    // Gather the metrics.
    let mut buffer = vec![];
    let metric_families = registry.gather();

    TextEncoder::new().encode(&metric_families, &mut buffer).unwrap();

    Ok(Response::new(Body::from(String::from_utf8(buffer).unwrap())))
  } else {
    Ok(Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap())
  }
}