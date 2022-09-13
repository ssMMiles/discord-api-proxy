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
  pub route: String,
}

pub fn get_route_bucket(bot_id: u64, method: &Method, route: &str) -> String {
  format!("{}:{}-{}", bot_id, method.to_string(), route)
}

pub fn get_route_info(method: &Method, path: &str) -> RouteInfo {
  let path_segments = path.split("/").skip(3).collect::<Vec<&str>>();
  let mut route = String::new();

  let major_resource = Resources::from_str(path_segments[0]);
  let major_bucket = match major_resource {
    Resources::Invites => {
      "invites/!".to_string()
    },
    Resources::Channels => {
      if path_segments.len() == 2 {
        route.push_str("channels/!");

        return RouteInfo {
          resource: major_resource, 
          route
        };
      }

      format!("channels/{}", path_segments[1])
    },
    Resources::Guilds => {
      if path_segments.len() == 3 && path_segments[2] == "channels" {
        route.push_str("guilds/!/channels");

        return RouteInfo { 
          resource: major_resource, 
          route
        };
      }

      if path_segments.len() >= 2 {
        format!("{}/{}", path_segments[0], path_segments[1])
      } else {
        path_segments[0].to_string()
      }
    },
    _ => {
      if path_segments.len() >= 2 {
        format!("{}/{}", path_segments[0], path_segments[1])
      } else {
        path_segments[0].to_string()
      }
    }
  };

  route.push_str(&major_bucket);

  if path_segments.len() <= 2 {
    return RouteInfo { 
      resource: major_resource, 
      route
     }
  }

  for (index, segment) in path_segments[2..].iter().enumerate() {
    let i = index + 2;

    // Split messages into special buckets if they
    // are either under 10 seconds old, or over 14 days old
    if is_snowflake(segment) {
      if major_resource == Resources::Guilds 
      && method == Method::DELETE
      && path_segments[i - 1] == "messages"  {
        let snowflake = u64::from_str_radix(segment, 10).unwrap();
        let message_age_ms = get_snowflake_age_ms(snowflake);
        
        if message_age_ms > 14 * 24 * 60 * 60 * 1000 {
          route.push_str("/!14d");
          break;
        } else if message_age_ms < 10 {
          route.push_str("/!10s");
          break;
        }
        
        continue;
      }

      route.push_str("/!");
      continue;
    }

    // Split reactions into modify/query buckets
    if major_resource == Resources::Channels && *segment == "reactions" {
      if method == Method::PUT || method == Method::DELETE {
        route.push_str("/reactions/!modify");
        break
      }

      route.push_str("/reactions/!");
      break
    }

    // Strip webhook/interaction tokens
    if major_resource == Resources::Webhooks || major_resource == Resources::Interactions
      && segment.len() >= 64 {
        route.push_str("/!");
        continue
    }

    route.push_str(&format!("/{}", segment));
  }

  RouteInfo {
    resource: major_resource, 
    route
  }
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