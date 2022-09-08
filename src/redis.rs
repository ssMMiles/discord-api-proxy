use std::time::Duration;
use redis::{Client, aio::ConnectionManager, AsyncCommands, RedisError, Script};
use tokio::time::sleep;
use rand::{thread_rng, Rng};
use rand::distributions::Alphanumeric;

use crate::global_rl::{get_global_ratelimit_key, fetch_discord_global_ratelimit};

#[derive(Clone)]
pub struct RedisClient {
  redis: ConnectionManager,
  check_ratelimit_script: Script,
  lock_ratelimit_script: Script,
  unlock_ratelimit_script: Script
}

impl RedisClient {
  pub async fn new(host: &str, port: u16) -> Self {
    let client = Client::open(format!("redis://{}:{}", host, port)).unwrap();
    let connection = loop {
        match client.get_tokio_connection_manager().await {
            Ok(mngr) => break mngr,
            Err(e) => println!("Error connecting to Redis: {}", e),
        }

        sleep(Duration::from_secs(5)).await;
    };

    let check_ratelimit_script = Script::new(r"
      local rl_key = KEYS[1]
      local rl_count_key = rl_key .. ':count'
      
      local rl_res = redis.call('MGET', rl_key, rl_count_key)

      if rl_res[1] == false then
        return nil
      end

      local rl = tonumber(rl_res[1])
      local rl_count = tonumber(rl_res[2])

      if rl_count == nil then
        rl_count = 0
      end

      if rl_count >= rl then
        return 0
      end

      local count = redis.call('INCR', rl_count_key)

      if count == 1 then
        redis.call('EXPIRE', rl_count_key, 1)
      end

      return count
    ");

    let lock_ratelimit_script = Script::new(r"
      local rl_key = KEYS[1]
      local rl = redis.call('GET', rl_key)

      if rl == false then
        local lock_val = ARGV[1]
        local lock = redis.call('SET', rl_key .. ':lock', lock_val, 'NX', 'EX', '5')

        local got_lock = lock ~= false

        return got_lock
      end

      return false
    ");

    let unlock_ratelimit_script = Script::new(r"
      local rl_key = KEYS[1]
      local rl_lock_key = rl_key .. ':lock'

      local lock_val = ARGV[1]
      local rl_lock = redis.call('GET', rl_lock_key)

      if rl_lock == lock_val then
        redis.call('DEL', rl_lock_key)
        return true
      end

      return false
    ");

    Self {
      redis: connection,
      check_ratelimit_script,
      lock_ratelimit_script,
      unlock_ratelimit_script
    }
  }

  pub async fn check_global_ratelimit(&mut self, bot_id: &u64, token: &str) -> Result<bool, RedisError> {
    let key = get_global_ratelimit_key(bot_id);
    return loop {
      let bucket_count = self.check_ratelimit_script.key(&key).invoke_async::<_, Option<u16>>(&mut self.redis).await?;

      match bucket_count {
        Some(count) => {
          println!("[{}] Passed Global Ratelimit - Bucket Count: {}", &key, count);

          break Ok(count != 0)
        },
        None => {
          println!("[{}] Ratelimit not set, will try to acquire a lock and set it...", &key);

          let lock_value = random_string(8);
          let lock = self.lock_ratelimit_script.key(&key).arg(&lock_value).invoke_async::<_, bool>(&mut self.redis).await?;

          if lock {
            println!("[{}] Lock acquired, setting ratelimit.", &key);

            let ratelimit = fetch_discord_global_ratelimit(token).await?;
            self.redis.set(&key, ratelimit).await?;

            println!("[{}] Ratelimit set to {}.", &key, ratelimit);

            match self.unlock_ratelimit_script.key(&key).arg(&lock_value).invoke_async::<_, bool>(&mut self.redis).await? {
              true => {
                println!("[{}] Lock released.", &key);
              },
              false => {
                println!("[{}] Lock expired before we could release it.", &key);
              }
            }

            break(Ok(true))
          } else {
            println!("Lock is taken, checking ratelimit again in 100ms.");
            sleep(Duration::from_millis(100)).await;
            
            continue;
          }
        }
      };
    }
  }
}

fn random_string(n: usize) -> String {
  thread_rng().sample_iter(&Alphanumeric)
    .take(n)
    .map(char::from)
    .collect()
}