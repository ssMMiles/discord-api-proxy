--  Takes one Key:
--  - Bot ID
-- 
--  And three Arguments:
--  - Lock value to check against.
--  - Global limit
--  - Global limit TTL (in ms)
-- 
--  Returns true if we unlocked the global bucket, false if we were too slow.
local id = KEYS[1]
local global_lock_key = id .. ':lock'

local lock_val = ARGV[1]
local global_lock = redis.call('GET', global_lock_key)

if global_lock == lock_val then
  local global_limit = ARGV[2]
  local bucket_expire_in = tonumber(ARGV[3])

  if bucket_expire_in == 0 then
    redis.call('SET', id, global_limit)
  else
    redis.call('SET', id, global_limit, 'PX', bucket_expire_in)
  end

  redis.call('DEL', global_lock_key)
  redis.call('PUBLISH', 'unlock', id)

  return true
end

return false