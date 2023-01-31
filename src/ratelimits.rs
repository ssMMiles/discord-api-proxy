use std::time::{SystemTime, UNIX_EPOCH};

use deadpool_redis::Connection;
use futures_util::{future::join, try_join};
use hyper::HeaderMap;
use rand::{thread_rng, distributions::Alphanumeric, Rng};
use redis::AsyncCommands;
use tokio::{time::Instant, select};

use crate::{proxy::{DiscordProxy, ProxyError}, buckets::{Resources, RouteInfo}, redis::RedisErrorWrapper, NewBucketStrategy};

#[derive(PartialEq, Debug)]
pub enum RatelimitStatus {
  Ok(Option<String>),
  GlobalRatelimited,
  RouteRatelimited,
  ProxyOverloaded
}

impl DiscordProxy {
  pub async fn check_ratelimits(&mut self, id: &str, token: &Option<&str>, route: &RouteInfo, route_bucket: &str) -> Result<RatelimitStatus, ProxyError> {  
    let use_global_rl = match route.resource {
      Resources::Webhooks => false,
      Resources::Interactions => route.route != "interactions/!*/!/callback",
      _ => true
    };
    
    log::debug!("[{}] Using Global Ratelimit : {}", route_bucket, use_global_rl);

    if use_global_rl {
      Ok(self.check_global_and_bucket_ratelimits(id, token, route_bucket).await?)
    } else {
      Ok(self.check_route_ratelimit(route_bucket).await?)
    }
  }

  async fn check_global_and_bucket_ratelimits(&mut self, id: &str, token: &Option<&str>, route_bucket: &str) -> Result<RatelimitStatus, ProxyError> {
    let epoch = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .expect("Time went backwards");

    let global_id = &format!("global-{{{}}}", id);

    let global_rl_time_slice = epoch.as_millis() / 1000 + (self.config.global_time_slice_offset_ms as u128);
    let global_rl_slice_id = &format!("{}-{}", global_id, global_rl_time_slice);

    let (mut redis_conn_1, mut redis_conn_2): (Connection, Connection) = try_join!(self.redis.pool.get(), self.redis.pool.get())
      .map_err(RedisErrorWrapper::RedisPoolError)?;

    let mut retries = 0;
    let status = loop {
      let ratelimit_check_started_at = Instant::now();

      let mut check_global_rl = self.redis.check_global_rl();
      check_global_rl.key(global_id).key(global_rl_slice_id);

      let mut check_route_rl = self.redis.check_route_rl();
      check_route_rl.key(route_bucket);

      let ratelimits = try_join!(
        check_global_rl.invoke_async::<_, Option<u16>>(&mut redis_conn_1), 
        check_route_rl.invoke_async::<_, Option<u16>>(&mut redis_conn_2)
      ).expect("Failed to check global or route bucket.");

      // let ratelimits = RedisClient::check_global_and_route_ratelimits()
      //   .key(id)
      //   .key(route_bucket)
      // .invoke_async::<_,Vec<Option<u16>>>(&mut redis_conn).await
      // .map_err(RedisErrorWrapper::RedisError)?;

      let global_ratelimit = ratelimits.0;
      let route_ratelimit = ratelimits.1;

      let is_overloaded = ratelimit_check_is_overloaded(route_bucket, ratelimit_check_started_at);
      if is_overloaded {
        retries += 1;

        if retries == 3 {
          log::error!("Ratelimit check is overloaded 3 times in a row, returning proxy overloaded.");

          break RatelimitStatus::ProxyOverloaded;
        }
      }

      let is_global_ratelimited = self.is_global_ratelimited(&mut redis_conn_1, global_id, token, global_ratelimit);
      let is_route_ratelimited = self.is_route_ratelimited(&mut redis_conn_2, route_bucket, route_ratelimit);

      let is_ratelimited = join(
        is_global_ratelimited,
        is_route_ratelimited
      ).await;

      match is_ratelimited.0? {
        Some(status) => {
          if status != RatelimitStatus::Ok(None) {
            break status
          }
        },
        None => continue
      }

      match is_ratelimited.1? {
        Some(status) => {
          if status != RatelimitStatus::Ok(None) {
            drop(redis_conn_2);

            redis_conn_1.decr::<&str, u8, bool>(global_rl_slice_id, 1).await.map_err(RedisErrorWrapper::RedisError)?;
          }

          break status
        },
        None => continue
      }
    };

    log::debug!("[{}] Ratelimit Status: {:?}", route_bucket, status);

    Ok(status)
  }

  async fn check_route_ratelimit(&mut self, route_bucket: &str) -> Result<RatelimitStatus, ProxyError> {
    let mut redis_conn = self.redis.pool.get().await.unwrap();

    let mut retries = 0;
    return loop {
      let ratelimit_check_started_at = Instant::now();

      let ratelimit = self.redis.check_route_rl()
        .key(route_bucket)
      .invoke_async::<_,Option<u16>>(&mut redis_conn).await.map_err(RedisErrorWrapper::RedisError)?;

      let is_overloaded = ratelimit_check_is_overloaded(route_bucket, ratelimit_check_started_at);
      if is_overloaded {
        retries += 1;

        if retries == 3 {
          log::error!("Ratelimit check is overloaded 3 times in a row, returning proxy overloaded.");

          break Ok(RatelimitStatus::ProxyOverloaded);
        }
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

  async fn is_global_ratelimited(&self, redis_conn: &mut Connection, global_id: &str, token: &Option<&str>, ratelimit: Option<u16>) -> Result<Option<RatelimitStatus>, ProxyError> {
    match ratelimit {
      Some(count) => {
        let hit_ratelimit = count == 0;

        if hit_ratelimit {
          return Ok(Some(RatelimitStatus::GlobalRatelimited));
        }
      },
      None => {
        log::debug!("[{}] Global ratelimit not set, will try to acquire a lock and set it...", global_id);

        let lock_value = random_string(8);
        let lock = self.redis.lock_bucket()
          .key(global_id)
            .arg(&lock_value)
        .invoke_async::<_, bool>(redis_conn).await
        .map_err(RedisErrorWrapper::RedisError)?;

        let mut ratelimit = 50;
        if lock {
          if global_id == "global-{0}" || token.is_none() {
            log::debug!("[{}] Global ratelimit lock acquired, but request is unauthenticated. Defaulting to 50 requests/s.", &global_id);
          } else {
            log::debug!("[{}] Global ratelimit lock acquired, fetching from Discord.", &global_id);
            ratelimit = match DiscordProxy::fetch_discord_global_ratelimit(self.client.clone(), token.unwrap()).await {
              Ok(limit) => {
                log::debug!("[{}] Global ratelimit fetched from Discord: {}", &global_id, &limit);
                limit
              },
              Err(e) => {
                log::debug!("[{}] Failed to fetch global ratelimit from Discord, defaulting to 50: {}", &global_id, &e);
                50
              }
            }
          }

          if self.redis.unlock_global()
            .key(global_id)
              .arg(&lock_value)
              .arg(ratelimit)
          .invoke_async::<_, bool>(redis_conn).await
          .map_err(RedisErrorWrapper::RedisError)? {
            log::debug!("[{}] Global ratelimit set to {} and lock released.", &global_id, &ratelimit);
          } else {
            log::debug!("[{}] Global ratelimit lock expired before we could release it.", &global_id);
          }

          return Ok(None);
        } else {
          if self.config.global == NewBucketStrategy::Strict {
            log::debug!("[{}]  Lock is taken and ratelimit config is Strict, awaiting unlock.", &global_id);
            
            select! {
              Ok(_) = self.redis.await_lock(global_id) => {
                log::debug!("[{}] Unlock received, continuing.", &global_id);

                return Ok(None);
              },
              _ = tokio::time::sleep(self.config.lock_timeout) => {
                log::debug!("[{}] Lock wait expired, retrying.", &global_id);
              }
            };

            log::debug!("[{}] Failed to obtain unlock from PubSub, cleaning up.", &global_id);
            self.redis.cleanup_pending_locks(global_id).await;

            return Ok(None);
          }

          log::debug!("[{}] Lock is taken and ratelimit config is Loose, skipping ratelimit check.", &global_id);
        }
      }
    };

    Ok(Some(RatelimitStatus::Ok(None)))
  }

  async fn is_route_ratelimited(&self, redis_conn: &mut Connection, route_bucket: &str, ratelimit: Option<u16>) -> Result<Option<RatelimitStatus>, ProxyError> {
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
        let lock = self.redis.lock_bucket()
          .key(route_bucket)
            .arg(&lock_value)
        .invoke_async::<_, bool>(redis_conn).await.map_err(RedisErrorWrapper::RedisError)?;

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

  pub async fn update_ratelimits(&mut self, global_rl_key: String, headers: &HeaderMap, bucket: String, bucket_lock: Option<String>, sent_request_at: u128) -> Result<(), RedisErrorWrapper> {
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

    let sent_at = sent_request_at as u64 + 500;

    let redis = self.redis.clone();
    let (mut redis_conn_1, mut redis_conn_2): (Connection, Connection) = try_join!(redis.pool.get(), redis.pool.get())?;

    tokio::task::spawn(async move {
      if bucket_lock.is_some() {
        log::debug!("[{}] New bucket! Setting ratelimit to {}, resetting at {}", bucket, bucket_limit, reset_at);

        // let mut expire_global = redis.expire_global();
        // expire_global.key(&global_rl_key)
        //   .arg(&sent_at);
        
        let mut expire_route = redis.unlock_route();
        expire_route.key(&bucket)
          .arg(&bucket_lock.unwrap())
          .arg(&bucket_limit)
          .arg(&reset_at);

        // try_join!(
        //   expire_global.invoke_async::<_, bool>(&mut redis_conn_1), 
        //   expire_route.invoke_async::<_, bool>(&mut redis_conn_2)
        // ).expect("Failed to expire global or unlock route bucket.");

        expire_route.invoke_async::<_, bool>(&mut redis_conn_1).await
          .expect("Failed to update route expiry.");

        // RedisClient::expire_global()
        //   .key(&global_rl_key)
        //     .arg(&(sent_request_at as u64 + 1000))
        // .invoke_async::<_, bool>(&mut redis_conn).await
        // .expect("Failed to update global ratelimit expiry time.");

        // RedisClient::unlock_route_bucket()
        //   .key(&bucket)
        //     .arg(&bucket_lock.unwrap())
        //     .arg(&bucket_limit)
        //     .arg(&reset_at)
        // .invoke_async::<_, bool>(&mut redis_conn).await
        // .expect("Failed to unlock route bucket and update its expiry times.");

        // RedisClient::expire_global_and_unlock_route_buckets()
        //   .key(&global_rl_key)
        //     .arg(&(sent_request_at as u64 + 1000))
        //   .key(bucket)
        //     .arg(&bucket_lock.unwrap())
        //     .arg(&bucket_limit)
        //     .arg(&reset_at)
        // .invoke_async::<_, bool>(&mut redis_conn).await
        // .expect("Failed to unlock route bucket and update expiry times.");
      } else {
        let mut expire_global = redis.expire_global();
        expire_global.key(&global_rl_key).arg(&sent_at);

        let mut expire_route_bucket = redis.expire_route();
        expire_route_bucket.key(&bucket).arg(&reset_at);

        try_join!(
          expire_global.invoke_async::<_, bool>(&mut redis_conn_1), 
          expire_route_bucket.invoke_async::<_, bool>(&mut redis_conn_2)
        ).expect("Failed to expire global or route bucket.");

        // RedisClient::expire_global()
        //   .key(&global_rl_key)
        //     .arg(&(sent_request_at as u64 + 1000))
        // .invoke_async::<_, bool>(&mut redis_conn).await
        // .expect("Failed to update global ratelimit expiry time.");

        // RedisClient::expire_route_bucket()
        //   .key(&bucket)
        //     .arg(&reset_at)
        // .invoke_async::<_, bool>(&mut redis_conn).await
        // .expect("Failed to update bucket expiry time.");

        // RedisClient::expire_global_and_route_buckets()
        //   .key(&global_rl_key)
        //     .arg(&(sent_request_at as u64 + 1000))
        //   .key(bucket)
        //     .arg(&reset_at)
        // .invoke_async::<_, bool>(&mut redis_conn).await
        // .expect("Failed to update bucket expiry times.");
      }
    });

    Ok(())
  }
}

fn ratelimit_check_is_overloaded(route_bucket: &str, started_at: Instant) -> bool {
  let time_taken = started_at.elapsed().as_millis();

  if time_taken > 50 {
    log::warn!("[{}] Ratelimit checks took {}ms to respond. Redis or the proxy is overloaded, request aborted.", route_bucket, time_taken);
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