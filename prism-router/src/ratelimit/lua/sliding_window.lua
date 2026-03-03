-- Sliding window rate limiter (Redis Sorted Set)
-- KEYS[1] = rate limit key
-- ARGV[1] = window_secs
-- ARGV[2] = max_requests
-- ARGV[3] = now (unix secs)
-- Returns: [allowed (0/1), current_count, retry_after_secs]

local key = KEYS[1]
local window = tonumber(ARGV[1])
local limit = tonumber(ARGV[2])
local now = tonumber(ARGV[3])

-- Remove expired entries
local window_start = now - window
redis.call('ZREMRANGEBYSCORE', key, '-inf', window_start)

local count = redis.call('ZCARD', key)
if count >= limit then
    local oldest = redis.call('ZRANGEBYSCORE', key, '-inf', '+inf', 'LIMIT', 0, 1)
    local retry = 0
    if #oldest > 0 then
        local oldest_score = tonumber(redis.call('ZSCORE', key, oldest[1]))
        retry = math.max(0, math.ceil(window - (now - oldest_score)))
    end
    return {0, count, retry}
end

-- Add this request (unique member = timestamp:random)
redis.call('ZADD', key, now, now .. ':' .. math.random(1000000))
redis.call('EXPIRE', key, window + 1)
return {1, count + 1, 0}
