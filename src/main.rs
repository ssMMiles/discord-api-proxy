use axum::{handler::Handler, routing::get, Router};
use std::{net::SocketAddr, process::exit};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

use crate::{
    config::AppEnvConfig,
    proxy::Proxy,
    routes::{health, metrics, proxy_request},
};

mod config;

#[cfg(feature = "metrics")]
mod metrics;

mod redis;
mod routes;

mod buckets;
mod discord;
mod proxy;
mod ratelimits;

#[tokio::main]
async fn main() {
    tracing::subscriber::set_global_default(
        FmtSubscriber::builder()
            .with_max_level(Level::INFO)
            .finish(),
    )
    .expect("Setting default trace subscriber failed.");

    tracing::info!("Starting API proxy.");

    let config = AppEnvConfig::from_env();

    let discord_proxy = match Proxy::new(config.proxy, config.redis).await {
        Ok(proxy) => proxy,
        Err(err) => {
            tracing::error!("Failed to create proxy: {}", err);
            exit(1);
        }
    };

    let addr: SocketAddr = format!("{}:{}", config.webserver.host, config.webserver.port)
        .parse()
        .expect("Failed to parse socket address.");

    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics).with_state(discord_proxy.clone()))
        .route_service("/api/*path", proxy_request.with_state(discord_proxy));

    tracing::info!("Starting HTTP Server on http://{}", &addr);

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
