# Discord API Proxy
A transparent, Redis backed proxy for handling Discord's API ratelimits.

## Usage

The easiest way to run the proxy for yourself is Docker, images are available [here](https://hub.docker.com/r/ssmmiles/discord-api-proxy).

```bash
docker run -d \
  -p 8080:8080 \
  -e HOST=0.0.0.0 \
  -e REDIS_HOST=redis \
  ssmmiles/discord-api-proxy
```

Once up and running, just send your normal requests to `http://YOURPROXY/api/v*` instead of `https://discord.com/api/v*`.

You'll get back all the same responses, except when you would have hit a ratelimit - then you'll get a 429 from the proxy with `x-sent-by-proxy` and `x-ratelimit-bucket` headers.

If you get a 429 from the proxy, you should just retry until it works. If it's happening a lot, just stop hitting them.

#### Environment Variables
Name | Description
--- | ---
`HOST` | The host to listen on. Defaults to `127.0.0.1`.
`PORT` | The port to listen on. Defaults to `8080`.
`REDIS_HOST` | The host of the Redis server. Defaults to `127.0.0.1`.
`REDIS_PORT` | The port of the Redis server. Defaults to `6379`.
`REDIS_USER` | The host of the Redis server. Defaults to an empty string, is only available on Redis 6+`.
`REDIS_PASS` | The host of the Redis server. If unset, auth is disabled.
`REDIS_POOL_SIZE` | The size of the Redis connection pool. Defaults to `64`. Note: At least one connection is always reserved for PubSub.
`ENABLE_METRICS` | Whether to expose Prometheus metrics on `/metrics`. Defaults to `true`.
`LOCK_WAIT_TIMEOUT` | Duration (in ms) a request should wait for a lock to be released before retrying. Defaults to `500`.
`RATELIMIT_ABORT_PERIOD` | If the proxy does ever hit a 429, the duration (in ms) it should abort all incoming requests with a 503 for. Defaults to `1000`.

## Warnings

### Reduced Throughput
When sending globally ratelimited requests, the proxy currently starts the 1s bucket time from when the first response is received as Discord never provide the exact time. This can cause the proxy to be slightly over-restrictive, with a bot where the global ratelimit is 50 only being allowed up to ~47 requests per second.

### Overloading
Under extreme load, Redis calls to check ratelimits may start to slow down, causing the global ratelimit bucket expiry to drift. We try to mitigate this by monitoring the time taken for ratelimit checks and aborting the request when overloaded, but some can still slip through.

In the event that the proxy does receive a 429 from Discord, it will abort all further requests for 1 second.

### Dirty Cache
Due to ratelimit expiry being set seperately to the ratelimit info, it's possible for a request to be sent to Discord, but the proxy/Redis/network to drop out before the expiry is set. Be prepared to flush Redis if this happens, or you see issues with requests hanging.

## Credits
  - [Nirn Proxy](https://github.com/germanoeich/nirn-proxy) by [@germanoeich](https://github.com/germanoeich) - Used as a reference for bucket mappings
  
