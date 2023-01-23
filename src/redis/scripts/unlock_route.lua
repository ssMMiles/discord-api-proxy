local route_key = KEYS[1]
local route_lock_key = route_key .. ':lock'

local lock_val = ARGV[1]
local route_lock = redis.call('GET', route_lock_key)

if route_lock == lock_val then
  local route_limit = ARGV[2]
  redis.call('SET', route_key, route_limit)

  local route_expire_at = tonumber(ARGV[3])
  if route_expire_at ~= 0 then
    local route_count_key = route_key .. ':count'

    redis.call('PEXPIREAT', route_count_key, route_expire_at)
  end

  redis.call('DEL', route_lock_key)
  redis.call('PUBLISH', 'unlock', route_key)

  return true
end

return false