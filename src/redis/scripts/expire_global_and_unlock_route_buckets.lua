--  Takes two Keys:
--  - Bot ID
--  - Bucket ID
-- 
--  And five Arguments:
--  - Global usage expiration time (in ms)
--  - Lock value to check against.
--  - Route bucket limit
--  - Route bucket usage expiration time (in ms)
--  - Route bucket TTL (in ms)
local global_count_key = KEYS[1] .. ':count'
local global_expire_at = tonumber(ARGV[1])

redis.call('PEXPIREAT', global_count_key, global_expire_at, 'LT')

local route_key = KEYS[2]
local route_lock_key = route_key .. ':lock'

local lock_val = ARGV[2]
local route_lock = redis.call('GET', route_lock_key)

if route_lock == lock_val then
  local route_limit = ARGV[3]
  local bucket_expire_in = tonumber(ARGV[5])
  
  if bucket_expire_in == 0 then
    redis.call('SET', route_key, route_limit)
  else
    redis.call('SET', route_key, route_limit, 'PX', bucket_expire_in)
  end

  local route_expire_at = tonumber(ARGV[4])
  if route_expire_at ~= 0 then
    local route_count_key = route_key .. ':count'

    redis.call('PEXPIREAT', route_count_key, route_expire_at)
  end

  redis.call('DEL', route_lock_key)
  redis.call('PUBLISH', 'unlock', route_key)

  return true
end

return false