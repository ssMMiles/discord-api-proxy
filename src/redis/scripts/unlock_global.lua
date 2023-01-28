--  Takes one Key:
--  - Bot ID
-- 
--  And two Arguments:
--  - Lock value to check against.
--  - Global ratelimit
-- 
--  Returns true if we unlocked the global bucket, false if we were too slow.
local id = KEYS[1]
local global_lock_key = id .. ':lock'

local lock_val = ARGV[1]
local global_lock = redis.call('GET', global_lock_key)

if global_lock == lock_val then
  local global_limit = ARGV[2]

  redis.call('SET', id, global_limit)

  redis.call('DEL', global_lock_key)
  redis.call('PUBLISH', 'unlock', id)

  return true
end

return false