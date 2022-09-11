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
  pub async fn new(host: &str, port: u16, pool_size: usize) -> Self {
    let addr = format!("redis://{}:{}", host, port);

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
    Script::new(r"
      local global_key = KEYS[1]
      local global_count_key = global_key .. ':count'
      
      local global_limit = tonumber(redis.call('GET', global_key))

      if global_limit == nil then
        return {false, false}
      end

      local global_count = tonumber(redis.call('INCR', global_count_key))

      if global_count >= global_limit then
        return {0, false}
      end

      local route_key = KEYS[2]
      local route_count_key = route_key .. ':count'
      
      local route_limit = tonumber(redis.call('GET', route_key))

      if route_limit == nil then
        return {global_count, false}
      end

      if route_limit == 0 then
        return {global_count, 2}
      end

      local route_count = tonumber(redis.call('INCR', route_count_key))

      if route_count >= route_limit then
        if tonumber(redis.call('DECR', global_count_key)) == 0 then
          redis.call('DEL', global_count_key)
        end

        return {1, 0}
      end

      return {global_count, route_count}
    ")
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
    Script::new(r"
      local bucket_key = KEYS[1]
      local bucket_count_key = bucket_key .. ':count'
      
      local bucket_limit = tonumber(redis.call('GET', bucket_key))

      if bucket_limit == nil then
        return false
      end

      if bucket_limit == 0 then
        return 2
      end

      local bucket_count = tonumber(redis.call('INCR', bucket_count_key))

      if bucket_count >= bucket_limit then
        return 0
      end

      return bucket_count
    ")
  }

  // Takes one Key:
  // - Bot ID/Bucket ID
  //
  // And one Argument:
  // - Random data to lock the bucket with.
  //
  // Returns true if we obtained the lock, false if not.
  pub fn lock_bucket() -> Script { 
    Script::new(r"
      local route_key = KEYS[1]
      local rl = redis.call('GET', route_key)

      if rl == false then
        local lock_val = ARGV[1]
        local lock = redis.call('SET', route_key .. ':lock', lock_val, 'NX', 'EX', '5')

        local got_lock = lock ~= false

        return got_lock
      end

      return false
    ")
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
    Script::new(r"
      local global_key = KEYS[1]
      local global_lock_key = global_key .. ':lock'

      local lock_val = ARGV[1]
      local global_lock = redis.call('GET', global_lock_key)

      if global_lock == lock_val then
        local global_limit = ARGV[2]

        redis.call('SET', global_key, global_limit)
        redis.call('DEL', global_lock_key)

        return true
      end

      return false
    ")
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
    Script::new(r"
      local global_count_key = KEYS[1] .. ':count'
      local global_expire_at = tonumber(ARGV[1])

      redis.call('PEXPIREAT', global_count_key, global_expire_at, 'NX')

      local route_key = KEYS[2]
      local route_lock_key = route_key .. ':lock'

      local lock_val = ARGV[2]
      local route_lock = redis.call('GET', route_lock_key)

      if route_lock == lock_val then
        local route_limit = ARGV[3]
        redis.call('SET', route_key, route_limit)

        local route_expire_at = tonumber(ARGV[4])
        if route_expire_at ~= 0 then
          local route_count_key = route_key .. ':count'

          redis.call('PEXPIREAT', route_count_key, route_expire_at)
        end

        redis.call('DEL', route_lock_key)
        return true
      end

      return false
    ")
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
    Script::new(r"
      local global_count_key = KEYS[1] .. ':count'
      local global_expire_at = tonumber(ARGV[1])

      redis.call('PEXPIREAT', global_count_key, global_expire_at, 'NX')

      local bucket_count_key = KEYS[2] .. ':count'
      local bucket_expire_at = tonumber(ARGV[2])

      redis.call('PEXPIREAT', bucket_count_key, bucket_expire_at, 'NX')
    ")
  }
}