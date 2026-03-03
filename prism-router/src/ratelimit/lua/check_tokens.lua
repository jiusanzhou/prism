-- Check token limit for a rate limit key
-- KEYS[1] = token counter key
-- ARGV[1] = max_tokens
-- Returns: [allowed (0/1), current_tokens]

local key = KEYS[1]
local max_tokens = tonumber(ARGV[1])

local current = tonumber(redis.call('GET', key) or '0')
if current >= max_tokens then
    return {0, current}
end
return {1, current}
