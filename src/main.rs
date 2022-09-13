use std::{net::SocketAddr, env, time::Duration};
use hyper::{Client, service::{service_fn}, server::conn::Http};
use hyper_tls::HttpsConnector;
use prometheus::Registry;
use tokio::{net::TcpListener, sync::watch};

use crate::{proxy::{ProxyWrapper, NewBucketStrategy, DiscordProxyConfig, Metrics}, routes::route, redis::RedisClient};

mod redis;

mod routes;

mod proxy;
mod ratelimits;
mod buckets;
mod discord;

#[tokio::main]
async fn main() {
  // Graceful shutdown channel
  let (tx, mut rx) = watch::channel(false);

  tokio::task::spawn(async move {
    ctrlc::set_handler(
      move || match tx.send(false) {
        Err(_) => eprintln!("Failed to send shutdown signal."),
        _ => ()
      })
    .expect("Failed to set Ctrl-C handler.");
  });

  println!("Starting API proxy.");

  let redis_host = env::var("REDIS_HOST").unwrap_or("127.0.0.1".to_string());
  let redis_port = env::var("REDIS_PORT").unwrap_or("6379".to_string())
    .parse::<u16>().expect("REDIS_PORT must be a valid port number.");
  
  let redis_pool_size = env::var("REDIS_POOL_SIZE").unwrap_or("64".to_string())
    .parse::<usize>().expect("REDIS_POOL_SIZE must be a valid integer.");

  let storage = RedisClient::new(&redis_host, redis_port, redis_pool_size).await;
  println!("Connected to Redis.");

  let https = HttpsConnector::new();
  let https_client = Client::builder()
    .build::<_, hyper::Body>(https);

  let prometheus_registry = Registry::new();
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

  let enable_metrics: bool = env::var("METRICS").unwrap_or("true".to_string())
    .parse().expect("METRICS must be a boolean value.");

  let proxy = ProxyWrapper::new(DiscordProxyConfig::new(
    NewBucketStrategy::Strict,
    NewBucketStrategy::Strict,
    Duration::from_millis(300),
    enable_metrics
  ), &metrics, &storage, &https_client);

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
                    // eprintln!("Incomplete message.");
                  } else if err.is_closed() {
                    // eprintln!("Sender channel closed.");
                  } else if err.is_canceled() {
                    // eprintln!("Request canceled.");
                  } else if err.is_connect() {
                    // eprintln!("Error serving connection: {:?}", err);
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
}