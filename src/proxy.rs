use fred::prelude::RedisError;
use http::{
    header::{CONNECTION, TRANSFER_ENCODING, UPGRADE},
    HeaderMap,
};
use hyper::{
    client::{connect::dns::GaiResolver, HttpConnector},
    http::HeaderValue,
    Body, Client, Response, StatusCode, Uri,
};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use std::{
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use thiserror::Error;
use tracing::{trace, trace_span};

use crate::{
    config::{ProxyEnvConfig, RedisEnvConfig},
    discord::DiscordError,
    redis::ProxyRedisClient,
    request::DiscordRequestInfo,
    responses,
};

#[cfg(feature = "metrics")]
use {
    crate::metrics,
    std::{sync::atomic::AtomicU64, time::Instant},
};

#[derive(Error, Debug)]
pub enum ProxyError {
    #[error("Redis Error: {0}")]
    RedisError(#[from] RedisError),

    #[error("Retrieving Global Ratelimit Info Failed: {0}")]
    GlobalRatelimitInfoUnavailable(#[from] DiscordError),

    #[error("Invalid Route: {0}")]
    InvalidRequest(String),

    #[error("Proxied Request Failed: {0}")]
    ProxiedRequestError(#[from] hyper::Error),
}

#[derive(Clone)]
pub struct Proxy {
    disabled: Arc<AtomicBool>,

    pub redis: Arc<ProxyRedisClient>,
    pub http_client: Client<HttpsConnector<HttpConnector<GaiResolver>>, Body>,

    #[cfg(feature = "metrics")]
    pub metrics_last_reset_at: Arc<AtomicU64>,

    pub config: Arc<ProxyEnvConfig>,
}

impl Proxy {
    pub async fn new(
        config: Arc<ProxyEnvConfig>,
        redis_config: Arc<RedisEnvConfig>,
    ) -> Result<Self, RedisError> {
        let redis_client = ProxyRedisClient::new(redis_config).await?;

        let mut http_connector = HttpConnector::new();
        http_connector.enforce_http(false);

        let builder = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_only()
            .enable_http1();

        let builder = if !config.disable_http2 {
            builder.enable_http2().wrap_connector(http_connector)
        } else {
            builder.wrap_connector(http_connector)
        };

        Ok(Self {
            disabled: Arc::new(AtomicBool::new(false)),

            redis: Arc::new(redis_client),
            http_client: Client::builder().build(builder),

            #[cfg(feature = "metrics")]
            metrics_last_reset_at: Arc::new(AtomicU64::new(0)),

            config,
        })
    }

    pub async fn handle_request(&self, req: http::Request<Body>) -> Response<Body> {
        let res = match self.process(req).await {
            Ok(response) => response,
            Err(err) => {
                #[cfg(feature = "metrics")]
                metrics::PROXY_REQUEST_ERRORS.inc();

                match err {
                    ProxyError::InvalidRequest(message) => responses::invalid_request(message),
                    ProxyError::ProxiedRequestError(err) => {
                        tracing::error!("Proxied Request Failed: {:?}", err);
                        responses::internal_error()
                    }
                    _ => {
                        tracing::error!("Proxying Request Failed: {:?}", err);
                        responses::internal_error()
                    }
                }
            }
        };

        return res;
    }

    async fn process(&self, mut req: http::Request<Body>) -> Result<Response<Body>, ProxyError> {
        let span = trace_span!("process_request");
        let _guard = span.enter();

        let method = req.method().clone();
        let path = req.uri().path();
        let headers = req.headers();

        let request_info = DiscordRequestInfo::new(&method, path, headers)?;

        #[cfg(feature = "metrics")]
        metrics::PROXY_REQUEST_COUNTER
            .with_label_values(&[
                request_info.global_id.as_str(),
                request_info.route_display_bucket.as_str(),
            ])
            .inc();

        drop(_guard);

        let lock_token = match self.check_ratelimits(&request_info).await? {
            Ok(lock_token) => lock_token,
            Err(response) => {
                return Ok(response);
            }
        };

        let headers = req.headers_mut();

        headers.insert("Host", HeaderValue::from_static("discord.com"));
        headers.insert(
            "User-Agent",
            HeaderValue::from_static("limbo-labs/discord-api-proxy/1.2"),
        );

        // Remove HTTP2 headers
        headers.remove(CONNECTION);
        headers.remove("keep-alive");
        headers.remove("proxy-connection");
        headers.remove(TRANSFER_ENCODING);
        headers.remove(UPGRADE);

        let path_and_query = match req.uri().path_and_query() {
            Some(path_and_query) => path_and_query.as_str(),
            None => "/",
        };

        *req.uri_mut() = Uri::from_str(&format!("https://discord.com{}", path_and_query))
            .expect("Failed to rebuild URI.");

        if self.disabled.load(Ordering::Acquire) {
            return Ok(responses::overloaded());
        }

        #[cfg(feature = "metrics")]
        metrics::DISCORD_REQUEST_COUNTER
            .with_label_values(&[
                request_info.global_id.as_str(),
                request_info.route_display_bucket.as_str(),
            ])
            .inc();

        trace!(?lock_token, "Sending request to Discord.");

        #[cfg(feature = "metrics")]
        let discord_request_sent_at = Instant::now();

        let response = self.http_client.request(req).await?;

        let status = response.status();

        #[cfg(feature = "metrics")]
        metrics::DISCORD_REQUEST_RESPONSE_TIMES
            .with_label_values(&[
                request_info.global_id.as_str(),
                request_info.route_display_bucket.as_str(),
                status.as_str(),
            ])
            .observe(discord_request_sent_at.elapsed().as_secs_f64());

        self.process_response(status, response.headers(), &request_info, lock_token)
            .await?;

        Ok(response)
    }

    async fn process_response(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
        request_info: &DiscordRequestInfo,
        lock_token: Option<String>,
    ) -> Result<(), ProxyError> {
        if status == StatusCode::TOO_MANY_REQUESTS {
            self.handle_429(request_info, headers).await;
        }

        self.update_ratelimits(headers, &request_info, lock_token)
            .await?;

        Ok(())
    }

    async fn handle_429(&self, _request_info: &DiscordRequestInfo, headers: &HeaderMap) {
        let is_shared_ratelimit = headers
            .get("X-RateLimit-Scope")
            .map(|v| v == "shared")
            .unwrap_or(false);

        if is_shared_ratelimit {
            #[cfg(feature = "metrics")]
            metrics::DISCORD_REQUEST_SHARED_429
                .with_label_values(&[
                    _request_info.global_id.as_str(),
                    _request_info.route_display_bucket.as_str(),
                ])
                .inc();

            tracing::debug!("Discord returned Shared 429!");
        } else {
            let is_global = headers
                .get("X-RateLimit-Global")
                .map(|v| v == "true")
                .unwrap_or(false);

            #[cfg(feature = "metrics")]
            if is_global {
                metrics::DISCORD_REQUEST_GLOBAL_429
                    .with_label_values(&[_request_info.global_id.as_str()])
                    .inc();
            } else {
                metrics::DISCORD_REQUEST_ROUTE_429
                    .with_label_values(&[
                        _request_info.global_id.as_str(),
                        _request_info.route_display_bucket.as_str(),
                    ])
                    .inc();
            }

            tracing::warn!(
                "Discord returned 429! Global: {:?} Scope: {:?}",
                is_global,
                headers.get("X-RateLimit-Scope"),
            );
        }
    }
}
