use std::{sync::Arc, time::Duration};

use ahash::AHashMap;
use fred::{pool::RedisPool, prelude::{RedisError, PubsubInterface, LuaInterface, ClientLike}, clients::SubscriberClient,  types::{RedisConfig, ReconnectPolicy, PerformanceConfig, RedisValue, RespVersion, ServerConfig, Server}, util::sha1_hash};
use thiserror::Error;
use tokio::{sync::{oneshot::{self, error::RecvError}, Mutex, RwLock}, time::sleep, select};

use crate::config::RedisEnvConfig;

struct StaticProxyScripts {
  pub check_global_and_route_rl: &'static str,
  pub check_global_rl: &'static str,
  pub check_route_rl: &'static str,

  // pub expire_global: &'static str,
  pub expire_route: &'static str,

  pub lock_bucket: &'static str,

  pub unlock_global: &'static str,
  pub unlock_route: &'static str,
}

static SCRIPTS: StaticProxyScripts = StaticProxyScripts {
  check_global_and_route_rl: include_str!("./scripts/check_global_and_route_rl.lua"),
  check_global_rl: include_str!("./scripts/check_global_rl.lua"),
  check_route_rl: include_str!("./scripts/check_route_rl.lua"),

  // expire_global: include_str!("./scripts/expire_global.lua"),
  expire_route: include_str!("./scripts/expire_route.lua"),

  lock_bucket: include_str!("./scripts/lock_bucket.lua"),

  unlock_global: include_str!("./scripts/unlock_global.lua"),
  unlock_route: include_str!("./scripts/unlock_route.lua"),
};

struct ProxyScriptHashes {
  pub check_global_and_route_rl: String,
  pub check_global_rl: String,
  pub check_route_rl: String,

  // pub expire_global: String,
  pub expire_route: String,

  pub lock_bucket: String,

  pub unlock_global: String,
  pub unlock_route: String,
}

impl ProxyScriptHashes {
  pub fn new() -> Self {
    Self {
      check_global_and_route_rl: sha1_hash(&SCRIPTS.check_global_and_route_rl),
      check_global_rl: sha1_hash(&SCRIPTS.check_global_rl),
      check_route_rl: sha1_hash(&SCRIPTS.check_route_rl),

      // expire_global: sha1_hash(&SCRIPTS.expire_global),
      expire_route: sha1_hash(&SCRIPTS.expire_route),

      lock_bucket: sha1_hash(&SCRIPTS.lock_bucket),

      unlock_global: sha1_hash(&SCRIPTS.unlock_global),
      unlock_route: sha1_hash(&SCRIPTS.unlock_route),
    }
  }
}

#[derive(Clone)]
pub struct ProxyRedisClient {
  pub pool: RedisPool,
  
  pubsub_receiver: SubscriberClient,
  pubsub_channels: Arc<RwLock<AHashMap<String, Arc<PubSubChannel>>>>,

  script_hashes: Arc<ProxyScriptHashes>,  
}

pub struct PubSubChannel {
  pending_clients: Arc<Mutex<Vec<oneshot::Sender<()>>>>
}

#[derive(Error, Debug)]
pub enum RedisErrorWrapper {
  #[error("Redis Error: {0}")]
  RedisError(#[from] RedisError),
}

#[derive(Error, Debug)]
pub enum LockError {
  #[error("Error awaiting lock: {0}")]
  RecvError(#[from] RecvError),
}

// const PUBSUB_INITIAL_RECONNECT_TIMEOUT: u64 = 5000;
// const PUBSUB_MAX_RECONNECT_TIMEOUT: u64 = 60000;

impl ProxyRedisClient {
  pub async fn new(env_config: Arc<RedisEnvConfig>) -> Result<Self, RedisError> {
    let server_config = if env_config.sentinel {
      let (sentinel_user, sentinel_pass) = if env_config.sentinel_auth { 
        (env_config.username.clone(), env_config.password.clone())
      } else { 
        (None, None)
      };

      ServerConfig::Sentinel {
        hosts: vec![Server {
          host: env_config.host.clone().into(),
          port: env_config.port,
          tls_server_name: None,
        }],
        service_name: env_config.sentinel_master.clone(),

        username: sentinel_user,
        password: sentinel_pass,
      }
    } else {
      ServerConfig::Centralized { 
        server: Server {
          host: env_config.host.clone().into(),
          port: env_config.port,
          tls_server_name: None,
        }
      }
    };
    
    let config = RedisConfig {
      server: server_config,

      username: env_config.username.clone(),
      password: env_config.password.clone(),

      version: RespVersion::RESP3,
      
      ..RedisConfig::default()
    };

    let policy = ReconnectPolicy::default();
    let perf = PerformanceConfig::default();

    let pool = RedisPool::new(
      config.clone(), 
      Some(perf.clone()), 
      Some(policy.clone()), 
      env_config.pool_size
    )?;
    
    let pubsub_receiver = SubscriberClient::new(config, Some(perf), Some(policy));

    let instance = Self {
      pool,

      pubsub_receiver,
      pubsub_channels: Arc::new(RwLock::new(AHashMap::new())),

      script_hashes: Arc::new(ProxyScriptHashes::new()),
    };

    instance.pool.connect();
    instance.pool.wait_for_connect().await?;

    instance.pubsub_receiver.connect();
    instance.pubsub_receiver.wait_for_connect().await?;
    
    let mut reconnect_stream = instance.pool.on_reconnect();
    let reconnect_instance = instance.clone();
    tokio::spawn(async move {
      while let Ok(_) = reconnect_stream.recv().await {
        println!("Pool reconnected to Redis.");

        match reconnect_instance.register_scripts().await {
          Ok(_) => log::debug!("Scripts reloaded."),
          Err(e) => log::error!("Error reloading scripts: {}", e),
        }
      }

      Ok::<_, RedisError>(())
    });

    instance.register_scripts().await?;
    
    let pubsub_instance = instance.clone();
    tokio::spawn(async move {
      pubsub_instance.start_pubsub_task().await;
    });

    Ok(instance)
  }

  async fn register_scripts(&self) -> Result<(), RedisError> {
    self.pool.script_load::<(), &str>(SCRIPTS.check_global_and_route_rl).await?;
    self.pool.script_load::<(), &str>(SCRIPTS.check_global_rl).await?;
    self.pool.script_load::<(), &str>(SCRIPTS.check_route_rl).await?;

    // self.pool.script_load::<(), &str>(SCRIPTS.expire_global).await?;
    self.pool.script_load::<(), &str>(SCRIPTS.expire_route).await?;

    self.pool.script_load::<(), &str>(SCRIPTS.lock_bucket).await?;

    self.pool.script_load::<(), &str>(SCRIPTS.unlock_global).await?;
    self.pool.script_load::<(), &str>(SCRIPTS.unlock_route).await?;

    Ok(())
  }

  async fn start_pubsub_task(&self) -> () {
    let _self = self.clone();

    log::debug!("Starting PubSub task...");

    let mut message_stream = _self.pubsub_receiver.on_message();
    let message_task = tokio::spawn(async move {
      while let Ok(message) = message_stream.recv().await {
        match message.value {
          RedisValue::String(payload) => {
            log::debug!("Received unlock over PubSub for {}.", &payload);

            _self.release_lock(&payload).await;
          },
          _ => log::warn!("Received unexpected message type over unlock channel."),
        }
      }

      Ok::<_, RedisError>(())
    });

    let manage_subscription_task = self.pubsub_receiver.manage_subscriptions();

    log::debug!("Subscribing to unlock channel...");

    loop {
      match self.pubsub_receiver.subscribe::<(), &str>("unlock").await {
        Ok(_) => {
          log::info!("Subscribed to unlock channel");

          break;
        },
        Err(e) => {
          log::error!("Failed to subscribe to unlock channel. Retrying in 5 seconds: {:?}", e);
          sleep(Duration::from_secs(5)).await;

          continue;
        }
      }
    }

    drop(self);

    select! {
      _ = message_task => {
        log::error!("PubSub message receiver task exited unexpectedly.");
      },
      _ = manage_subscription_task => {
        log::error!("PubSub subscription manager task exited unexpectedly.");
      },
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

  pub async fn check_global_and_route_rl(&self, bot_id: &str, global_rl_key: &str, route_rl_key: &str) -> Result<(Option<u16>, Option<u16>), RedisError> {
    self.pool.evalsha::<(Option<u16>, Option<u16>), &str, Vec<&str>, _>(
      &self.script_hashes.check_global_and_route_rl,
      vec!(bot_id, global_rl_key, route_rl_key),
      None
    ).await
  }

  pub async fn check_global_rl(&self, bot_id: &str, global_rl_key: &str) -> Result<Option<u16>, RedisError> {
    self.pool.evalsha::<Option<u16>, &str, Vec<&str>, _>(
      &self.script_hashes.check_global_rl,
      vec!(bot_id, global_rl_key),
      None
    ).await
  }

  pub async fn check_route_rl(&self, route_rl_key: &str) -> Result<Option<u16>, RedisError> {
    self.pool.evalsha::<Option<u16>, &str, &str, _>(
      &self.script_hashes.check_route_rl,
      route_rl_key,
      None
    ).await
  }

  pub async fn lock_bucket(&self, bucket_key: &str, lock_id: &str) -> Result<bool, RedisError> {
    self.pool.evalsha::<Option<bool>, &str, &str, &str>(
      &self.script_hashes.lock_bucket,
      bucket_key,
      lock_id
    ).await.map(|r| r.unwrap_or(false))
  }

  pub async fn unlock_global(&self, bot_id: &str, lock_id: &str, global_rl: u16) -> Result<bool, RedisError> {
    self.pool.evalsha::<Option<bool>, &str, &str, Vec<&str>>(
      &self.script_hashes.unlock_global,
      bot_id,
      vec!(lock_id, &global_rl.to_string()),
    ).await.map(|r| r.unwrap_or(false))
  }

  pub async fn unlock_route(&self, route_rl_key: &str, lock_id: &str, route_rl: u16, reset_at: &str) -> Result<bool, RedisError> {
    self.pool.evalsha::<Option<bool>, &str, &str, Vec<&str>>(
      &self.script_hashes.unlock_route,
      route_rl_key,
      vec!(lock_id, &route_rl.to_string(), reset_at),
    ).await.map(|r| r.unwrap_or(false))
  }

  pub async fn expire_route(&self, route_rl_key: &str, reset_at: &str) -> Result<bool, RedisError> {
    self.pool.evalsha::<Option<bool>, &str, &str, Vec<&str>>(
      &self.script_hashes.expire_route,
      route_rl_key,
      vec!(reset_at),
    ).await.map(|r| r.unwrap_or(false))
  }

}