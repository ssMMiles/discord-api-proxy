--  Returned ratelimit status can be:
--  - False/Nil: Ratelimit not found, must be fetched.
--  - 0: Ratelimit exceeded.
--  - 1-Infinity: Ratelimit OK, is number of requests in current bucket.
--  
--  Takes two Keys: 
--  - Bot ID
--  - Global RL Key w/ Time Slice
-- 
--  Returns the global ratelimit status.
--  - global_ratelimit_status
local id = KEYS[1]

local global_rl_key = KEYS[2]
local global_count_key = global_rl_key .. ':count'

local global_limit = tonumber(redis.call('GET', id))

if global_limit == nil then
  return false
end

local global_count = tonumber(redis.call('INCR', global_count_key))

if global_count >= global_limit then
  return 0
end

if global_count == 1 then
  redis.call('EXPIRE', global_count_key, 3)
end

return global_count