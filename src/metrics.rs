use prometheus::{CounterVec, HistogramOpts, HistogramVec, Opts, Registry};

use lazy_static::lazy_static;

lazy_static! {
    pub static ref REGISTRY: Registry = Registry::new();
    pub static ref RESPONSE_TIME_COLLECTOR: HistogramVec = HistogramVec::new(
        HistogramOpts::new(
            "request_response_times",
            "Results of attempted Discord API requests."
        )
        .buckets(vec![0.1, 0.25, 0.5, 1.0, 2.5]),
        &["id", "method", "route", "status"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref SHARED_429_COLLECTOR: CounterVec = CounterVec::new(
        Opts::new(
            "requests_shared_429s",
            "Number of requests for which a shared 429 was encountered."
        ),
        &["id", "method", "route"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref ROUTE_429_COLLECTOR: CounterVec = CounterVec::new(
        Opts::new(
            "requests_route_429s",
            "Number of requests for which a unique 429 was encountered."
        ),
        &["id", "method", "route"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref GLOBAL_429_COLLECTOR: CounterVec = CounterVec::new(
        Opts::new(
            "requests_global_429s",
            "Number of requests for which a global 429 was encountered."
        ),
        &["id"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref PROXY_ROUTE_429_COLLECTOR: CounterVec = CounterVec::new(
        Opts::new(
            "requests_proxy_route_429s",
            "Number of requests ratelimited by the proxy."
        ),
        &["id", "method", "route"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref PROXY_GLOBAL_429_COLLECTOR: CounterVec = CounterVec::new(
        Opts::new(
            "requests_proxy_global_429s",
            "Number of requests ratelimited by the proxy."
        ),
        &["id"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref PROXY_OVERLOAD_COLLECTOR: CounterVec = CounterVec::new(
        Opts::new(
            "requests_overloaded",
            "Number of requests for which the proxy was overloaded."
        ),
        &["id", "method", "route"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref PROXY_ERROR_COLLECTOR: CounterVec = CounterVec::new(
        Opts::new(
            "requests_proxy_error",
            "Number of requests for which the proxy encountered an unexpected error."
        ),
        &["id", "method", "route"]
    )
    .expect("Failed to create metrics collector.");
    pub static ref PROXY_REQUESTS: CounterVec = CounterVec::new(
        Opts::new(
            "requests_proxy",
            "Number of requests for which the proxy encountered an unexpected error."
        ),
        &["id"]
    )
    .expect("Failed to create metrics collector.");
}

pub fn register_metrics() {
    REGISTRY
        .register(Box::new(RESPONSE_TIME_COLLECTOR.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(SHARED_429_COLLECTOR.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(ROUTE_429_COLLECTOR.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(GLOBAL_429_COLLECTOR.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(PROXY_ROUTE_429_COLLECTOR.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(PROXY_GLOBAL_429_COLLECTOR.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(PROXY_OVERLOAD_COLLECTOR.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(PROXY_ERROR_COLLECTOR.clone()))
        .expect("Failed to register metrics collector.");

    REGISTRY
        .register(Box::new(PROXY_REQUESTS.clone()))
        .expect("Failed to register metrics collector.");
}
