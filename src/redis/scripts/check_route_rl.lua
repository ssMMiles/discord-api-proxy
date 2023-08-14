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

local function check_route_rl (route_key)
    local route_count_key = route_key .. ':count'
    
    local route_limit = tonumber(redis.call('GET', route_key))
    
    if route_limit == nil then
        return false
    end
    
    if route_limit == 0 then
        return 2
    end
    
    local route_count = tonumber(redis.call('INCR', route_count_key))
    
    if route_count == 1 then
        redis.call('EXPIRE', route_count_key, 60)
    end
    
    if route_count > route_limit then
        return 0
    end
    
    return route_count
end

local route_key = KEYS[1]
return check_route_rl(route_key)