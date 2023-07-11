use hyper::{body::Buf, Body, Request, StatusCode};
use serde::Deserialize;
use thiserror::Error;

use crate::proxy::Proxy;

const DEFAULT: u16 = 50;

const LARGE_SHARDING_MINIMUM: u16 = 500;
const LARGE_SHARDING_INTERNAL_SHARD_RL: u16 = 25;

#[derive(Deserialize)]
struct GetGatewayBotResponse {
    // url: String,
    // shards: u16,
    session_start_limit: SessionStartLimit,
}

#[derive(Deserialize)]
struct SessionStartLimit {
    // total: u16,
    // remaining: u16,
    // reset_after: u64,
    max_concurrency: u16,
}

#[derive(Error, Debug)]
pub enum DiscordError {
    #[error("Non 2xx Status Code fetching Global Ratelimit: {0}")]
    DiscordError(StatusCode),

    #[error("HTTP Error fetching global ratelimit: {0}")]
    RequestError(#[from] hyper::Error),

    #[error("Error proxying request: {0}")]
    ParseError(#[from] serde_json::Error),
}

const GET_GATEWAY_URL: &'static str = "https://discord.com/api/v10/gateway/bot";

impl Proxy {
    pub async fn fetch_discord_global_ratelimit(&self, token: &str) -> Result<u16, DiscordError> {
        println!("CHECKING DISCORD RL");

        let req = Request::builder()
            .method("GET")
            .uri(GET_GATEWAY_URL)
            .header("Authorization", token)
            .body(Body::empty())
            .unwrap();

        let result = self.http_client.request(req).await?;

        if !result.status().is_success() {
            return Err(DiscordError::DiscordError(result.status()));
        }

        let body = hyper::body::aggregate(result).await?;

        let gateway_bot: GetGatewayBotResponse = serde_json::from_reader(body.reader())?;

        let global_ratelimit = if gateway_bot.session_start_limit.max_concurrency > 1 {
            let allowed_for_concurrency = gateway_bot.session_start_limit.max_concurrency as u16
                * LARGE_SHARDING_INTERNAL_SHARD_RL;

            if allowed_for_concurrency > LARGE_SHARDING_MINIMUM {
                allowed_for_concurrency
            } else {
                LARGE_SHARDING_MINIMUM
            }
        } else {
            DEFAULT
        };

        Ok(global_ratelimit)
    }
}
