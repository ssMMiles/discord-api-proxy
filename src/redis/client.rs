use deadpool_redis::{Config, Runtime, Pool};
use redis::Script;

use super::scripts::{check_bucket_ratelimit_script, check_global_and_bucket_ratelimit_script, lock_ratelimit_script, unlock_ratelimit_script};

#[derive(Clone)]
pub struct RedisClient {
  pub pool: Pool,
  pub check_bucket_ratelimit_script: Script,
  pub check_global_and_bucket_ratelimit_script: Script,
  pub lock_ratelimit_script: Script,
  pub unlock_ratelimit_script: Script
}

impl RedisClient {
  pub async fn new(host: &str, port: u16) -> Self {
    let addr = format!("redis://{}:{}", host, port);

    let cfg_builder = Config::builder(&Config::from_url(addr)).unwrap();
    let pool = cfg_builder.max_size(64).runtime(Runtime::Tokio1).build().unwrap();

    Self {
      pool,
      check_bucket_ratelimit_script: check_bucket_ratelimit_script(),
      check_global_and_bucket_ratelimit_script: check_global_and_bucket_ratelimit_script(),
      lock_ratelimit_script: lock_ratelimit_script(),
      unlock_ratelimit_script: unlock_ratelimit_script()
    }
  }
}