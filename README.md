# WIP - Discord API Proxy
A transparent proxy for handling Discord's API ratelimits.

## Todo
 - Fix race condition in global ratelimit
 - Refactor everything, it's a mess

## Usage

Once up and running, just send your normal requests to `http://YOURPROXY/api/v*` instead of `https://discord.com/api/v*`.

You'll get back all the same responses, except when you would have hit a ratelimit - then you'll get a 429 from the proxy with `x-sent-by-proxy` and `x-ratelimit-bucket` headers

If you hit a 429, you can just retry the request until it works. If that's happening a lot, just stop hitting them.

#### Environment Variables
Name | Description
--- | ---
`HOST` | The host to listen on. Defaults to `127.0.0.1`.
`PORT` | The port to listen on. Defaults to `8080`.
`REDIS_HOST` | The host of the Redis server. (Required)
`REDIS_PORT` | The port of the Redis server. Defaults to `6379`.

## Credits
  - [Nirn Proxy](https://github.com/germanoeich/nirn-proxy) by [@germanoeich](https://github.com/germanoeich) - Used as a reference for bucket mappings
  