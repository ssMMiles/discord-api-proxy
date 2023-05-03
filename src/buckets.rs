use std::time::{SystemTime, UNIX_EPOCH};
use base64_simd::forgiving_decode_to_vec;
use hyper::Method;

#[derive(Debug, PartialEq)]
pub enum Resources {
  Channels,
  Guilds,
  Webhooks,
  Invites,
  Interactions,
  OAuth2,
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
      "oauth2" => Self::OAuth2,
      _ => Self::None,
    }
  }
}

impl ToString for Resources {
  fn to_string(&self) -> String {
    match self {
      Self::Channels => "channels".to_string(),
      Self::Guilds => "guilds".to_string(),
      Self::Webhooks => "webhooks".to_string(),
      Self::Invites => "invites".to_string(),
      Self::Interactions => "interactions".to_string(),
      Self::OAuth2 => "oauth2".to_string(),
      Self::None => "".to_string(),
    }
  }
}

pub struct RouteInfo {
  pub resource: Resources,

  pub route: String,
  pub display_route: String,
}

impl RouteInfo {
  pub fn new(resource: Resources) -> Self {
    Self {
      resource,
      route: String::new(),
      display_route: String::new()
    }
  }

  pub fn append(&mut self, segment: &str) {
    self.route.push_str(segment);
    self.display_route.push_str(segment);
  }

  pub fn append_hidden(&mut self, segment: &str, replacement: Option<&str>) {
    self.route.push_str(segment);
    self.display_route.push_str(replacement.unwrap_or("/!*"));
  }
}

pub fn get_route_info(method: &Method, path: &str) -> RouteInfo {
  let path_segments = path.split("/").skip(3).collect::<Vec<&str>>();

  if path_segments.len() == 0 {
    return RouteInfo::new(Resources::None);
  }

  let mut route_info = RouteInfo::new(Resources::from_str(path_segments[0]));
  let major_bucket = match route_info.resource {
    Resources::Invites => {
      "invites/!".to_string()
    },
    Resources::Channels => {
      if path_segments.len() == 2 {
        route_info.append("channels/!");

        return route_info;
      }

      format!("channels/{}", path_segments[1])
    },
    Resources::Guilds => {
      if path_segments.len() == 3 && path_segments[2] == "channels" {
        route_info.append("guilds/!*/channels");

        return route_info;
      }

      if path_segments.len() >= 2 {
        format!("guilds/{}", path_segments[1])
      } else {
        "guilds".to_string()
      }
    },
    Resources::Interactions => {
      if path_segments.len() == 4 && path_segments[2] == "callback" {
        route_info.append(&format!("interactions/{}/!/callback", path_segments[1]));

        return route_info;
      }

      format!("interactions/{}", path_segments[1])
    },
    _ => {
      if path_segments.len() >= 2 {
        format!("{}/{}", path_segments[0], path_segments[1])
      } else {
        path_segments[0].to_string()
      }
    }
  };

  route_info.append(&major_bucket);

  if path_segments.len() <= 2 {
    return route_info;
  }

  for (index, segment) in path_segments[2..].iter().enumerate() {
    let i = index + 2;

    // Split messages into special buckets if they
    // are either under 10 seconds old, or over 14 days old
    if is_snowflake(segment) {
      if route_info.resource == Resources::Guilds 
      && method == Method::DELETE
      && path_segments[i - 1] == "messages"  {
        let snowflake = u64::from_str_radix(segment, 10).unwrap();
        let message_age_ms = get_snowflake_age_ms(snowflake);
        
        if message_age_ms > 14 * 24 * 60 * 60 * 1000 {
          route_info.append("/!14d");
          break;
        } else if message_age_ms < 10 {
          route_info.append("/!10s");
          break;
        }
        
        continue;
      }

      route_info.append("/!*");
      continue;
    }

    // Split reactions into modify/query buckets
    if route_info.resource == Resources::Channels && *segment == "reactions" {
      if method == Method::PUT || method == Method::DELETE {
        route_info.append("/reactions/!modify");
        break
      }

      route_info.append("/reactions/!");
      break
    }

    if segment.len() >= 64 {
      let interaction_id = match route_info.resource {
        Resources::Webhooks => is_interaction_webhook(segment),
        _ => None
      };

      if interaction_id.is_some() {
        route_info.append_hidden(&format!("/{}", interaction_id.unwrap()), Some("/!interaction"));
      } else {
        route_info.append("/!");
      }

      continue;
    }

    route_info.append(&format!("/{}", segment));
  }

  route_info
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

fn is_interaction_webhook(token: &str) -> Option<String> {
  if !token.starts_with("aW50ZXJhY3Rpb246") {
    return None;
  }

  let interaction_data = match forgiving_decode_to_vec(token.as_bytes()) {
    Ok(data) => String::from_utf8(data).unwrap(),
    Err(_) => return None,
  };

  let interaction_id = interaction_data.split(":").skip(1).next();
  if interaction_id.is_none() {
    None
  } else {
    Some(interaction_id.unwrap().to_string())
  }
}