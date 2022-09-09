use std::{net::SocketAddr, env, time::Duration};
use graceful::SignalGuard;
use hyper::{Client, service::{service_fn}, server::conn::Http};
use hyper_tls::HttpsConnector;
use tokio::{net::TcpListener, sync::watch};

use crate::{proxy::{ProxyWrapper, NewBucketStrategy, DiscordProxyConfig}, routes::route, redis::client::RedisClient};

mod redis;

mod routes;
mod proxy;

mod global_rl;
mod buckets;

#[tokio::main]
async fn main() {
  println!("Starting API proxy.");

  let redis_host = env::var("REDIS_HOST")
  .expect("REDIS_PORT env var is not set");

  let redis_port = env::var("REDIS_PORT").unwrap_or("6379".to_string())
    .parse::<u16>().expect("REDIS_PORT must be a valid port number.");
  
  let storage = RedisClient::new(&redis_host, redis_port).await;

  println!("Connected to Redis.");

  let https = HttpsConnector::new();
  let https_client = Client::builder()
    .build::<_, hyper::Body>(https);

  let proxy = ProxyWrapper::new(DiscordProxyConfig::new(
    NewBucketStrategy::Loose, 
    NewBucketStrategy::Strict, 
    Duration::from_millis(300)
  ), &storage, &https_client);

  let host = env::var("HOST").unwrap_or("127.0.0.1".to_string());
  let port = env::var("PORT").unwrap_or("8080".to_string());

  let addr: SocketAddr = format!("{}:{}", host, port).parse().expect("Failed to parse socket address.");
  let listener = TcpListener::bind(addr).await.expect("Failed to bind to socket.");
  
  println!("Starting HTTP Server on http://{}", addr);

  // Graceful shutdown channel
  let (tx, mut rx) = watch::channel(false);

  let http_server = tokio::task::spawn(async move {
    loop {
      tokio::select! {
        res = listener.accept() => {
          let (stream, _) = res.expect("Failed to accept");

          let mut rx = rx.clone();
          let proxy = proxy.clone();

          tokio::task::spawn(async move {
            let mut conn = Http::new().serve_connection(stream, service_fn(move |req| {
              route(req, proxy.clone())
            })).with_upgrades();

            let mut conn = Pin::new(&mut conn);

            tokio::select! {
              res = &mut conn => {
                if let Err(err) = res {
                  let cause = match err.into_cause() {
                    Some(cause) => cause,
                    None => {
                      // eprintln!("Unknown error serving connection.");
                      return;
                    },
                  };
                    
                  eprintln!("Error serving connection: {:?}", cause);
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

  let signal_guard = SignalGuard::new();

  signal_guard.at_exit(move | sig | {
    match sig {
      15 => {
        println!("Received SIGTERM, shutting down...");
        tx.send(false).unwrap();
      },
      _ => {
        println!("Received signal {}", sig);
      }
    }
  });

  tokio::select! {
    _ = http_server => {
      println!("HTTP Server terminated.");
    }
  }
}