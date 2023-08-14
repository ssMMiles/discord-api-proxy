use axum::response::Response;
use http::response::Builder;
use hyper::Body;

fn proxy_response_builder() -> Builder {
    Response::builder().header("x-sent-by-proxy", "true")
}

pub fn invalid_request(message: String) -> Response<Body> {
    proxy_response_builder()
        .status(400)
        .body(message.into())
        .expect("Response builder failed.")
}

pub fn ratelimited(bucket: &str, limit: u16, reset_at: u128, reset_after: u64) -> Response<Body> {
    proxy_response_builder()
        .status(429)
        .header("x-ratelimit-bucket", bucket)
        .header("x-ratelimit-limit", limit)
        .header("x-ratelimit-remaining", 0)
        .header("x-ratelimit-reset", (reset_at as f64 / 1000.0).to_string())
        .header(
            "x-ratelimit-reset-after",
            (reset_after as f64 / 1000.0).to_string(),
        )
        .body(Body::empty())
        .expect("Response builder failed.")
}

pub fn overloaded() -> Response<Body> {
    proxy_response_builder()
        .status(503)
        .body(Body::empty())
        .expect("Response builder failed.")
}

pub fn internal_error() -> Response<Body> {
    proxy_response_builder()
        .status(500)
        .body(Body::empty())
        .expect("Response builder failed.")
}
