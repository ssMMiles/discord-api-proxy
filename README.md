# WIP - Discord API Proxy
A transparent proxy for handling Discord's API ratelimits.

## Todo
 - Metrics
 - Finish/cleanup route specific ratelimiting, not tested much
 - Refactor everything, it's a mess

## Usage

Once up and running, just send any requests you would normally send to `https://discord.com/api/v*` to `http://YOURPROXY/api/v*`.

You'll get back all the same responses as Discord, except when one would have hit a 429 - then you'll get a 429 from the proxy instead with `x-sent-by-proxy` and `x-ratelimit-bucket` headers.

When you do hit a 429, you can just retry the request until it works. If that's happening a lot, just stop hitting them.

#### Environment Variables
Name | Description
--- | ---
`HOST` | The host to listen on. Defaults to `127.0.0.1`.
`PORT` | The port to listen on. Defaults to `8080`.
`REDIS_HOST` | The host of the Redis server. (Required)
`REDIS_PORT` | The port of the Redis server. Defaults to `6379`.

## Credits
  - [Nirn Proxy](https://github.com/germanoeich/nirn-proxy) by [@germanoeich](https://github.com/germanoeich) - Used as a reference for mapping routes to shared buckets
  