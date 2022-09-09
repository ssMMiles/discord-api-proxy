use std::time::{SystemTime, UNIX_EPOCH};

use hyper::Method;

#[derive(Debug, PartialEq)]
pub enum Resources {
  Channels,
  Guilds,
  Webhooks,
  Invites,
  Interactions,
  None
}

impl Resources {
  pub fn from_str(s: &str) -> Self {
    match s {
      "channels" => Self::Channels,
      "guilds" => Self::Guilds,
      "webhooks" => Self::Webhooks,
      "invites" => Self::Invites,
      "interactions" => Self::Interactions,
      _ => Self::None,
    }
  }
}

// implement to string
impl ToString for Resources {
  fn to_string(&self) -> String {
    match self {
      Self::Channels => "channels".to_string(),
      Self::Guilds => "guilds".to_string(),
      Self::Webhooks => "webhooks".to_string(),
      Self::Invites => "invites".to_string(),
      Self::Interactions => "interactions".to_string(),
      Self::None => "".to_string(),
    }
  }
}

pub struct RouteInfo {
  pub resource: Resources,
  pub bucket: String,
}

pub fn get_route_info(bot_id: &u64, method: &Method, path: &str) -> RouteInfo {
  let route_segments = path.split("/").skip(3).collect::<Vec<&str>>();
  let mut bucket = String::from(format!("{}:{}/", method.as_str(), bot_id));

  let major_resource = Resources::from_str(route_segments[0]);
  let major_bucket = match major_resource {
    Resources::Invites => {
      "invites/!".to_string()
    },
    Resources::Channels => {
      if route_segments.len() == 2 {
        return RouteInfo { 
          resource: major_resource, 
          bucket: "channels/!".to_string() 
        };
      }

      format!("channels/{}", route_segments[1])
    },
    Resources::Guilds => {
      if route_segments.len() == 3 && route_segments[2] == "channels" {
        return RouteInfo { 
          resource: major_resource, 
          bucket: "guilds/!/channels".to_string() 
        };
      }

      format!("{}/{}", route_segments[0], route_segments[1])
    },
    _ => {
      format!("{}/{}", route_segments[0], route_segments[1])
    }
  };

  bucket.push_str(&major_bucket);

  if route_segments.len() == 2 {
    return RouteInfo { resource: major_resource, bucket }
  }

  for (index, segment) in route_segments[2..].iter().enumerate() {
    let i = index + 2;

    // Split messages into special buckets if they
    // are either under 10 seconds old, or over 14 days old
    if is_snowflake(segment) {
      if major_resource == Resources::Guilds 
      && method == Method::DELETE
      && route_segments[i - 1] == "messages"  {
        let snowflake = u64::from_str_radix(segment, 10).unwrap();
        let message_age_ms = get_snowflake_age_ms(snowflake);
        
        if message_age_ms > 14 * 24 * 60 * 60 * 1000 {
          bucket.push_str("/!14d");
          break;
        } else if message_age_ms < 10 {
          bucket.push_str("/!10s");
          break;
        }
        
        continue;
      }

      bucket.push_str("/!");
      continue;
    }

    // Split reactions into modify/query buckets
    if major_resource == Resources::Channels && *segment == "reactions" {
      if method == Method::PUT || method == Method::DELETE {
        bucket.push_str("/reactions/!modify");
        break
      }

      bucket.push_str("/reactions/!");
      break
    }

    // Strip webhook/interaction tokens
    if major_resource == Resources::Webhooks || major_resource == Resources::Interactions
      && segment.len() >= 64 {
        bucket.push_str("/!");
        continue
    }

    bucket.push_str(&format!("/{}", segment));
  }

  RouteInfo { resource: major_resource, bucket }
}

fn is_snowflake(s: &str) -> bool {
  let length = s.len();

  17 < length && length < 21 && s.chars().all(|c| c.is_numeric())
}

fn get_snowflake_age_ms(snowflake: u64) -> u64 {
  let timestamp = snowflake >> 22;
  let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;

  now - timestamp
}