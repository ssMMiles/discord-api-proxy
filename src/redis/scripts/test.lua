local result = redis.call('MGET', 'global:{972740784501698650}', 'global:{972740784501698650}-route:users/@me')

local global_limit = tonumber(result[1])
local global_route_limit = tonumber(result[2])

return {global_limit, global_route_limit}