use std::net::SocketAddr;
use hyper::{Client, service::{service_fn}, server::conn::Http};
use hyper_tls::HttpsConnector;
use tokio::net::TcpListener;

use crate::{proxy::ProxyWrapper, routes::discord_proxy, redis::RedisClient};

mod redis;

mod routes;
mod proxy;

mod global_rl;

const HOST: [u8; 4] = [127, 0, 0, 1];
const PORT: u16 = 8080;

const REDIS_HOST: &str = "127.0.0.1";
const REDIS_PORT: u16 = 6379;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let storage = RedisClient::new(REDIS_HOST, REDIS_PORT).await;

  let https = HttpsConnector::new();
  let https_client = Client::builder()
    .build::<_, hyper::Body>(https);

  let proxy = ProxyWrapper::new(&storage, &https_client);

  let addr = SocketAddr::from((HOST, PORT));
  let listener = TcpListener::bind(addr).await?;
  
  println!("Starting Discord API Proxy on http://{}", addr);
  
  loop {
    let (stream, _) = listener.accept().await?;
    let proxy = proxy.clone();

    let service = service_fn(move |req| {
      discord_proxy(req, proxy.clone())
    });

    if let Err(err) = Http::new().serve_connection(stream, service).await {
      eprintln!("Error serving connection: {:?}", err);
    }
  }
}