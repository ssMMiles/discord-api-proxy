local route_key = KEYS[1]
local route_lock_key = route_key .. ':lock'

local lock_token = ARGV[1]
local route_limit = ARGV[2]
local route_remaining = ARGV[3]
local route_reset_at = ARGV[4]
local route_reset_after = ARGV[5]
local route_info_expire_in = ARGV[6]

local route_count_key = route_key .. ':count'
local route_reset_after_key = route_key .. ':reset_after'

if lock_token ~= '' then
    local current_lock_holder = redis.call('GET', route_lock_key)

    if lock_token == current_lock_holder then
        if route_info_expire_in == '0' then
            redis.call('SET', route_key, route_limit)
        else
            redis.call('SET', route_key, route_limit, 'PX', route_info_expire_in)
        end

        redis.call('SET', route_count_key, route_limit - route_remaining, 'PXAT', route_reset_at)
        redis.call('SET', route_reset_after_key, '1', 'PX', route_reset_after)

        redis.call('DEL', route_lock_key)
        redis.call('PUBLISH', 'unlock', route_key)

        return true
    end
else
    local result = redis.call('PEXPIREAT', route_count_key, route_reset_at, 'GT')

    if result == 1 then
        redis.call('SET', route_count_key, route_limit - route_remaining, 'KEEPTTL')
        redis.call('SET', route_reset_after_key, '1', 'PX', route_reset_after)

        return true
    end
end

return false