use deadpool_redis::{Config, Runtime, Pool, PoolError};
use redis::{Script, RedisError};
use thiserror::Error;

#[derive(Clone)]
pub struct RedisClient {
  pub pool: Pool,
}

#[derive(Error, Debug)]
pub enum RedisErrorWrapper {
  #[error("Redis Error: {0}")]
  RedisError(#[from] RedisError),

  #[error("Redis Pool Error: {0}")]
  RedisPoolError(#[from] PoolError),
}

impl RedisClient {
  pub async fn new(host: String, port: u16, user: String, pass: String, pool_size: usize) -> Self {
    let addr = if pass == "" {
      format!("redis://{}:{}", host, port)
    } else {
      format!("redis://{}:{}@{}:{}", user, pass, host, port)
    };

    let cfg_builder = Config::builder(&Config::from_url(addr)).unwrap();
    let pool = cfg_builder
      .max_size(pool_size)
      .runtime(Runtime::Tokio1)
      .build().unwrap();

    Self {
      pool
    }
  }

  // Returned ratelimit status can be:
  // - False/Nil: Ratelimit not found, must be fetched.
  // - 0: Ratelimit exceeded.
  // - 1-Infinity: Ratelimit OK, is number of requests in current bucket.
  // 
  // Takes two Keys: 
  // - Bot ID
  // - Bucket ID
  //
  // Returns a list containing status of both global and bucket ratelimits.
  // - [global_ratelimit_status, bucket_ratelimit_status]
  pub fn check_global_and_route_ratelimits() -> Script {
    Script::new(include_str!("scripts/check_global_and_route_rl.lua"))
  }

  // Returned ratelimit status can be:
  // - False/Nil: Ratelimit not found, must be fetched.
  // - 0: Ratelimit exceeded.
  // - 1-Infinity: Ratelimit OK, is number of requests in current bucket.
  // 
  // Takes one Key: 
  // - Bucket ID
  //
  // Returns the bucket ratelimit status.
  // - bucket_ratelimit_status
  pub fn check_route_ratelimit() -> Script {
    Script::new(include_str!("scripts/check_route_rl.lua"))
  }

  // Takes one Key:
  // - Bot ID/Bucket ID
  //
  // And one Argument:
  // - Random data to lock the bucket with.
  //
  // Returns true if we obtained the lock, false if not.
  pub fn lock_bucket() -> Script { 
    Script::new(include_str!("scripts/lock_bucket.lua"))
  }

  // Takes one Key:
  // - Bot ID
  //
  // And two Arguments:
  // - Lock value to check against.
  // - Global ratelimit
  //
  // Returns true if we unlocked the global bucket, false if we were too slow.
  pub fn unlock_global_bucket() -> Script {
    Script::new(include_str!("scripts/unlock_global_bucket.lua"))
  }

  // Takes two Keys:
  // - Bot ID
  // - Bucket ID
  //
  // And four Arguments:
  // - Global ratelimit expiration time (in ms)
  // - Lock value to check against.
  // - Route bucket limit
  // - Route bucket expiration time (in ms)
  //
  // Returns true if we unlocked the route bucket, false if we were too slow.
  pub fn expire_global_and_unlock_route_buckets() -> Script {
    Script::new(include_str!("scripts/expire_global_and_unlock_route_buckets.lua"))
  }

  // Takes two Keys:
  // - Bot ID
  // - Bucket ID
  //
  // And two Arguments:
  // - Global ratelimit expiration time (in ms)
  // - Bucket ratelimit expiration time (in ms)
  //
  // Returns nothing.
  pub fn expire_global_and_route_buckets() -> Script {
    Script::new(include_str!("scripts/expire_global_and_route_buckets.lua"))
  }
}