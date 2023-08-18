use std::sync::atomic::Ordering;

use axum::response::Response;
use hyper::Body;
use lazy_static::lazy_static;
use prometheus::{
    Counter, CounterVec, Encoder, HistogramOpts, HistogramVec, Opts, Registry, TextEncoder,
};

use crate::proxy::Proxy;

lazy_static! {
    pub static ref REGISTRY: Registry = Registry::new();
    pub static ref DISCORD_REQUEST_RESPONSE_TIMES: HistogramVec = HistogramVec::new(
        HistogramOpts::new(
            "discord_request_response_times",
            "Results of attempted Discord API requests."
        )
        .buckets(vec![0.1, 0.2, 0.3, 0.4, 0.6, 1.0, 2.5, 5.0]),
        &["global_id", "route", "status"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref DISCORD_REQUEST_COUNTER: CounterVec = CounterVec::new(
        Opts::new(
            "discord_request_counter",
            "Number of requests for which the proxy encountered an unexpected error."
        ),
        &["global_id", "route"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref DISCORD_REQUEST_SHARED_429: CounterVec = CounterVec::new(
        Opts::new(
            "discord_request_shared_429",
            "Number of requests for which a shared 429 was encountered."
        ),
        &["global_id", "route"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref DISCORD_REQUEST_ROUTE_429: CounterVec = CounterVec::new(
        Opts::new(
            "discord_request_route_429",
            "Number of requests for which a unique 429 was encountered."
        ),
        &["global_id", "route"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref DISCORD_REQUEST_GLOBAL_429: CounterVec = CounterVec::new(
        Opts::new(
            "discord_request_global_429",
            "Number of requests for which a global 429 was encountered."
        ),
        &["global_id"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref PROXY_REQUEST_RATELIMIT_CHECK_TIMES: HistogramVec = HistogramVec::new(
        HistogramOpts::new(
            "proxy_request_ratelimit_check_times",
            "Time taken to check ratelimits for a request."
        )
        .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25]),
        &["global_id", "route",]
    )
    .expect("Failed to create metrics collector.");
    pub static ref PROXY_REQUEST_COUNTER: CounterVec = CounterVec::new(
        Opts::new(
            "proxy_request_counter",
            "Number of requests for which the proxy encountered an unexpected error."
        ),
        &["global_id", "route"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref PROXY_REQUEST_ROUTE_429: CounterVec = CounterVec::new(
        Opts::new(
            "proxy_request_route_429",
            "Number of requests ratelimited by the proxy."
        ),
        &["global_id", "route"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref PROXY_REQUEST_GLOBAL_429: CounterVec = CounterVec::new(
        Opts::new(
            "proxy_request_global_429",
            "Number of requests ratelimited by the proxy."
        ),
        &["global_id"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref PROXY_REQUEST_OVERLOADED: CounterVec = CounterVec::new(
        Opts::new(
            "proxy_request_overloaded",
            "Number of requests for which the proxy was overloaded."
        ),
        &["global_id", "route"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref PROXY_REQUEST_ERRORS: Counter = Counter::new(
        "proxy_request_error",
        "Number of requests for which the proxy encountered an unexpected error."
    )
    .expect("Failed to create metrics collector.");
}

pub fn register_metrics() {
    REGISTRY
        .register(Box::new(DISCORD_REQUEST_RESPONSE_TIMES.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(DISCORD_REQUEST_COUNTER.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(DISCORD_REQUEST_SHARED_429.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(DISCORD_REQUEST_ROUTE_429.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(DISCORD_REQUEST_GLOBAL_429.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(PROXY_REQUEST_RATELIMIT_CHECK_TIMES.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(PROXY_REQUEST_COUNTER.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(PROXY_REQUEST_ROUTE_429.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(PROXY_REQUEST_GLOBAL_429.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(PROXY_REQUEST_OVERLOADED.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(PROXY_REQUEST_ERRORS.clone()))
        .expect("Failed to register metrics collector.");

    reset_metrics();
}

pub fn reset_metrics() {
    DISCORD_REQUEST_RESPONSE_TIMES.reset();
    DISCORD_REQUEST_COUNTER.reset();
    DISCORD_REQUEST_SHARED_429.reset();
    DISCORD_REQUEST_ROUTE_429.reset();
    DISCORD_REQUEST_GLOBAL_429.reset();
    PROXY_REQUEST_RATELIMIT_CHECK_TIMES.reset();
    PROXY_REQUEST_COUNTER.reset();
    PROXY_REQUEST_ROUTE_429.reset();
    PROXY_REQUEST_GLOBAL_429.reset();
    PROXY_REQUEST_OVERLOADED.reset();
    PROXY_REQUEST_ERRORS.reset();
}

impl Proxy {
    pub fn get_metrics(&self) -> Response<Body> {
        let mut buffer = Vec::new();
        if let Err(e) = TextEncoder::new().encode(&REGISTRY.gather(), &mut buffer) {
            eprintln!("Metrics could not be encoded: {}", e);
            return Response::new(Body::from("Internal Server Error"));
        };

        let res = match String::from_utf8(buffer.clone()) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Metrics buffer could not be converted to string: {}", e);
                return Response::new(Body::from("Internal Server Error"));
            }
        };
        buffer.clear();

        let last_reset_at = self.metrics_last_reset_at.load(Ordering::Acquire);
        let current_timestamp = get_current_timestamp();

        if last_reset_at + self.config.metrics_ttl < current_timestamp {
            self.metrics_last_reset_at
                .store(current_timestamp, Ordering::Release);
            reset_metrics();
        }

        return Response::new(Body::from(res));
    }
}

fn get_current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
