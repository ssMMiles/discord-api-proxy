local route_key = KEYS[1]
local rl = redis.call('GET', route_key)

if rl == false then
  local lock_val = ARGV[1]
  local lock = redis.call('SET', route_key .. ':lock', lock_val, 'NX', 'EX', '5')

  local got_lock = lock ~= false

  return got_lock
end

return false