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

if bucket_count >= bucket_limit then
  return 0
end

return bucket_count