--  Returned ratelimit status can be:
--  - False/Nil: Ratelimit not found, must be fetched.
--  - 0: Ratelimit exceeded.
--  - 1-Infinity: Ratelimit OK, is number of requests in current bucket.
--  
--  Takes one Key: 
--  - Bucket ID
-- 
--  Returns the bucket ratelimit status.
--  - bucket_ratelimit_status
local bucket_key = KEYS[1]
local bucket_count_key = bucket_key .. ':count'

local bucket_limit = tonumber(redis.call('GET', bucket_key))

if bucket_limit == nil then
  return false
end

if bucket_limit == 0 then
  return 2
end

local bucket_count = tonumber(redis.call('INCR', bucket_count_key))

if bucket_count == 1 then
  redis.call('EXPIRE', bucket_count_key, 60)
end

if bucket_count >= bucket_limit then
  return 0
end

return bucket_count