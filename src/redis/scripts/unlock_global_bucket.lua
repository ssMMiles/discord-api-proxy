local global_key = KEYS[1]
local global_lock_key = global_key .. ':lock'

local lock_val = ARGV[1]
local global_lock = redis.call('GET', global_lock_key)

if global_lock == lock_val then
  local global_limit = ARGV[2]

  redis.call('SET', global_key, global_limit)
  redis.call('DEL', global_lock_key)

  return true
end

return false