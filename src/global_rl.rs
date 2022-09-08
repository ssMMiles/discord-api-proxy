use redis::RedisError;
use twilight_http::Client;

const DEFAULT: u16 = 50;

const LARGE_SHARDING_MINIMUM: u16 = 500;
const LARGE_SHARDING_INTERNAL_SHARD_RL: u16 = 25;

pub async fn fetch_discord_global_ratelimit(token: &str) -> Result<u16, RedisError> {
  let client = Client::new(token.to_string());

  let app_info_response = match client.gateway().authed().exec().await {
    Ok(app_info_response) => app_info_response,
    Err(e) => {
      println!("Error while fetching app info: {}", e);
      return Ok(DEFAULT);
    },
  };

  let app_info = app_info_response.model().await.unwrap();

  let global_ratelimit = if app_info.session_start_limit.max_concurrency > 1 {
    let allowed_for_concurrency = app_info.session_start_limit.max_concurrency as u16 * LARGE_SHARDING_INTERNAL_SHARD_RL;

    if allowed_for_concurrency > LARGE_SHARDING_MINIMUM {
      allowed_for_concurrency
    } else {
      LARGE_SHARDING_MINIMUM
    }
  } else {
    DEFAULT
  };

  Ok(global_ratelimit)
}

pub fn get_global_ratelimit_key(bot_id: &u64) -> String {
  format!("global_ratelimit_{}", bot_id)
}