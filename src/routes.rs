use std::convert::Infallible;
use base64::decode;
use hyper::{Body, Request, Response, StatusCode};
use prometheus::{Registry, TextEncoder, Encoder};

use crate::proxy::ProxyWrapper;

pub async fn route(req: Request<Body>, proxy: ProxyWrapper, registry: Registry) -> Result<Response<Body>, Infallible> {
  let path = req.uri().path();

  if path.starts_with("/api/v") {
    proxy_discord_request(req, proxy).await
  } else if path == "/health" {
    Ok(Response::new(Body::from("OK")))
  } else if path == "/metrics" {
    
    // Gather the metrics.
    let mut buffer = vec![];
    let encoder = TextEncoder::new();
    let metric_families = registry.gather();
    encoder.encode(&metric_families, &mut buffer).unwrap();

    Ok(Response::new(Body::from(String::from_utf8(buffer).unwrap())))
  } else {
    Ok(Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap())
  }
}

async fn proxy_discord_request(req: Request<Body>, mut proxy: ProxyWrapper) -> Result<Response<Body>, Infallible> {
  let (token, bot_id) = match check_headers(&req) {
    Ok((token, bot_id)) => (token, bot_id),
    Err(message) => return Ok(Response::builder().status(400).body(message.into()).unwrap())
  };
  
  let result = match proxy.proxy_request(&bot_id, &token, req).await {
    Ok(result) => result,
    Err(err) => {
      match err {
        _ => {
          eprintln!("Internal Server Error: {:?}", err);

          return Ok(Response::builder().status(500).body("Internal Server Error".into()).unwrap());
        }
      }
    }
  };

  Ok(result)
}

fn check_headers(req: &Request<Body>) -> Result<(String, u64), String> {
  // if a token retrieval function is not configured, use that instead of this
  let token = match req.headers().get("Authorization") {
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