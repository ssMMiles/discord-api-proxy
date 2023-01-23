use std::{sync::{Arc}, collections::HashMap};

use deadpool_redis::{Config, Runtime, Pool, PoolError, Connection};
use futures_util::StreamExt;
use redis::{Script, RedisError, aio::PubSub, ScriptInvocation};
use thiserror::Error;
use tokio::{sync::{oneshot::{self, error::RecvError}, Mutex, RwLock}};

#[derive(Clone)]
pub struct RedisClient {
  pub pool: Pool,
  
  pubsub_channels: Arc<RwLock<HashMap<String, Arc<PubSubChannel>>>>,

  pub check_global_rl: Script,
  pub check_route_rl: Script,

  pub expire_global: Script,
  pub expire_route: Script,

  pub lock_bucket: Script,

  pub unlock_global: Script,
  pub unlock_route: Script,
}

pub struct PubSubChannel {
  pending_clients: Arc<Mutex<Vec<oneshot::Sender<()>>>>
}

#[derive(Error, Debug)]
pub enum RedisErrorWrapper {
  #[error("Redis Error: {0}")]
  RedisError(#[from] RedisError),

  #[error("Redis Pool Error: {0}")]
  RedisPoolError(#[from] PoolError),
}

#[derive(Error, Debug)]
pub enum LockError {
  #[error("Error awaiting lock: {0}")]
  RecvError(#[from] RecvError),
}

const PUBSUB_INITIAL_RECONNECT_TIMEOUT: u64 = 5000;
const PUBSUB_MAX_RECONNECT_TIMEOUT: u64 = 60000;

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

    let instance = Self {
      pool,

      pubsub_channels: Arc::new(RwLock::new(HashMap::new())),

      check_global_rl: Script::new(include_str!("./scripts/check_global_rl.lua")),
      check_route_rl: Script::new(include_str!("./scripts/check_route_rl.lua")),

      expire_global: Script::new(include_str!("./scripts/expire_global.lua")),
      expire_route: Script::new(include_str!("./scripts/expire_route.lua")),

      lock_bucket: Script::new(include_str!("./scripts/lock_bucket.lua")),

      unlock_global: Script::new(include_str!("./scripts/unlock_global.lua")),
      unlock_route: Script::new(include_str!("./scripts/unlock_route.lua")),
    };
    
    let pubsub_instance = instance.clone();
    
    tokio::spawn(async move {
      pubsub_instance.start_pubsub_task().await;
    });

    instance
  }

  async fn start_pubsub_task(&self) -> () {
    async fn open_connection(pool: Pool) -> Result<PubSub, RedisErrorWrapper> {
      let pubsub = Connection::take(pool.get().await?).into_pubsub();

      Ok(pubsub)
    }

    loop {
      let mut retry_count = 0;
      let mut pubsub = loop {
        match open_connection(self.pool.clone()).await {
          Ok(pubsub) => break pubsub,
          Err(e) => {
            log::error!("Error opening PubSub connection: {}", e);

            let timeout = PUBSUB_INITIAL_RECONNECT_TIMEOUT * 2_u64.pow(retry_count);
            let timeout = if timeout > PUBSUB_MAX_RECONNECT_TIMEOUT {
              PUBSUB_MAX_RECONNECT_TIMEOUT
            } else {
              timeout
            };

            tokio::time::sleep(std::time::Duration::from_millis(timeout)).await;

            retry_count += 1;
          }
        }
      };

      if pubsub.subscribe("unlock").await.is_ok() {
        log::info!("Subscribed to unlock channel");
      } else {
        log::error!("Failed to subscribe to unlock channel");
        continue;
      }

      loop {
        let msg = pubsub.on_message().next().await;

        if msg.is_none() {
          log::debug!("Received empty PubSub message.");
          break;
        }

        let payload = msg.unwrap().get_payload::<String>().unwrap();
        log::debug!("Received unlock over PubSub for {}.", &payload);

        self.release_lock(&payload).await;
      }

      pubsub.into_connection().await;
    }
  }

  pub async fn await_lock(&self, key: &str) -> Result<(), LockError> {
    let (tx, rx) = oneshot::channel::<()>();

    let pubsub_channels_r = self.pubsub_channels.read().await;

    async fn push_pending_client(channel: Arc<PubSubChannel>, tx: oneshot::Sender<()>) {
      let mut pending_clients = channel.pending_clients.lock().await;
      pending_clients.push(tx);

      drop(pending_clients)
    }
  
    match pubsub_channels_r.get(key) {
      Some(channel) => {
        let channel = channel.clone();
        
        push_pending_client(channel, tx).await;
        drop(pubsub_channels_r);
      },
      None => {
        drop(pubsub_channels_r);

        let mut pubsub_channels_w = self.pubsub_channels.write().await;

        if let Some(channel) = pubsub_channels_w.get(key) {
          log::debug!("Another thread subscribed to channel for key {} while this thread was waiting for the write lock, pushing to queue.", key);

          push_pending_client(channel.clone(), tx).await;
          drop(pubsub_channels_w);
        } else {
          // self.send_pubsub_command(key.to_string(), true).await?;

          pubsub_channels_w.insert(key.to_string(), Arc::new(PubSubChannel {
            pending_clients: Arc::new(Mutex::new(vec![tx])),
          }));

          drop(pubsub_channels_w);
        }
      }
    };

    rx.await?;
    Ok(())
  }

  pub async fn cleanup_pending_locks(&self, key: &str) {
    let pubsub_channels_r = self.pubsub_channels.read().await;

    let mut emptied = false;
    if let Some(channel) = pubsub_channels_r.get(key) {
      let pending_clients_m = channel.pending_clients.clone();
      drop(pubsub_channels_r);

      let mut pending_clients = pending_clients_m.lock().await;

      while let Some(index) = pending_clients.iter().position(|tx| tx.is_closed()) {
        pending_clients.remove(index);
      }

      if pending_clients.len() == 0 {
        emptied = true;
      }
      
      drop(pending_clients);
    }

    if emptied {
      let mut pubsub_channels_w = self.pubsub_channels.write().await;

      let channel = match pubsub_channels_w.get(key) {
        Some(channel_m) => channel_m,
        None => return,
      };

      let pending_clients = channel.pending_clients.lock().await;

      let pending_client_len = pending_clients.len();
      drop(pending_clients);

      if pending_client_len == 0 {
        pubsub_channels_w.remove(key);
      }

      drop(pubsub_channels_w);
    }
  }

  async fn release_lock(&self, key: &str) {
    let mut pubsub_channels_w = self.pubsub_channels.write().await;

    let channel = match pubsub_channels_w.get(key) {
      Some(channel_m) => channel_m.clone(),
      None => return,
    };

    // match self.send_pubsub_command(key.to_string(), false).await {
    //   Ok(_) => (),
    //   Err(e) => {
    //     log::error!("Error unsubscribing from PubSub channel {} after unlock. This will not resolve itself: {}", key, e);
    //   }
    // };

    pubsub_channels_w.remove(key);
    drop(pubsub_channels_w);

    let mut pending_clients = channel.pending_clients.lock().await;
    for tx in pending_clients.drain(..) {
      match tx.send(()){
        Ok(_) => (),
        Err(e) => log::error!("Error completing a pending lock on {}: {:?}", key, e),
      }
    }
    drop(pending_clients);

    drop(channel);
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
  // pub fn check_global_and_route_ratelimits(&self) -> ScriptInvocation {
  //   Script::new(include_str!("scripts/check_global_and_route_rl.lua"))
  // }

  pub fn check_global_rl(&self) -> ScriptInvocation {
    self.check_global_rl.prepare_invoke()
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
  pub fn check_route_rl(&self) -> ScriptInvocation {
    self.check_route_rl.prepare_invoke()
  }

  // Takes one Key:
  // - Bot ID/Bucket ID
  //
  // And one Argument:
  // - Random data to lock the bucket with.
  //
  // Returns true if we obtained the lock, false if not.
  pub fn lock_bucket(&self) -> ScriptInvocation { 
    self.lock_bucket.prepare_invoke()
  }

  // Takes one Key:
  // - Bot ID
  //
  // And two Arguments:
  // - Lock value to check against.
  // - Global ratelimit
  //
  // Returns true if we unlocked the global bucket, false if we were too slow.
  pub fn unlock_global(&self) -> ScriptInvocation {
    self.unlock_global.prepare_invoke()
  }

  // Takes one Key:
  // - Bucket ID
  //
  // And three Arguments:
  // - Lock value to check against.
  // - Bucket ratelimit
  // - Bucket ratelimit expiration time (in ms)
  pub fn unlock_route(&self) -> ScriptInvocation {
    self.unlock_route.prepare_invoke()
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
  // pub fn expire_global_and_unlock_route_buckets(&self) -> ScriptInvocation {
  //   Script::new(include_str!("scripts/expire_global_and_unlock_route_buckets.lua"))
  // }

  // Takes one key:
  // - Bot ID
  //
  // And one argument:
  // - Global ratelimit expiration time (in ms)
  pub fn expire_global(&self) -> ScriptInvocation {
    self.expire_global.prepare_invoke()
  }

  // Takes one key:
  // - Bucket ID
  //
  // And one argument:
  // - Bucket ratelimit expiration time (in ms)
  pub fn expire_route(&self) -> ScriptInvocation {
    self.expire_route.prepare_invoke()
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
  // pub fn expire_global_and_route_buckets(&self) -> ScriptInvocation {
  //   Script::new(include_str!("scripts/expire_global_and_route_buckets.lua"))
  // }
}