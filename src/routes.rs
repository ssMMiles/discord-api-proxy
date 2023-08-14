use axum::{extract::State, response::Response};
use http::Request;
use hyper::Body;

use crate::proxy::Proxy;

pub async fn health() -> &'static str {
    "OK"
}

pub async fn proxy(State(proxy): State<Proxy>, req: Request<Body>) -> Response<Body> {
    proxy.handle_request(req).await
}

pub async fn metrics(State(proxy): State<Proxy>) -> Response<Body> {
    proxy.get_metrics()
}
