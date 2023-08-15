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
    crate::metrics::{
        record_failed_request_metrics, record_overloaded_request_metrics,
        record_ratelimited_request_metrics, record_successful_request_metrics, register_metrics,
        REGISTRY,
    },
    prometheus::{Encoder, TextEncoder},
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

            config,
        })
    }

    pub fn get_metrics(&self) -> Response<Body> {
        #[cfg(feature = "metrics")]
        {
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

            return Response::new(Body::from(res));
        }

        #[cfg(not(feature = "metrics"))]
        return Response::new(Body::from("Metrics are disabled."));
    }

    pub async fn handle_request(&self, req: http::Request<Body>) -> Response<Body> {
        let res = match self.process(req).await {
            Ok(response) => response,
            Err(err) => match err {
                ProxyError::InvalidRequest(message) => responses::invalid_request(message),
                ProxyError::ProxiedRequestError(err) => {
                    tracing::error!("Proxied Request Failed: {:?}", err);
                    responses::internal_error()
                }
                _ => {
                    tracing::error!("Proxying Request Failed: {:?}", err);
                    responses::internal_error()
                }
            },
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

        // trace!(?request_info);

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

        trace!(?lock_token, "Sending request to Discord.");
        let response = self.http_client.request(req).await?;

        self.process_response(
            response.status(),
            response.headers(),
            &request_info,
            lock_token,
        )
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
            self.handle_429(headers).await;
        }

        self.update_ratelimits(headers, &request_info, lock_token)
            .await?;

        Ok(())
    }

    async fn handle_429(&self, headers: &HeaderMap) {
        let is_shared_ratelimit = headers
            .get("X-RateLimit-Scope")
            .map(|v| v == "shared")
            .unwrap_or(false);

        if is_shared_ratelimit {
            #[cfg(feature = "metrics")]
            record_ratelimited_request_metrics(
                crate::metrics::RatelimitType::Shared,
                id,
                &method,
                &route_info,
            );

            tracing::debug!("Discord returned Shared 429!");
        } else {
            let is_global = headers
                .get("X-RateLimit-Global")
                .map(|v| v == "true")
                .unwrap_or(false);

            #[cfg(feature = "metrics")]
            record_ratelimited_request_metrics(
                if is_global {
                    crate::metrics::RatelimitType::GlobalProxy
                } else {
                    crate::metrics::RatelimitType::RouteProxy
                },
                id,
                &method,
                &route_info,
            );

            // tracing::error!(
            //     "Discord returned 429! Global: {:?} Scope: {:?} - ABORTING REQUESTS FOR {}ms!",
            //     is_global,
            //     headers.get("X-RateLimit-Scope"),
            //     self.config.ratelimit_timeout.as_millis()
            // );

            // self.disabled.store(true, Ordering::Release);
            // tokio::time::sleep(self.config.ratelimit_timeout).await;
            // self.disabled.store(false, Ordering::Release);

            tracing::error!(
                "Discord returned 429! Global: {:?} Scope: {:?}",
                is_global,
                headers.get("X-RateLimit-Scope"),
            );
        }
    }
}
