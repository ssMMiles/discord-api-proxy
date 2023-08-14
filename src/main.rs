use axum::{handler::Handler, routing::get, Router};
use fred::prelude::RedisError;
use std::{net::SocketAddr, process::exit};
use tracing_subscriber::{
    filter::LevelFilter, prelude::__tracing_subscriber_SubscriberExt, EnvFilter, Registry,
};

use crate::{
    config::AppEnvConfig,
    proxy::Proxy,
    routes::{health, metrics, proxy},
};

mod config;

#[cfg(feature = "metrics")]
mod metrics;

mod buckets;
mod discord;
mod proxy;
mod ratelimits;
mod redis;
mod request;
mod responses;
mod routes;

#[tokio::main]
async fn main() -> Result<(), RedisError> {
    tracing::subscriber::set_global_default(
        Registry::default()
            .with(
                EnvFilter::builder()
                    .with_default_directive(LevelFilter::INFO.into())
                    .from_env_lossy(),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    // .json()
                    .with_target(false)
                    // .with_current_span(true)
                    .with_target(false)
                    .compact(),
            ),
    )
    .expect("Setting default trace subscriber failed.");

    let config = AppEnvConfig::from_env();

    #[cfg(feature = "metrics")]
    metrics::register_metrics();

    let discord_proxy = Proxy::new(config.proxy, config.redis).await?;

    let addr: SocketAddr = format!("{}:{}", config.webserver.host, config.webserver.port)
        .parse()
        .expect("Failed to parse socket address.");

    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics).with_state(discord_proxy.clone()))
        .route_service("/api/*path", proxy.with_state(discord_proxy));

    tracing::info!("Serving API Proxy on http://{}", &addr);

    let server = axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .with_graceful_shutdown(shutdown_signal());

    if let Err(err) = server.await {
        eprintln!("Axum Server Error: {}", err);
    }

    tracing::info!("Shutting down.");

    exit(0);
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Tokio failed to register Ctrl-C handler.");
}
