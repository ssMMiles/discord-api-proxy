use std::ops::{DerefMut, Deref};
use hyper::{Request, Client, client::HttpConnector, Body, Response};
use hyper_tls::HttpsConnector;
use redis::RedisError;
use thiserror::Error;
use tokio::time::Instant;

use crate::{redis::RedisClient, global_rl::get_global_ratelimit_key};

#[derive(Clone)]
pub struct ProxyWrapper {
  pub proxy: DiscordProxy
}

impl ProxyWrapper {
  pub fn new(redis: &RedisClient, client: &Client<HttpsConnector<HttpConnector>>) -> Self {
    Self { 
      proxy: DiscordProxy { redis: redis.clone(), client: client.clone() }
    }
  }
}

impl Deref for ProxyWrapper {
  type Target = DiscordProxy;

  fn deref(&self) -> &Self::Target {
    &self.proxy
  }
}

impl DerefMut for ProxyWrapper {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.proxy
  }
}

#[derive(Clone)]
pub struct DiscordProxy {
  redis: RedisClient,
  client: Client<HttpsConnector<HttpConnector>>
}

impl DiscordProxy {
  pub async fn proxy_request(&mut self, bot_id: &u64, token: &str, req: Request<Body>) -> Result<Response<Body>, ProxyError> {
    let start = Instant::now();
    
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    if !self.redis.check_global_ratelimit(bot_id, token).await? {
      return Err(ProxyError::RateLimited());
    };

    let uri = format!("https://discord.com/api{}", path);

    let proxied_req = Request::builder()
      .header("User-Agent", "RockSolidRobots Discord Proxy/1.0")
      .uri(&uri)
      .method(&method)
      .header("Authorization", token)
      .body(req.into_body()).unwrap();


    let key = get_global_ratelimit_key(bot_id);

    println!("[{}] {}ms - {} https://discord.com/api{}", key, start.elapsed().as_millis(), method, path);
    let result = self.client.request(proxied_req).await.map_err(|e| ProxyError::RequestError(e))?;

    Ok(result)
  }
}

#[derive(Error, Debug)]
pub enum ProxyError {
  #[error("Redis Error: {0}")]
  RedisError(#[from] RedisError),

  #[error("You are being ratelimited.")]
  RateLimited(),

  #[error("Error proxying request: {0}")]
  RequestError(#[from] hyper::Error)
}
