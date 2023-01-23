use std::{net::SocketAddr, env, time::Duration, process::exit, str::FromStr, sync::Arc};
use hyper::{Client, service::{service_fn}, server::conn::Http};
use hyper_tls::HttpsConnector;
use prometheus::Registry;
use tokio::{net::TcpListener, sync::watch};

use crate::{proxy::{DiscordProxyConfig, Metrics, DiscordProxy}, routes::route, redis::RedisClient};

mod redis;

mod routes;

mod proxy;
mod ratelimits;
mod buckets;
mod discord;

#[derive(Clone, PartialEq)]
pub enum NewBucketStrategy {
  Strict,
  Loose
}

impl FromStr for NewBucketStrategy {
  type Err = ();

  fn from_str(input: &str) -> Result<NewBucketStrategy, Self::Err> {
    match input.to_lowercase().as_str() {
        "strict"  => Ok(NewBucketStrategy::Strict),
        "loose"  => Ok(NewBucketStrategy::Loose),
        _      => Err(()),
    }
  }
}

#[tokio::main]
async fn main() {
  env_logger::init();

  // Graceful shutdown channel
  let (tx, mut rx) = watch::channel(false);

  tokio::task::spawn(async move {
    ctrlc::set_handler(
      move || match tx.send(false) {
        Err(_) => log::error!("Failed to send shutdown signal."),
        _ => ()
      })
    .expect("Failed to set Ctrl-C handler.");
  });

  log::info!("Starting API proxy.");

  let redis_host = env::var("REDIS_HOST").unwrap_or("127.0.0.1".to_string());
  let redis_port = env::var("REDIS_PORT").unwrap_or("6379".to_string())
    .parse::<u16>().expect("REDIS_PORT must be a valid port number.");

  let redis_user = env::var("REDIS_USER").unwrap_or("".to_string());
  let redis_pass = env::var("REDIS_PASS").unwrap_or("".to_string());

  let redis_pool_size = env::var("REDIS_POOL_SIZE").unwrap_or("48".to_string())
    .parse::<usize>().expect("REDIS_POOL_SIZE must be a valid integer.");

  let lock_wait_timeout = env::var("LOCK_WAIT_TIMEOUT").unwrap_or("500".to_string())
    .parse::<u64>().expect("LOCK_WAIT_TIMEOUT must be a valid integer. (ms)");
  let ratelimit_timeout = env::var("RATELIMIT_ABORT_PERIOD").unwrap_or("1000".to_string())
    .parse::<u64>().expect("RATELIMIT_ABORT_PERIOD must be a valid integer. (ms)");

  let global_ratelimit_strategy = env::var("GLOBAL_RATELIMIT_STRATEGY").unwrap_or("strict".to_string())
    .parse::<NewBucketStrategy>().expect("GLOBAL_RATELIMIT_STRATEGY must be either 'Strict' or 'Loose'.");
  let bucket_ratelimit_strategy = env::var("BUCKET_RATELIMIT_STRATEGY").unwrap_or("strict".to_string())
    .parse::<NewBucketStrategy>().expect("BUCKET_RATELIMIT_STRATEGY must be either 'Strict' or 'Loose'.");

  let redis = RedisClient::new(redis_host, redis_port, redis_user, redis_pass, redis_pool_size).await;
  log::info!("Connected to Redis.");

  let https = HttpsConnector::new();
  let https_client = Client::builder()
    .build::<_, hyper::Body>(https);

  let prometheus_registry = Arc::new(Registry::new());
  let request_histogram = prometheus::HistogramVec::new(prometheus::HistogramOpts::new(
    "request_results",
    "Results of attempted Discord API requests."
  ).buckets(
    vec![0.1, 0.25, 0.5, 1.0, 2.5]),
    &["bot_id", "method", "route", "status"]).unwrap();

  prometheus_registry.register(Box::new(request_histogram.clone())).unwrap();

  let metrics = Metrics {
    requests: request_histogram.clone()
  };

  let enable_metrics: bool = env::var("ENABLE_METRICS").unwrap_or("true".to_string())
    .parse().expect("METRICS must be a boolean value.");

  let global_time_slice_offset_ms: u64 = env::var("GLOBAL_TIME_SLICE_OFFSET").unwrap_or("200".to_string())
    .parse().expect("GLOBAL_TIME_SLICE_OFFSET must be a valid integer. (ms)");

  let proxy_config = DiscordProxyConfig::new(
    global_ratelimit_strategy,
    bucket_ratelimit_strategy,
    global_time_slice_offset_ms,
    Duration::from_millis(lock_wait_timeout),
    Duration::from_millis(ratelimit_timeout),
    enable_metrics
  );

  let proxy = DiscordProxy::new(
    redis, https_client, metrics, proxy_config
  );

  let host = env::var("HOST").unwrap_or("127.0.0.1".to_string());
  let port = env::var("PORT").unwrap_or("8080".to_string());

  let addr: SocketAddr = format!("{}:{}", host, port).parse().expect("Failed to parse socket address.");
  let listener = TcpListener::bind(addr).await.expect("Failed to bind to socket.");
  
  println!("Starting HTTP Server on http://{}", addr);

  let webserver = tokio::task::spawn(async move {
    loop {
      tokio::select! {
        res = listener.accept() => {
          let (stream, _) = res.expect("Failed to accept");

          let mut rx = rx.clone();

          let prometheus_registry = prometheus_registry.clone();
          let proxy = proxy.clone();

          tokio::task::spawn(async move {
            let mut conn = Http::new().serve_connection(stream, service_fn(move |req| {
              route(req, proxy.clone(), prometheus_registry.clone())
            })).with_upgrades();

            let mut conn = Pin::new(&mut conn);

            tokio::select! {
              res = &mut conn => {
                if let Err(err) = res {
                  if err.is_incomplete_message() {
                    log::error!("Incomplete message.");
                  } else if err.is_closed() {
                    log::error!("Sender channel closed.");
                  } else if err.is_canceled() {
                    log::error!("Request canceled.");
                  } else if err.is_connect() {
                    log::error!("Error serving connection: {:?}", err);
                  }
                }
              }
              // Polling for graceful shutdown.
              _ = rx.changed() => {
                conn.graceful_shutdown();
              }
            }
          });
        }
        _ = rx.changed() => {
          break;
        }
      }
    }
  });

  tokio::select! {
    _ = webserver => {
      println!("Webserver exited.");
    }
  }

  exit(0);
}