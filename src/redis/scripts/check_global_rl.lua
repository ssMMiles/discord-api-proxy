--  Returned ratelimit status can be:
--  - False/Nil: Ratelimit not found, must be fetched.
--  - 0: Ratelimit exceeded.
--  - 1-Infinity: Ratelimit OK, is number of requests in current bucket.
--  
--  Takes two Keys: 
--  - Global ID
--  - Time Slice
-- 
--  Returns the global ratelimit status.
--  - global_ratelimit_status

local function check_global_rl(global_id, time_slice)
    local global_time_slice_key = global_id .. time_slice
    local global_time_slice_count_key = global_time_slice_key .. ':count'
    
    local global_limit = tonumber(redis.call('GET', global_id))
    
    if global_limit == nil then
        return false
    end
    
    local global_time_slice_count = tonumber(redis.call('INCR', global_time_slice_count_key))
    
    if global_time_slice_count == 1 then
        redis.call('EXPIRE', global_time_slice_count_key, 3)
    end
    
    if global_time_slice_count > global_limit then
        return 0
    end
    
    return global_time_slice_count
end

local global_id = KEYS[1]
local time_slice = KEYS[2]
return check_global_rl(global_id, time_slice)