use std::{net::SocketAddr, process::exit};
use axum::{Router, routing::get, handler::Handler};

use crate::{routes::{health, proxy_request, metrics}, config::{AppEnvConfig}, proxy::Proxy};

mod config;

#[cfg(feature = "metrics")]
mod metrics;

mod redis;
mod routes;

mod proxy;
mod ratelimits;
mod buckets;
mod discord;

#[tokio::main]
async fn main() {
  env_logger::init_from_env(
    env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info")
  );

  log::info!("Starting API proxy.");

  let config = AppEnvConfig::from_env();

  let discord_proxy = match Proxy::new(
    config.proxy,
    config.redis
  ).await {
    Ok(proxy) => proxy,
    Err(err) => {
      log::error!("Failed to create proxy: {}", err);
      exit(1);
    }
  };

  let addr: SocketAddr = format!("{}:{}", config.webserver.host, config.webserver.port).parse().expect("Failed to parse socket address.");

  let app = Router::new()
    .route("/health", get(health))
    .route("/metrics", get(metrics).with_state(discord_proxy.clone()))
    .route_service("/api/*path", proxy_request.with_state(discord_proxy));

  log::info!("Starting HTTP Server on http://{}", &addr);

  let server = axum::Server::bind(&addr)
    .serve(app.into_make_service())
    .with_graceful_shutdown(shutdown_signal());

  if let Err(err) = server.await {
    eprintln!("Axum Server Error: {}", err);
  }

  exit(0);
}

async fn shutdown_signal() {
  tokio::signal::ctrl_c()
    .await
    .expect("Tokio failed to register Ctrl-C handler.");

  println!("Received shutdown signal.");
}