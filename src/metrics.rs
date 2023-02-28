use prometheus::{
  HistogramOpts, HistogramVec, Registry,
};

use lazy_static::lazy_static;

lazy_static! {
  pub static ref REGISTRY: Registry = Registry::new();

  pub static ref RESPONSE_TIME_COLLECTOR: HistogramVec = HistogramVec::new(
    HistogramOpts::new(
      "request_results",
      "Results of attempted Discord API requests."
    ).buckets(
      vec![0.1, 0.25, 0.5, 1.0, 2.5]
    ),
    &["id", "method", "route", "status"]
  ).expect("Failed to create metrics collector.");
}

pub fn register_metrics() {
  REGISTRY
      .register(Box::new(RESPONSE_TIME_COLLECTOR.clone()))
      .expect("Failed to register metrics collector.");
}