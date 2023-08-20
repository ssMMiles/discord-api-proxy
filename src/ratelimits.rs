use core::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::response::Response;
use fred::prelude::RedisError;
use hyper::{Body, HeaderMap};
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use tokio::{select, time::Instant, try_join};
use tracing::{debug, error, trace, warn};

use crate::{
    buckets::Resources,
    proxy::{Proxy, ProxyError},
    request::DiscordRequestInfo,
    responses,
};

#[cfg(feature = "metrics")]
use crate::metrics;

#[derive(PartialEq, Debug)]
pub enum RatelimitRetryCause {
    AwaitingGlobalLock,
    AwaitingRouteLock,
    HoldingGlobalLockAwaitingRouteLock,
    GlobalRatelimitDrifted,
    ProxyOverloaded { retry_count: u8 },
}

#[derive(PartialEq, Debug)]
pub enum RatelimitStatus {
    ProxyOverloaded,
    RequiresRetry(RatelimitRetryCause),
    GlobalRatelimited {
        limit: u16,
        reset_at: u128,
        reset_after: u64,
    },
    RouteRatelimited {
        limit: u16,
        reset_at: u128,
        reset_after: u64,
    },
    Allowed {
        holds_global_lock: bool,
        holds_route_lock: bool,
    },
}

impl RatelimitStatus {
    pub fn from(
        overload_count: u8,
        check_started_at_timestamp: Duration,
        check_started_at: Instant,
        data: Vec<String>,
    ) -> Self {
        let check_time = check_started_at.elapsed().as_millis();

        let global_slice_reset_at = (check_started_at_timestamp.as_secs() + 1) as u128 * 1000;
        let curr_time = check_started_at_timestamp.as_millis() + check_time;

        if ratelimit_check_is_overloaded(check_time) {
            if overload_count == 3 {
                return RatelimitStatus::ProxyOverloaded;
            }

            return RatelimitStatus::RequiresRetry(RatelimitRetryCause::ProxyOverloaded {
                retry_count: overload_count + 1,
            });
        }

        if curr_time >= global_slice_reset_at {
            return RatelimitStatus::RequiresRetry(RatelimitRetryCause::GlobalRatelimitDrifted);
        }

        debug!(?data, "Ratelimit check response: {:#?}", data);

        let status_code = data[0].parse::<u8>().unwrap();
        match status_code {
            0 => {
                let reset_after = (global_slice_reset_at - curr_time) as u64;
                let limit = data[1].parse::<u16>().unwrap();

                RatelimitStatus::GlobalRatelimited {
                    limit,
                    reset_at: global_slice_reset_at,
                    reset_after,
                }
            }
            1 => RatelimitStatus::RequiresRetry(RatelimitRetryCause::AwaitingGlobalLock),
            2 => {
                let limit = data[1].parse::<u16>().unwrap();

                let reset_at = data[2].parse::<u128>().unwrap();
                let reset_after = match data[3].parse::<u64>() {
                    Ok(after) => after,
                    Err(_) => {
                        error!(data = ?data, "Failed to parse reset_after, defaulting to 0.",);

                        0
                    }
                };

                RatelimitStatus::RouteRatelimited {
                    limit,
                    reset_at,
                    reset_after,
                }
            }
            3 => RatelimitStatus::RequiresRetry(RatelimitRetryCause::AwaitingRouteLock),
            4 => RatelimitStatus::RequiresRetry(
                RatelimitRetryCause::HoldingGlobalLockAwaitingRouteLock,
            ),
            5 => {
                let holds_global_lock = data[1].as_str() == "1";
                let holds_route_lock = data[2].as_str() == "1";

                RatelimitStatus::Allowed {
                    holds_global_lock,
                    holds_route_lock,
                }
            }
            _ => panic!("Invalid ratelimit status code: {}", status_code),
        }
    }
}

impl fmt::Display for RatelimitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RatelimitStatus::ProxyOverloaded => write!(f, "Proxy Overloaded"),
            RatelimitStatus::RequiresRetry(cause) => write!(f, "Requires Retry - {:?}", cause),
            RatelimitStatus::GlobalRatelimited {
                limit,
                reset_at,
                reset_after,
            } => write!(
                f,
                "Global Ratelimited - Limit: {} - Resets At {} - Resource Available In {}ms",
                limit, reset_at, reset_after
            ),
            RatelimitStatus::RouteRatelimited {
                limit,
                reset_at,
                reset_after,
            } => write!(
                f,
                "Route Ratelimited - Limit: {} - Resets At {} - Resource Available In {}ms",
                limit, reset_at, reset_after
            ),
            RatelimitStatus::Allowed {
                holds_global_lock,
                holds_route_lock,
            } => {
                write!(
                    f,
                    "Allowed - Holds Global Lock: {}, Holds Route Lock: {}",
                    holds_global_lock, holds_route_lock
                )
            }
        }
    }
}

type RouteLockToken = Option<String>;
type RatelimitedResponse = Response<Body>;

impl Proxy {
    pub async fn check_ratelimits(
        &self,
        request_info: &DiscordRequestInfo,
    ) -> Result<Result<RouteLockToken, RatelimitedResponse>, ProxyError> {
        #[cfg(feature = "metrics")]
        let ratelimit_checks_started_at = Instant::now();

        let use_global_rl = !self.config.disable_global_rl && request_info.uses_global_ratelimit;

        let mut overload_count: u8 = 0;
        let result = loop {
            let check_started_at_timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards");
            let check_started_at = Instant::now();

            let global_rl_time_slice = &format!("-{}", check_started_at_timestamp.as_secs());
            let lock_token = random_string(8);

            let data = if use_global_rl {
                self.redis
                    .check_global_and_route_rl(
                        &request_info.global_id_redis_key,
                        global_rl_time_slice,
                        &request_info.route_bucket_redis_key,
                        &lock_token,
                    )
                    .await?
            } else {
                self.redis
                    .check_route_rl(&request_info.route_bucket_redis_key, &lock_token)
                    .await?
            };

            let status = RatelimitStatus::from(
                overload_count,
                check_started_at_timestamp,
                check_started_at,
                data,
            );

            trace!(?status);

            let result = match status {
                RatelimitStatus::ProxyOverloaded => {
                    #[cfg(feature = "metrics")]
                    metrics::PROXY_REQUEST_OVERLOADED
                        .with_label_values(&[
                            request_info.global_id.as_str(),
                            request_info.route_display_bucket.as_str(),
                        ])
                        .inc();

                    Ok(Err(responses::overloaded()))
                }
                RatelimitStatus::RequiresRetry(cause) => {
                    match cause {
                        RatelimitRetryCause::HoldingGlobalLockAwaitingRouteLock => {
                            try_join!(
                                self.fetch_global_ratelimit(request_info, &lock_token),
                                self.await_lock(&request_info.route_bucket_redis_key)
                            )?;
                        }
                        RatelimitRetryCause::AwaitingGlobalLock => {
                            self.await_lock(&request_info.global_id_redis_key).await?;
                        }
                        RatelimitRetryCause::AwaitingRouteLock => {
                            self.await_lock(&request_info.route_bucket_redis_key)
                                .await?;
                        }
                        RatelimitRetryCause::ProxyOverloaded { .. } => {
                            overload_count += 1;
                        }
                        RatelimitRetryCause::GlobalRatelimitDrifted => {
                            debug!("Global ratelimit drifted, retrying.");
                        }
                    }

                    continue;
                }
                RatelimitStatus::GlobalRatelimited {
                    limit,
                    reset_at,
                    reset_after,
                } => {
                    #[cfg(feature = "metrics")]
                    metrics::PROXY_REQUEST_GLOBAL_429
                        .with_label_values(&[request_info.global_id.as_str()])
                        .inc();

                    Ok(Err(responses::ratelimited(
                        &request_info.global_id,
                        limit,
                        reset_at,
                        reset_after,
                    )))
                }
                RatelimitStatus::RouteRatelimited {
                    limit,
                    reset_at,
                    reset_after,
                } => {
                    #[cfg(feature = "metrics")]
                    metrics::PROXY_REQUEST_ROUTE_429
                        .with_label_values(&[
                            request_info.global_id.as_str(),
                            request_info.route_display_bucket.as_str(),
                        ])
                        .inc();

                    Ok(Err(responses::ratelimited(
                        &request_info.route_bucket,
                        limit,
                        reset_at,
                        reset_after,
                    )))
                }
                RatelimitStatus::Allowed {
                    holds_global_lock,
                    holds_route_lock,
                } => {
                    if holds_global_lock {
                        self.fetch_global_ratelimit(request_info, &lock_token)
                            .await?;
                    }

                    let pass_lock_token = if holds_route_lock {
                        Some(lock_token)
                    } else {
                        None
                    };

                    Ok(Ok(pass_lock_token))
                }
            };

            break result;
        };

        #[cfg(feature = "metrics")]
        metrics::PROXY_REQUEST_RATELIMIT_CHECK_TIMES
            .with_label_values(&[
                request_info.global_id.as_str(),
                request_info.route_display_bucket.as_str(),
            ])
            .observe(ratelimit_checks_started_at.elapsed().as_secs_f64());

        result
    }

    async fn fetch_global_ratelimit(
        &self,
        request_info: &DiscordRequestInfo,
        lock_token: &str,
    ) -> Result<(), ProxyError> {
        let mut ratelimit = 50;

        if request_info.global_id == "NoAuth" {
            trace!("Global ratelimit lock acquired, but request is unauthenticated. Defaulting to 50 requests/s.");
        } else {
            ratelimit = match self
                .fetch_discord_global_ratelimit(request_info.token.as_ref().unwrap())
                .await
            {
                Ok(limit) => {
                    trace!("Fetched global ratelimit of {}/s from Discord.", limit);
                    limit
                }
                Err(err) => {
                    warn!("Failed to fetch global ratelimit from Discord, falling back to default 50/s. Error: {}", err);
                    50
                }
            }
        }

        if !self
            .redis
            .release_global_lock(
                &request_info.global_id_redis_key,
                lock_token,
                ratelimit,
                self.config.bucket_ttl_ms,
            )
            .await?
        {
            debug!("Lock expired before we could set the ratelimit.");
        }

        Ok(())
    }

    async fn await_lock(&self, bucket: &str) -> Result<(), ProxyError> {
        trace!("Waiting for lock on {}", bucket);

        select! {
          Ok(_) = self.redis.await_lock(bucket) => {
            trace!("Lock released.");
          },
          _ = tokio::time::sleep(self.config.lock_timeout) => {
            trace!("Lock wait expired.");
            self.redis.cleanup_pending_locks(bucket).await;
          }
        };

        Ok(())
    }

    pub async fn update_ratelimits(
        &self,
        headers: &HeaderMap,
        request_info: &DiscordRequestInfo,
        lock_token: Option<String>,
    ) -> Result<(), RedisError> {
        let headers: Option<(u16, u16, u64, u64)> = || -> Option<(u16, u16, u64, u64)> {
            let limit = match headers.get("X-RateLimit-Limit") {
                Some(limit) => limit.clone().to_str().unwrap().parse::<u16>().unwrap(),
                None => {
                    warn!("X-RateLimit-Limit header missing");
                    return None;
                }
            };

            let remaining = match headers.get("X-RateLimit-Remaining") {
                Some(remaining) => remaining.clone().to_str().unwrap().parse::<u16>().unwrap(),
                None => {
                    warn!("X-RateLimit-Remaining header missing");
                    return None;
                }
            };

            let reset_at = match headers.get("X-RateLimit-Reset") {
                Some(timestamp) => timestamp
                    .clone()
                    .to_str()
                    .unwrap()
                    .replace(".", "")
                    .parse::<u64>()
                    .unwrap(),
                None => {
                    warn!("X-RateLimit-Reset header missing");
                    return None;
                }
            };

            let reset_after = match headers.get("X-RateLimit-Reset-After") {
                Some(after) => after
                    .clone()
                    .to_str()
                    .unwrap()
                    .replace(".", "")
                    .parse::<u64>()
                    .unwrap(),
                None => {
                    warn!("X-RateLimit-Reset-After header missing");
                    return None;
                }
            };

            Some((limit, remaining, reset_at, reset_after))
        }();

        if headers.is_none() {
            return Ok(());
        }

        let (limit, remaining, reset_at, reset_after) = headers.unwrap();

        // Force 15 minute TTL for interaction routes
        let bucket_ttl = if request_info.resource == Resources::Interactions {
            15 * 60 * 1000
        } else {
            self.config.bucket_ttl_ms
        };

        let redis = self.redis.clone();
        let request_info_clone = request_info.clone();
        tokio::task::spawn(async move {
            if lock_token.is_some() {
                trace!(
                    "[{}] New Bucket/Leaky Bucket Allowance! Setting ratelimit to {}/s, resetting at {}",
                    &request_info_clone.route_bucket,
                    limit,
                    reset_at
                );
            }

            trace!(
                ?lock_token,
                ?limit,
                ?remaining,
                ?reset_at,
                ?reset_after,
                ?bucket_ttl,
                "Updating ratelimits: "
            );

            match redis
                .set_route_expiry(
                    &request_info_clone.route_bucket_redis_key,
                    lock_token.clone(),
                    limit,
                    remaining,
                    reset_at,
                    reset_after,
                    bucket_ttl,
                )
                .await
            {
                Ok(success) => {
                    if lock_token.is_some() && !success {
                        warn!(
                            lock_token,
                            "Lock expired before we could set the ratelimit for it: {}",
                            &request_info_clone.route_bucket
                        );
                    }
                }
                Err(err) => {
                    error!(
                        lock_token,
                        "Failed to update ratelimits for {}: {}",
                        &request_info_clone.route_bucket,
                        err
                    );
                }
            }
        });

        Ok(())
    }
}

fn ratelimit_check_is_overloaded(time_taken: u128) -> bool {
    if time_taken > 50 {
        warn!(
            "Ratelimit checks took {}ms to respond. Retrying.",
            time_taken
        );

        return true;
    }

    if time_taken > 25 {
        debug!("Ratelimit checks took {}ms to respond.", time_taken);
    }

    false
}

fn random_string(n: usize) -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(n)
        .map(char::from)
        .collect()
}
