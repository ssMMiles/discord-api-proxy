local function lock_bucket(key, token)
    local result = redis.call('SET', key .. ':lock', token, 'NX', 'EX', '5')
    return result ~= false
end

local function increment_global_count(key)
    local global_count = redis.call('INCR', key)

    if global_count == 1 then
        redis.call('EXPIRE', key, 3)
    end

    return global_count
end

local function increment_route_count(key)
    local route_count = tonumber(redis.call('INCR', key))
    
    if route_count == 1 then
        redis.call('EXPIRE', key, 60)
    end

    return route_count
end

local global_key = KEYS[1]
local time_slice = KEYS[2]
local global_count_key = global_key .. time_slice

local route_key = KEYS[3]
local route_count_key = route_key .. ':count'

local lock_token = ARGV[1]

local ratelimits = redis.call('MGET', global_key, route_key, global_count_key, route_count_key)

local global_limit = tonumber(ratelimits[1])
local route_limit = tonumber(ratelimits[2])

local global_count = tonumber(ratelimits[3])
local route_count = tonumber(ratelimits[4])

if global_count == nil then
    global_count = 0
end

if route_count == nil then
    route_count = 0
end

local holds_global_lock = false
local holds_route_lock = false

if route_limit == nil then
    if global_limit == nil then
        holds_global_lock = lock_bucket(global_key, lock_token)

        if holds_global_lock == false then
            return 1
        end
    else
        if global_count + 1 > global_limit then
            return {0, global_limit}
        end
    end

    holds_route_lock = lock_bucket(route_key, lock_token)

    if holds_route_lock == false then
        if holds_global_lock then
            return 4
        else
            return 1
        end
    end
else
    if route_count + 1 > route_limit then
        local reset_after = redis.call('PTTL', route_key .. ':reset_after')

        if reset_after ~= -2 then
            local reset_at = redis.call('PEXPIRETIME', route_count_key)

            return {2, route_limit, reset_at, reset_after}
        end

        if global_limit == nil then
            holds_global_lock = lock_bucket(global_key, lock_token)

            if holds_global_lock == false then
                return 1
            end
        else
            if global_count + 1 > global_limit then
                return {0, global_limit}
            end
        end

        holds_route_lock = lock_bucket(route_key, lock_token)

        if holds_route_lock == false then
            if holds_global_lock then
                return 4
            else
                return 3
            end
        end
    else
        if global_limit == nil then
            holds_global_lock = lock_bucket(global_key, lock_token)

            if holds_global_lock == false then
                return 1
            end
        else
            if global_count + 1 > global_limit then
                return {0, global_limit}
            end
        end
    end
end

increment_global_count(global_count_key)
increment_route_count(route_count_key)

return {5, holds_global_lock, holds_route_lock}