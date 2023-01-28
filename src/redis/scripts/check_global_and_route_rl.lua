--  Returned ratelimit status can be:
--  - False/Nil: Ratelimit not found, must be fetched.
--  - 0: Ratelimit exceeded.
--  - 1-Infinity: Ratelimit OK, is number of requests in current bucket.
--  
--  Takes three Keys: 
--  - Bot ID
--  - Global RL Key w/ Time Slice
--  - Bucket ID
-- 
--  Returns a list containing status of both global and bucket ratelimits.
--  - [global_ratelimit_status, bucket_ratelimit_status]

-- Global RL Check
local id = KEYS[1]

local global_rl_key = KEYS[2]
local global_count_key = global_rl_key .. ':count'

local global_limit = tonumber(redis.call('GET', id))

if global_limit == nil then
  return {false, false}
end

local global_count = tonumber(redis.call('INCR', global_count_key))

if global_count >= global_limit then
  return {0, false}
end

if global_count == 1 then
  redis.call('EXPIRE', global_count_key, 3)
end

-- Route RL Check

local route_key = KEYS[3]
local route_count_key = route_key .. ':count'

local route_limit = tonumber(redis.call('GET', route_key))

if route_limit == nil then
  return {global_count, false}
end

if route_limit == 0 then
  return {global_count, 2}
end

local route_count = tonumber(redis.call('INCR', route_count_key))

if route_count == 1 then
  redis.call('EXPIRE', route_count_key, 60)
end

if route_count >= route_limit then
  if tonumber(redis.call('DECR', global_count_key)) == 0 then
    redis.call('DEL', global_count_key)
  end

  return {1, 0}
end

return {global_count, route_count}