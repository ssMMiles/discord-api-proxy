use deadpool_redis::Connection;
use hyper::HeaderMap;
use rand::{thread_rng, distributions::Alphanumeric, Rng};
use tokio::{time::Instant, select};

use crate::{proxy::{DiscordProxy, ProxyError}, buckets::{Resources, RouteInfo}, redis::{RedisClient, RedisErrorWrapper}, NewBucketStrategy};

#[derive(PartialEq, Debug)]
pub enum RatelimitStatus {
  Ok(Option<String>),
  GlobalRatelimited,
  RouteRatelimited,
  ProxyOverloaded
}

impl DiscordProxy {
  pub async fn check_ratelimits(&mut self, bot_id: &u64, token: &str, route: &RouteInfo, route_bucket: &str) -> Result<RatelimitStatus, ProxyError> {  
    let use_global_rl = match route.resource {
      Resources::Webhooks => false,
      Resources::Interactions => route.route != "interactions/!*/!/callback",
      _ => true
    };
    
    log::debug!("[{}] Using Global Ratelimit : {}", route_bucket, use_global_rl);

    if use_global_rl {
      Ok(self.check_global_and_bucket_ratelimits(&bot_id, &token, route_bucket).await?)
    } else {
      Ok(self.check_route_ratelimit(route_bucket).await?)
    }
  }

  async fn check_global_and_bucket_ratelimits(&mut self, bot_id: &u64, token: &str, route_bucket: &str) -> Result<RatelimitStatus, ProxyError> {
    let mut redis_conn = self.redis.pool.get().await
      .map_err(RedisErrorWrapper::RedisPoolError)?;

    let id = bot_id.to_string();

    let status = loop {
      let ratelimit_check_started_at = Instant::now();

      let ratelimits = RedisClient::check_global_and_route_ratelimits()
        .key(&bot_id)
        .key(route_bucket)
      .invoke_async::<_,Vec<Option<u16>>>(&mut redis_conn).await
      .map_err(RedisErrorWrapper::RedisError)?;

      let global_ratelimit = ratelimits[0];
      let route_ratelimit = ratelimits[1];

      if ratelimit_check_is_overloaded(route_bucket, ratelimit_check_started_at) {
        break RatelimitStatus::ProxyOverloaded;
      }

      match self.is_global_ratelimited(&mut redis_conn, &id, token, global_ratelimit).await? {
        Some(status) => {
          if status != RatelimitStatus::Ok(None) {
            break status
          }
        },
        None => continue
      }

      match self.is_route_ratelimited(&mut redis_conn, route_bucket, route_ratelimit).await? {
        Some(status) => break status,
        None => continue
      }
    };

    log::debug!("[{}] Ratelimit Status: {:?}", route_bucket, status);

    Ok(status)
  }

  async fn check_route_ratelimit(&mut self, route_bucket: &str) -> Result<RatelimitStatus, RedisErrorWrapper> {
    let mut redis_conn = self.redis.pool.get().await.unwrap();

    return loop {
      let ratelimit_check_started_at = Instant::now();

      let ratelimit = RedisClient::check_route_ratelimit()
        .key(route_bucket)
      .invoke_async::<_,Option<u16>>(&mut redis_conn).await?;

      if ratelimit_check_is_overloaded(route_bucket, ratelimit_check_started_at) {
        break Ok(RatelimitStatus::ProxyOverloaded);
      }

      match self.is_route_ratelimited(&mut redis_conn, route_bucket, ratelimit).await? {
        Some(status) => {
          log::debug!("[{}] Bucket Ratelimit Status: {:?} - Count: {}", &route_bucket, &status, &ratelimit.unwrap_or(0));
          break Ok(status)
        },
        None => continue
      }
    };
  }

  async fn is_global_ratelimited(&mut self, redis_conn: &mut Connection, id: &str, token: &str, ratelimit: Option<u16>) -> Result<Option<RatelimitStatus>, ProxyError> {
    match ratelimit {
      Some(count) => {
        let hit_ratelimit = count == 0;

        if hit_ratelimit {
          return Ok(Some(RatelimitStatus::GlobalRatelimited));
        }
      },
      None => {
        log::debug!("[{}] Ratelimit not set, will try to acquire a lock and set it...", &id);

        let lock_value = random_string(8);
        let lock = RedisClient::lock_bucket()
          .key(id)
            .arg(&lock_value)
        .invoke_async::<_, bool>(redis_conn).await
        .map_err(RedisErrorWrapper::RedisError)?;

        if lock {
          log::debug!("[{}] Global ratelimit lock acquired, fetching from Discord.", &id);
          let ratelimit = self.fetch_discord_global_ratelimit(token).await?;

          if RedisClient::unlock_global_bucket()
            .key(id)
              .arg(&lock_value)
              .arg(ratelimit)
          .invoke_async::<_, bool>(redis_conn).await
          .map_err(RedisErrorWrapper::RedisError)? {
            log::debug!("[{}] Global ratelimit set to {} and lock released.", &id, &ratelimit);
          } else {
            log::debug!("[{}] Global ratelimit lock expired before we could release it.", &id);
          }

          return Ok(None);
        } else {
          if self.config.global == NewBucketStrategy::Strict {
            log::debug!("[{}]  Lock is taken and ratelimit config is Strict, awaiting unlock.", &id);
            
            select! {
              Ok(_) = self.redis.await_lock(id) => {
                log::debug!("[{}] Unlock received, continuing.", &id);

                return Ok(None);
              },
              _ = tokio::time::sleep(self.config.lock_timeout) => {
                log::debug!("[{}] Lock wait expired, retrying.", &id);
              }
            };

            log::debug!("[{}] Failed to obtain unlock from PubSub, cleaning up.", &id);
            self.redis.cleanup_pending_locks(id).await;

            return Ok(None);
          }

          log::debug!("[{}] Lock is taken and ratelimit config is Loose, skipping ratelimit check.", &id);
        }
      }
    };

    Ok(Some(RatelimitStatus::Ok(None)))
  }

  async fn is_route_ratelimited(&self, redis_conn: &mut Connection, route_bucket: &str, ratelimit: Option<u16>) -> Result<Option<RatelimitStatus>, RedisErrorWrapper> {
    match ratelimit {
      Some(count) => {
        let hit_ratelimit = count == 0;

        if hit_ratelimit {
          return Ok(Some(RatelimitStatus::RouteRatelimited))
        }
      },
      None => {
        log::debug!("[{}] Ratelimit not set, will try to acquire a lock and set it...", &route_bucket);

        let lock_value = random_string(8);
        let lock = RedisClient::lock_bucket()
          .key(route_bucket)
            .arg(&lock_value)
        .invoke_async::<_, bool>(redis_conn).await?;

        if lock {
          log::debug!("[{}] Acquired bucket lock.", route_bucket);

          return Ok(Some(RatelimitStatus::Ok(Some(lock_value))))
        } else {
          if self.config.buckets == NewBucketStrategy::Strict {
            log::debug!("[{}]  Lock is taken, awaiting unlock.", &route_bucket);
            
            select! {
              Ok(_) = self.redis.await_lock(route_bucket) => {
                log::debug!("[{}] Unlock received, continuing.", &route_bucket);
              },
              _ = tokio::time::sleep(self.config.lock_timeout) => {
                log::debug!("[{}] Lock wait expired, retrying.", &route_bucket);
              }
            };

            log::debug!("[{}] Failed to obtain unlock from PubSub, cleaning up.", &route_bucket);
            self.redis.cleanup_pending_locks(route_bucket).await;

            return Ok(None);
          }

          log::debug!("[{}]  Lock is taken, skipping ratelimit check.", &route_bucket);
        }
      }
    };

    Ok(Some(RatelimitStatus::Ok(None)))
  }

  pub async fn update_ratelimits(&mut self, bot_id: u64, headers: &HeaderMap, bucket: String, bucket_lock: Option<String>, sent_request_at: u128) -> Result<(), RedisErrorWrapper> {
    let bucket_limit = match headers.get("X-RateLimit-Limit") {
      Some(limit) => limit.clone().to_str().unwrap().parse::<u16>().unwrap(),
      None => 0
    };

    let reset_at = match headers.get("X-RateLimit-Reset") {
      Some(timestamp) => {
        timestamp.clone()
          .to_str().unwrap()
          .replace(".", "")
          .to_string()
        },
      None => "0".to_string()
    };

    let redis = self.redis.clone();
    let mut redis_conn = redis.pool.get().await?;

    tokio::task::spawn(async move {
      if bucket_lock.is_some() {
        log::debug!("[{}] New bucket! Setting ratelimit to {}, resetting at {}", bucket, bucket_limit, reset_at);

        RedisClient::expire_global_and_unlock_route_buckets()
          .key(&bot_id)
            .arg(&(sent_request_at as u64 + 1000))
          .key(bucket)
            .arg(&bucket_lock.unwrap())
            .arg(&bucket_limit)
            .arg(&reset_at)
        .invoke_async::<_, bool>(&mut redis_conn).await
        .expect("Failed to unlock route bucket and update expiry times.");
      } else {
        tokio::task::spawn(async move {
          RedisClient::expire_global_and_route_buckets()
            .key(&bot_id)
              .arg(&(sent_request_at as u64 + 1000))
            .key(bucket)
              .arg(&reset_at)
          .invoke_async::<_, bool>(&mut redis_conn).await
          .expect("Failed to update bucket expiry times.");
        });
      }
    });

    Ok(())
  }
}

fn ratelimit_check_is_overloaded(route_bucket: &str, started_at: Instant) -> bool {
  let time_taken = started_at.elapsed().as_millis();

  if time_taken > 50 {
    log::error!("[{}] Ratelimit checks took {}ms to respond. Redis or the proxy is overloaded, request aborted.", route_bucket, time_taken);
    return true;
  } else if time_taken > 25 {
    log::warn!("[{}] Ratelimit checks took {}ms to respond. Redis or the proxy is getting overloaded.", route_bucket, time_taken);
  } else {
    log::debug!("[{}] Redis took {}ms to respond.", route_bucket, time_taken);
  }

  false
}

fn random_string(n: usize) -> String {
  thread_rng().sample_iter(&Alphanumeric)
    .take(n)
    .map(char::from)
    .collect()
}