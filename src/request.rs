use base64_simd::forgiving_decode_to_vec;
use http::{HeaderMap, Method};

use crate::{
    buckets::{BucketInfo, Resources},
    proxy::ProxyError,
};

#[derive(Clone, Debug)]
pub struct DiscordRequestInfo {
    pub global_id: String,
    pub token: Option<String>,

    pub global_id_redis_key: String,

    pub resource: Resources,
    pub uses_global_ratelimit: bool,

    pub route_bucket: String,
    pub route_display_bucket: String,

    pub route_bucket_redis_key: String,

    pub require_auth: bool,
}

impl DiscordRequestInfo {
    const DEFAULT_GLOBAL_ID: &str = "NoAuth";

    pub fn new(method: &Method, path: &str, headers: &HeaderMap) -> Result<Self, ProxyError> {
        let bucket_info = BucketInfo::new(&method, &path)?;

        let can_ignore_auth = (bucket_info.resource == Resources::Webhooks
            && bucket_info.route_bucket.split("/").count() != 2)
            || bucket_info.resource == Resources::OAuth2
            || bucket_info.resource == Resources::Interactions;
        let require_auth = !can_ignore_auth;

        let auth = parse_headers(headers, require_auth)?;

        let (global_id, token): (String, Option<String>) = match &auth {
            Some((id, token)) => (id.into(), Some(token.into())),
            None => (Self::DEFAULT_GLOBAL_ID.into(), None),
        };

        let route_uses_global_ratelimit = match bucket_info.resource {
            Resources::Webhooks => false,
            Resources::Interactions => false,
            _ => true,
        };

        let uses_global_ratelimit =
            route_uses_global_ratelimit && global_id != Self::DEFAULT_GLOBAL_ID;

        let global_id_redis_key = format!("global:{{{}}}", global_id);
        let route_bucket_redis_key = if uses_global_ratelimit {
            format!("{}-route:{}", global_id_redis_key, bucket_info.route_bucket)
        } else {
            format!("route:{{{}}}", bucket_info.route_bucket)
        };

        Ok(Self {
            global_id,
            token,

            global_id_redis_key,

            resource: bucket_info.resource,
            uses_global_ratelimit,

            route_bucket: bucket_info.route_bucket,
            route_display_bucket: bucket_info.route_display_bucket,

            route_bucket_redis_key,

            require_auth,
        })
    }
}

fn parse_headers(
    headers: &HeaderMap,
    require_auth: bool,
) -> Result<Option<(String, String)>, ProxyError> {
    // Use auth header by default
    let token = match headers.get("Authorization") {
        Some(header) => {
            let token = match header.to_str() {
                Ok(token) => token,
                Err(_) => {
                    return Err(ProxyError::InvalidRequest(
                        "Invalid Authorization header".into(),
                    ))
                }
            };

            if !token.starts_with("Bot ") {
                return Err(ProxyError::InvalidRequest(
                    "Invalid Authorization header".into(),
                ));
            }

            token.to_string()
        }
        None => {
            if require_auth {
                return Ok(None);
            }

            return Err(ProxyError::InvalidRequest(
                "Missing Authorization header".into(),
            ));
        }
    };

    let base64_bot_id = match token[4..].split('.').nth(0) {
        Some(base64_bot_id) => base64_bot_id.as_bytes(),
        None => {
            return Err(ProxyError::InvalidRequest(
                "Invalid Authorization header".into(),
            ))
        }
    };

    let bot_id = String::from_utf8(
        forgiving_decode_to_vec(base64_bot_id)
            .map_err(|_| ProxyError::InvalidRequest("Invalid Authorization header".into()))?,
    )
    .map_err(|_| ProxyError::InvalidRequest("Invalid Authorization header".into()))?;

    Ok(Some((bot_id, token)))
}
