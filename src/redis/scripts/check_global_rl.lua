-- I don't think we need this, but its here incase/for reference

local function increment_global_count(key)
    local global_count = redis.call('INCR', key)

    if global_count == 1 then
        redis.call('EXPIRE', key, 3)
    end

    return global_count
end

local global_key = KEYS[1]
local time_slice = KEYS[2]
local global_count_key = global_key .. time_slice

local lock_token = ARGV[1]

local global_limit = tonumber(redis.call('GET', global_key))

local holds_global_lock = false
if global_limit == nil then
    holds_global_lock = lock_bucket(global_key, lock_token)

    if holds_global_lock == false then
        return 1
    end
end

local global_count = increment_global_count(global_count_key)

if global_count > global_limit then
    return {0, global_limit}
end

local holds_route_lock = false
return {5, holds_global_lock, holds_route_lock}