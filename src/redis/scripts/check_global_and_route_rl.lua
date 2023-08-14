--  Keys:
--  - Global ID
--  - Time Slice
--  - Bucket ID
--
--  Arguments:
--  - Lock Token
-- 
--  Returns:
--  - [global_remaining, global_limit, global_lock, route_remaining, route_limit, route_reset_at, route_lock]

local function check_global_rl(global_id, global_time_slice_count_key)
    local global_limit = tonumber(redis.call('GET', global_id))

    if global_limit == nil then
        return false, false
    end

    local global_time_slice_count = tonumber(redis.call('INCR', global_time_slice_count_key))
    
    if global_time_slice_count == 1 then
        redis.call('EXPIRE', global_time_slice_count_key, 3)
    end

    return global_time_slice_count, global_limit
end

local function check_route_rl(route_key, route_count_key)
    local route_limit = tonumber(redis.call('GET', route_key))

    if route_limit == nil then
        return false, false, false, false
    end

    -- if route_limit == 0 then
    --     return 1, 0, 0, false
    -- end
    
    local route_count = tonumber(redis.call('INCR', route_count_key))
    
    if route_count == 1 then
        redis.call('EXPIRE', route_count_key, 60)
    end

    local route_leaky_bucket_ttl = redis.call('PTTL', route_key .. ':waiting_leaky')
    if route_leaky_bucket_ttl == -2 then
        route_leaky_bucket_ttl = false
    end

    if route_count > route_limit or route_leaky_bucket_ttl == false then
        return route_count, route_limit, 0, route_leaky_bucket_ttl
    end

    local reset_at = redis.call('PEXPIRETIME', route_count_key)

    -- error('route_remaining: ' .. tostring(route_remaining) .. ', route_limit: ' .. tostring(route_limit) .. ', reset_at: ' .. tostring(reset_at) .. ', route_waiting_leaky: ' .. tostring(route_waiting_leaky))
    return route_count, route_limit, reset_at, route_leaky_bucket_ttl
end

local function lock_bucket(key, token)
    local result = redis.call('SET', key .. ':lock', token, 'NX', 'EX', '5')
    return result ~= false
end

local function get_request_id()
    local request_id = redis.call('INCR', 'request_id')

    -- Reset before overflow
    if request_id == '9223372036854775806' then
        redis.call('SET', 'request_id', '0')
    end

    return request_id
end

local global_id = KEYS[1]
local time_slice = KEYS[2]

local global_time_slice_key = global_id .. time_slice
local global_time_slice_count_key = global_time_slice_key .. ':count'

local global_count, global_limit = check_global_rl(global_id, global_time_slice_key, global_time_slice_count_key)

if global_count > global_limit and global_limit ~= false then
    return {0, 0, global_limit, false, false, false, false, false, false}
end

local route_key = KEYS[3]
local route_count_key = route_key .. ':count'

local route_count, route_limit, route_reset_at, route_leaky_bucket_ttl = check_route_rl(route_key, route_count_key)

local global_lock = false
local route_lock = false

local request_id = get_request_id()

if route_count > route_limit and route_limit ~= false  then
    if route_leaky_bucket_ttl == false then
        route_lock = lock_bucket(route_key, request_id)

        if route_lock == false then
            if global_count == 1 then
                redis.call('DEL', global_time_slice_count_key)
            else
                redis.call('DECR', global_time_slice_count_key)
            end

            global_count = global_count - 1
        end

        redis.log(redis.LOG_NOTICE, '1')

        return {request_id, global_limit - global_count, global_limit, global_lock, 0, route_limit, route_reset_at, route_leaky_bucket_ttl, route_lock}
    else
        if global_count == 1 then
            redis.call('DEL', global_time_slice_count_key)
        else
            redis.call('DECR', global_time_slice_count_key)
        end
        
        redis.log(redis.LOG_NOTICE, '2')
        return {0, global_limit - global_count, global_limit, global_lock, 0, route_limit, route_reset_at, route_leaky_bucket_ttl, route_lock}
    end
end

if global_limit == false then
    global_lock = lock_bucket(global_id, request_id)
end

if route_limit == false then
    route_lock = lock_bucket(route_key, request_id)
end

local global_remaining
if global_limit ~= false then
    global_remaining = global_limit - global_count
else
    global_remaining = false
end

local route_remaining
if route_limit ~= false then
    route_remaining = route_limit - route_count
else
    route_remaining = false
end

redis.log(redis.LOG_NOTICE, '3')
return {request_id, global_remaining, global_limit, global_lock, route_remaining, route_limit, route_reset_at, route_leaky_bucket_ttl, route_lock}

-- if route_remaining == false then
--     route_lock = lock_bucket(route_key, request_id)

--     if route_lock == false then
--         redis.call('DECR', route_count_key)
--     end
-- elseif route_remaining < 0 then
--     redis.log(redis.LOG_NOTICE, 'got request over route limit - remaining: ' .. tostring(route_remaining) .. ', route_leaky_bucket_ttl: ' .. tostring(route_leaky_bucket_ttl))
--     route_lock = lock_bucket(route_key, request_id)

--     if route_lock == false then
--         redis.call('DECR', route_count_key)
--     else
--         redis.log(redis.LOG_NOTICE, 'request over route limit obtained the leaky lock')
--     end

--     if global_remaining ~= false then
--         local global_count = global_limit - global_remaining

--         if global_count == 1 then
--             local old_count = redis.call('GET', global_time_slice_count_key)
--             redis.call('DEL', global_time_slice_count_key)
--             redis.log(redis.LOG_NOTICE, 'DEL ' .. 'new_count: ' .. new_count .. ', global_remaining: ' .. tostring(global_remaining) .. ', global_count: ' .. tostring(global_count) .. ', global_limit: ' .. tostring(global_limit) .. ', global_lock: ' .. tostring(global_lock) .. ', route_remaining: ' .. tostring(route_remaining) .. ', route_limit: ' .. tostring(route_limit) .. ', route_reset_at: ' .. tostring(route_reset_at) .. ', route_leaky_bucket_ttl: ' .. tostring(route_leaky_bucket_ttl) .. ', route_lock: ' .. tostring(route_lock) .. ', request_id: ' .. request_id .. ', global_remaining: ' .. tostring(global_remaining) .. ', global_limit: ' .. tostring(global_limit) .. ', global_lock: ' .. tostring(global_lock))
--         else
--             local old_count = redis.call('GET', global_time_slice_count_key)
--             global_count = global_count - 1
--             redis.call('DECR', global_time_slice_count_key)
--             local new_count = redis.call('GET', global_time_slice_count_key)
--             -- global_remaining = global_remaining + 1
--             redis.log(redis.LOG_NOTICE, 'SET ' .. 'new_count: ' .. new_count .. ', old_count: ' .. old_count .. ', global_remaining: ' .. global_remaining .. ', global_count: ' .. tostring(global_count) .. ', global_limit: ' .. tostring(global_limit) .. ', global_lock: ' .. tostring(global_lock) .. ', route_remaining: ' .. tostring(route_remaining) .. ', route_limit: ' .. tostring(route_limit) .. ', route_reset_at: ' .. tostring(route_reset_at) .. ', route_leaky_bucket_ttl: ' .. tostring(route_leaky_bucket_ttl) .. ', route_lock: ' .. tostring(route_lock) .. ', request_id: ' .. request_id .. ', global_remaining: ' .. tostring(global_remaining) .. ', global_limit: ' .. tostring(global_limit) .. ', global_lock: ' .. tostring(global_lock))
--         end
--     else
--         redis.log(redis.LOG_NOTICE, 'RLed but not decrementing as has leaky lock - global_remaining: ' .. tostring(global_remaining) .. ', global_count: ' .. tostring(global_count) .. ', global_limit: ' .. tostring(global_limit) .. ', global_lock: ' .. tostring(global_lock) .. ', route_remaining: ' .. tostring(route_remaining) .. ', route_limit: ' .. tostring(route_limit) .. ', route_reset_at: ' .. tostring(route_reset_at) .. ', route_leaky_bucket_ttl: ' .. tostring(route_leaky_bucket_ttl) .. ', route_lock: ' .. tostring(route_lock) .. ', request_id: ' .. request_id .. ', global_remaining: ' .. tostring(global_remaining) .. ', global_limit: ' .. tostring(global_limit) .. ', global_lock: ' .. tostring(global_lock))
--     end
-- elseif route_remaining == 0 then
--     if global_remaining ~= false then
--         local global_count = global_limit - global_remaining

--         if global_count == 1 then
--             redis.call('DEL', global_time_slice_count_key)
--         else
--             redis.call('DECR', global_time_slice_count_key)
--             global_remaining = global_remaining + 1
--         end

--         -- redis.log(redis.LOG_NOTICE, 'ROUTE RATELIMITED remaining: ' .. tostring(route_remaining) .. ', route_limit: ' .. tostring(route_limit) .. ', route_reset_at: ' .. tostring(route_reset_at) .. ', route_leaky_bucket_ttl: ' .. tostring(route_leaky_bucket_ttl) .. ', route_lock: ' .. tostring(route_lock) .. ', route_count_key: ' .. route_count_key .. ', route_key: ' .. route_key .. ', request_id: ' .. request_id .. ', global_remaining: ' .. tostring(global_remaining) .. ', global_limit: ' .. tostring(global_limit) .. ', global_lock: ' .. tostring(global_lock) .. ', global_time_slice_count_key: ' .. global_time_slice_count_key .. ', global_time_slice_key: ' .. global_time_slice_key .. ', time_slice: ' .. time_slice .. ', global_id: ' .. global_id)
--     end

--     return {0, global_remaining, global_limit, false, route_remaining, route_limit, route_reset_at, route_leaky_bucket_ttl, false}
-- end

-- if request_id == false then
--     request_id = get_request_id()
-- end

-- redis.log(redis.LOG_NOTICE, 'request_id: ' .. request_id .. ', global_remaining: ' .. tostring(global_remaining) .. ', global_limit: ' .. tostring(global_limit) .. ', global_lock: ' .. tostring(global_lock) .. ', route_remaining: ' .. tostring(route_remaining) .. ', route_limit: ' .. tostring(route_limit) .. ', route_reset_at: ' .. tostring(route_reset_at) .. ', route_reset_after: ' .. tostring(route_leaky_bucket_ttl) .. ', route_lock: ' .. tostring(route_lock))
-- return {request_id, global_remaining, global_limit, global_lock, route_remaining, route_limit, route_reset_at, route_leaky_bucket_ttl, route_lock}

