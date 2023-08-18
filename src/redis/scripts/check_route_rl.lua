local function lock_bucket(key, token)
    local result = redis.call('SET', key .. ':lock', token, 'NX', 'EX', '5')
    return result ~= false
end

local function increment_route_count(key)
    local route_count = tonumber(redis.call('INCR', key))
    
    if route_count == 1 then
        redis.call('EXPIRE', key, 60)
    end

    return route_count
end

local route_key = KEYS[1]
local route_count_key = route_key .. ':count'

local lock_token = ARGV[1]

local route_limit = tonumber(redis.call('GET', route_key))

local holds_route_lock = false
if route_limit == nil then
    holds_route_lock = lock_bucket(route_key, lock_token)

    if holds_route_lock == false then
        return 3
    end
end

local route_count = increment_route_count(route_count_key)

if route_count > route_limit then
    local reset_after = redis.call('PTTL', route_key .. ':reset_after')

    if reset_after ~= -2 then
        local reset_at = redis.call('PEXPIRETIME', route_count_key)

        return {2, route_limit, reset_at, reset_after} 
    end

    holds_route_lock = lock_bucket(route_key, lock_token)

    if holds_route_lock == false then
        return 3
    end
end

local holds_global_lock = false
return {5, holds_global_lock, holds_route_lock}