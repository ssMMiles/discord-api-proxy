--  Takes one key:
--  - Bucket ID
-- 
--  And one argument:
--  - Bucket usage expiration time (in ms)
local bucket_count_key = KEYS[1] .. ':count'
local bucket_expire_at = tonumber(ARGV[1])

redis.call('PEXPIREAT', bucket_count_key, bucket_expire_at)