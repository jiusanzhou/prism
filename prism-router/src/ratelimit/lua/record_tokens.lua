-- Record token usage for a rate limit key
-- KEYS[1] = token counter key
-- ARGV[1] = tokens to add
-- ARGV[2] = window_secs (for TTL alignment)
-- Returns: current total token count

local key = KEYS[1]
local tokens = tonumber(ARGV[1])
local window = tonumber(ARGV[2])

local current = redis.call('INCRBY', key, tokens)
-- Align TTL with the rate limit window
if current == tokens then
    redis.call('EXPIRE', key, window + 1)
end
return current
