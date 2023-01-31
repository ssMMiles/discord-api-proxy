use axum::{extract::State, response::Response};
use http::Request;
use hyper::Body;

use crate::proxy::Proxy;

const INTERNAL_ERROR: &'static str = "OK";

pub async fn health() -> &'static str {
  "OK"
}

pub async fn proxy_request(State(state): State<Proxy>, req: Request<Body>) -> Response<Body> {
  match state.handle_request(req).await {
    Ok(response) => response,
    Err(err) => {
      log::error!("Internal Server Error: {:?}", err);

      return Response::builder().status(500).body(Body::from(INTERNAL_ERROR)).unwrap();
    }
  }
}

pub async fn metrics() {
  // if !proxy.config.enable_metrics {
  //   return Ok(Response::new(Body::from("Metrics are disabled.")));
  // }
  
  // // Gather the metrics.
  // let mut buffer = vec![];
  // let metric_families = registry.gather();

  // TextEncoder::new().encode(&metric_families, &mut buffer).unwrap();

  // Ok(Response::new(Body::from(String::from_utf8(buffer).unwrap())))
}