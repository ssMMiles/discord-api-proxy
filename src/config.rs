use std::{
    env::{self, VarError},
    ffi::OsString,
    fmt::Display,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

pub struct RedisEnvConfig {
    pub host: String,
    pub port: u16,

    pub username: Option<String>,
    pub password: Option<String>,

    pub pool_size: usize,

    pub sentinel: bool,
    pub clustered: bool,

    pub sentinel_auth: bool,
    pub sentinel_master: String,
}

pub struct WebserverEnvConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, PartialEq)]
pub enum NewBucketStrategy {
    Strict,
    Loose,
}

impl FromStr for NewBucketStrategy {
    type Err = ();

    fn from_str(input: &str) -> Result<NewBucketStrategy, Self::Err> {
        match input.to_lowercase().as_str() {
            "strict" => Ok(NewBucketStrategy::Strict),
            "loose" => Ok(NewBucketStrategy::Loose),
            _ => Err(()),
        }
    }
}

impl Display for NewBucketStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NewBucketStrategy::Strict => write!(f, "NewBucketStrategy::Strict"),
            NewBucketStrategy::Loose => write!(f, "NewBucketStrategy::Loose"),
        }
    }
}

#[derive(Clone)]
pub struct ProxyEnvConfig {
    pub global_rl_strategy: NewBucketStrategy,
    pub route_rl_strategy: NewBucketStrategy,

    pub disable_global_rl: bool,
    pub lock_timeout: Duration,

    pub bucket_ttl_ms: u64,

    pub disable_http2: bool,
    pub clustered_redis: bool, // TODO: Clustered redis only really needs a small number of changes to the client as all keys are already namespaced, but it's not finished yet

    #[cfg(feature = "metrics")]
    pub metrics_ttl: u64,
}

pub enum EnvError {
    NotPresent(String),
    InvalidUnicode(String, OsString),
}

impl From<(String, VarError)> for EnvError {
    fn from((key, err): (String, VarError)) -> Self {
        match err {
            VarError::NotPresent => EnvError::NotPresent(key),
            VarError::NotUnicode(value) => EnvError::InvalidUnicode(key, value),
        }
    }
}

fn get_and_parse_envvar<T: FromStr + std::fmt::Display>(key: &str, default: T) -> T {
    match env::var(key) {
        Ok(value) => match value.parse() {
            Ok(parsed) => parsed,
            Err(_) => {
                eprintln!("Failed to parse value for environment variable {}={:?}. Using default value of {}", key, value, default);

                default
            }
        },
        Err(VarError::NotPresent) => default,
        Err(VarError::NotUnicode(value)) => {
            eprintln!(
                "Invalid value for environment variable {}={:?}. Using default value.",
                key, value
            );

            default
        }
    }
}

fn get_optional_envvar(key: &str) -> Option<String> {
    match env::var(key) {
        Ok(value) => Some(value),
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(value)) => {
            eprintln!(
                "Invalid value for environment variable {}={:?}.",
                key, value
            );

            None
        }
    }
}

fn get_envvar_with_default(key: &str, default: String) -> String {
    match env::var(key) {
        Ok(value) => value,
        Err(VarError::NotPresent) => default,
        Err(VarError::NotUnicode(value)) => {
            eprintln!(
                "Invalid value for environment variable {}={:?}. Using default value.",
                key, value
            );

            default
        }
    }
}

pub struct AppEnvConfig {
    pub redis: Arc<RedisEnvConfig>,
    pub webserver: Arc<WebserverEnvConfig>,
    pub proxy: Arc<ProxyEnvConfig>,
}

impl AppEnvConfig {
    pub fn from_env() -> Self {
        let sentinel_redis = get_and_parse_envvar::<bool>("REDIS_SENTINEL", false);
        let clustered_redis = get_and_parse_envvar::<bool>("REDIS_CLUSTER", false);

        if sentinel_redis && clustered_redis {
            panic!("Cannot use both Redis Sentinel and Redis Cluster at the same time.");
        }

        let sentinel_auth = get_and_parse_envvar::<bool>("REDIS_SENTINEL_AUTH", false);
        let sentinel_master =
            get_envvar_with_default("REDIS_SENTINEL_MASTER", "mymaster".to_owned());

        let default_redis_port = if sentinel_redis { 26379 } else { 6379 };

        let redis_host = get_envvar_with_default("REDIS_HOST", "127.0.0.1".to_string());
        let redis_port = get_and_parse_envvar::<u16>("REDIS_PORT", default_redis_port);

        let redis_user = get_optional_envvar("REDIS_USER");
        let redis_pass = get_optional_envvar("REDIS_PASS");

        let redis_pool_size = get_and_parse_envvar::<usize>("REDIS_POOL_SIZE", 128);

        let lock_wait_timeout = get_and_parse_envvar::<u64>("LOCK_WAIT_TIMEOUT", 500);

        let global_ratelimit_strategy = get_and_parse_envvar::<NewBucketStrategy>(
            "GLOBAL_RATELIMIT_STRATEGY",
            NewBucketStrategy::Strict,
        );
        let route_ratelimit_strategy = get_and_parse_envvar::<NewBucketStrategy>(
            "ROUTE_RATELIMIT_STRATEGY",
            NewBucketStrategy::Strict,
        );

        let disable_global_rl = get_and_parse_envvar::<bool>("DISABLE_GLOBAL_RATELIMIT", false);

        let bucket_ttl_ms = get_and_parse_envvar::<u64>("BUCKET_TTL", 86400000);

        let disable_http2 = get_and_parse_envvar::<bool>("DISABLE_HTTP2", true);

        let host = get_envvar_with_default("HOST", "127.0.0.1".to_string());
        let port = get_and_parse_envvar::<u16>("PORT", 8080);

        #[cfg(feature = "metrics")]
        let metrics_ttl = get_and_parse_envvar::<u64>("METRICS_TTL", 86400000);

        Self {
            redis: Arc::new(RedisEnvConfig {
                host: redis_host,
                port: redis_port,

                username: redis_user,
                password: redis_pass,

                pool_size: redis_pool_size,

                sentinel: sentinel_redis,
                clustered: clustered_redis,

                sentinel_auth,
                sentinel_master,
            }),

            webserver: Arc::new(WebserverEnvConfig { host, port }),

            proxy: Arc::new(ProxyEnvConfig {
                bucket_ttl_ms,

                global_rl_strategy: global_ratelimit_strategy,
                route_rl_strategy: route_ratelimit_strategy,

                disable_global_rl,

                lock_timeout: Duration::from_millis(lock_wait_timeout),

                disable_http2,

                clustered_redis,

                #[cfg(feature = "metrics")]
                metrics_ttl,
            }),
        }
    }
}
