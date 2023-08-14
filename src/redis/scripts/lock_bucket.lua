--  Takes one Key:
--  - Global ID/Bucket ID
-- 
--  And one Argument:
--  - Random data to lock the bucket with.
-- 
--  Returns true if we obtained the lock, false if not.
local route_key = KEYS[1]
local lock_val = ARGV[1]

return redis.call('SET', route_key .. ':lock', lock_val, 'GET', 'NX', 'EX', '5')