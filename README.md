# Discord API Proxy
A transparent, Redis backed proxy for handling Discord's API ratelimits.

## Usage

The easiest way to run the proxy for yourself is Docker, images are available [here](https://hub.docker.com/r/limbolabs/discord-api-proxy).

```bash
docker run -d \
  -p 8080:8080 \
  -e HOST=0.0.0.0 \
  -e REDIS_HOST=redis \
  limbolabs/discord-api-proxy
```

Once up and running, just send your normal requests to `http://YOURPROXY/api/v*` instead of `https://discord.com/api/v*`.

You'll get back all the same responses, except when you would have hit a ratelimit - then you'll get a 429 from the proxy with `x-sent-by-proxy` and `x-ratelimit-bucket` headers.

If you get a 429 from the proxy, you should just retry until it works. If it's happening a lot, just stop hitting them.

#### Environment Variables
| Name                       | Description                                                                                                                                                                                                                                                                                                 |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `HOST`                     | The host to listen on. Defaults to `127.0.0.1`.                                                                                                                                                                                                                                                             |
| `PORT`                     | The port to listen on. Defaults to `8080`.                                                                                                                                                                                                                                                                  |
| `DISABLE_HTTP2`            | Whether to disable HTTP/2 support. Defaults to `false`.                                                                                                                                                                                                                                                     |
| `REDIS_HOST`               | The host of the Redis server. Defaults to `127.0.0.1`.                                                                                                                                                                                                                                                      |
| `REDIS_PORT`               | The port of the Redis server. Defaults to `6379`.                                                                                                                                                                                                                                                           |
| `REDIS_USER`               | The host of the Redis server. Defaults to an empty string, is only available on Redis 6+.                                                                                                                                                                                                                   |
| `REDIS_PASS`               | The host of the Redis server. If unset, auth is disabled.                                                                                                                                                                                                                                                   |
| `REDIS_POOL_SIZE`          | The size of the Redis connection pool. Defaults to `64`. Note: At least one connection is always reserved for PubSub.                                                                                                                                                                                       |
| `REDIS_SENTINEL`           | Whether to enable Redis Sentinel support. Defaults to `false`.                                                                                                                                                                                                                                              |
| `REDIS_SENTINEL_MASTER`    | The name of the Redis Sentinel master. Defaults to `mymaster`.                                                                                                                                                                                                                                              |
| `LOCK_WAIT_TIMEOUT`        | Duration (in ms) a request should wait for a lock to be released before retrying. Defaults to `500`.                                                                                                                                                                                                        |
| `RATELIMIT_ABORT_PERIOD`   | If the proxy does ever hit a 429, the duration (in ms) it should abort all incoming requests with a 503 for this amount of time. Defaults to `1000`.                                                                                                                                                        |
| `GLOBAL_TIME_SLICE_OFFSET` | The offset (in ms) to add to the global ratelimit's 1s fixed window to make up for the round trip to Discord. You probably don't want to mess with this unless you have a very high ping to the API. Defaults to `200`.                                                                                     |
| `DISABLE_GLOBAL_RATELIMIT` | Whether to disable the global ratelimit checks, only use this if you're sure you won't hit it. Defaults to `false`.                                                                                                                                                                                         |
| `BUCKET_TTL`               | How long the proxy will cache bucket info for. Set to `0` to store forever, but this isn't recommended. Defaults to `86400000` (24h), except for interaction buckets (Ignores this value, always 15 minutes). If trying to save memory consider using `maxmemory` and `allkeys-lru` on your Redis instance. |

## Warnings

### Slightly Reduced Throughput
When sending globally ratelimited requests, the proxy currently starts the 1s bucket time from when the first response is received as Discord never provide the exact time. This can cause the proxy to be slightly over-restrictive depending on your , with a bot where the global ratelimit is 50 only being allowed up to ~47 requests per second.

### Overloading
Under heavy load slow calls to Redis can cause the global ratelimit time slice to drift and let through more requests than it should.

To mitigate this, the proxy monitors its latency to Redis and will reject requests with a 503 and an `x-sent-by-proxy` header if things slow down too much. These could just be caused by a short latency spike, but if it's happening often you should take a look.

If you're seeing lots of warnings about this in your logs, [check out this page](https://redis.io/docs/management/optimization/latency/).

## Credits
  - [Nirn Proxy](https://github.com/germanoeich/nirn-proxy) by [@germanoeich](https://github.com/germanoeich) - Used as a reference for bucket mappings
  
