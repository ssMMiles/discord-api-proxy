# Discord API Proxy
A transparent, Redis backed proxy for handling Discord's API ratelimits.

## Todo
 - Look into better solutions for getting the start time on the global ratelimit bucket, as 
  we currently wait for a response to be safe - in exchange for 15-25% less actual throughput.
 - Use pub/sub when waiting for a bucket's info to be available instead of retrying every 300ms.
 - Better handling of 429s; On hitting one temporarily (1s) abort all requests with a 503 (`x-sent-by-proxy` header still present)?
 - Logging

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

If you get a 429 from the proxy, you can just retry until it works. If it's happening a lot, just stop hitting them lol.

#### Environment Variables
Name | Description
--- | ---
`HOST` | The host to listen on. Defaults to `127.0.0.1`.
`PORT` | The port to listen on. Defaults to `8080`.
`REDIS_HOST` | The host of the Redis server. Defaults to `127.0.0.1`.
`REDIS_PORT` | The port of the Redis server. Defaults to `6379`.
`METRICS` | Whether to expose Prometheus metrics on `/metrics`. Defaults to `true`.

#### Warnings

Under extreme load, Redis calls to check ratelimits may start to slow down, causing the global ratelimit bucket expiry to drift. We try to mitigate this by monitoring the time taken for ratelimit checks and aborting the request when overloaded, but some can still slip through.

In the event that the proxy does receive a 429 from Discord, it will immediately exit to prevent further damage. This is considered fatal and may leave Redis in a bad state. (In the proxy's current state, give it its own database that you can easily flush if this does happen.)

## Credits
  - [Nirn Proxy](https://github.com/germanoeich/nirn-proxy) by [@germanoeich](https://github.com/germanoeich) - Used as a reference for bucket mappings
  
