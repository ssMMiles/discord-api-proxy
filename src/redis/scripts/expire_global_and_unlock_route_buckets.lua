local global_count_key = KEYS[1] .. ':count'
local global_expire_at = tonumber(ARGV[1])

redis.call('PEXPIREAT', global_count_key, global_expire_at, 'LT')

local route_key = KEYS[2]
local route_lock_key = route_key .. ':lock'

local lock_val = ARGV[2]
local route_lock = redis.call('GET', route_lock_key)

if route_lock == lock_val then
  local route_limit = ARGV[3]
  redis.call('SET', route_key, route_limit)

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