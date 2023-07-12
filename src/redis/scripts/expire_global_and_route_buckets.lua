--  Takes two Keys:
--  - Bot ID
--  - Bucket ID
-- 
--  And two Arguments:
--  - Global usage expiration time (in ms)
--  - Bucket usage expiration time (in ms)
-- 
--  Returns nothing.
local global_count_key = KEYS[1] .. ':count'
local global_expire_at = tonumber(ARGV[1])

redis.call('PEXPIREAT', global_count_key, global_expire_at, 'LT')

local bucket_count_key = KEYS[2] .. ':count'
local bucket_expire_at = tonumber(ARGV[2])

redis.call('PEXPIREAT', bucket_count_key, bucket_expire_at)