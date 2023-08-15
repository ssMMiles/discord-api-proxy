--  Keys:
--  - Global ID
-- 
--  Arguments:
--  - Request ID
--  - Global limit
--  - Global limit TTL (in ms)
-- 
--  Returns true if we unlocked the global bucket, false if we were too slow.

local global_id = KEYS[1]
local global_lock_key = global_id .. ':lock'

local lock_val = ARGV[1]
local global_limit = ARGV[2]
local bucket_expire_in = ARGV[3]

-- redis.log(redis.LOG_NOTICE, 'unlocking global bucket: ' .. global_id .. ' with lock key: ' .. global_lock_key .. ' and lock val: ' .. lock_val .. ' and limit: ' .. global_limit .. ' and expire in: ' .. bucket_expire_in)

local global_lock = redis.call('GET', global_lock_key)

if global_lock == lock_val then
  if bucket_expire_in == '0' then
    redis.call('SET', global_id, global_limit)
  else
    redis.call('SET', global_id, global_limit, 'PX', bucket_expire_in)
  end

  redis.call('DEL', global_lock_key)
  redis.call('PUBLISH', 'unlock', global_id)

  return true
end

return false