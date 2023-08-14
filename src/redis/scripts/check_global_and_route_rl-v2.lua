local function lock_bucket(key, token)
    local result = redis.call('SET', key .. ':lock', token, 'NX', 'EX', '5')
    return result ~= false
end

local function increment_global_count(key, time_slice)
    local time_sliced_key = key .. time_slice
    local global_count = redis.call('INCR', time_sliced_key)

    if global_count == 1 then
        redis.call('EXPIRE', time_sliced_key, 3)
    end

    return global_count
end

local function increment_route_count(key)
    local count_key = key .. ':count'
    local route_count = tonumber(redis.call('INCR', count_key))
    
    if route_count == 1 then
        redis.call('EXPIRE', count_key, 60)
    end

    return route_count
end

local global_key = KEYS[1]
local time_slice = KEYS[2]

local route_key = KEYS[3]

local lock_token = ARGV[1]

local ratelimits = redis.call('MGET', global_key, route_key)

local global_limit = tonumber(ratelimits[1])
local route_limit = tonumber(ratelimits[2])

local holds_global_lock = false
local holds_route_lock = false

local global_count
if global_limit == nil then
    holds_global_lock = lock_bucket(global_key, lock_token)

    if holds_global_lock == false then
        return 1
    end
else
    global_count = increment_global_count(global_key, time_slice)

    if global_count > global_limit then
        local remaining = 0
        return {0, global_limit}
    end
end

if route_limit == nil then
    holds_route_lock = lock_bucket(route_key, lock_token)

    if holds_route_lock == false then
        if holds_global_lock then
            return 4
        end

        return 3
    end
end

if holds_global_lock == true then
    global_count = increment_global_count(global_key, time_slice)
end

local route_count = increment_route_count(route_key)

if route_limit ~= nil and route_count > route_limit then
    local reset_after = redis.call('PTTL', route_key .. ':reset_after')

    if reset_after ~= -2 then
        local remaining = 0
        local reset_at = redis.call('PEXPIRETIME', route_key .. ':count')

        return {2, remaining, route_limit, reset_at, reset_after}
    end

    holds_route_lock = lock_bucket(route_key, lock_token)

    if holds_route_lock == false then
        if holds_global_lock then
            return 4
        end

        return 3
    end
end

return {5, holds_global_lock, holds_route_lock}