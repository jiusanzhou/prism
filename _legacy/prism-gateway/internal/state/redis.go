package state

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"time"

	"github.com/google/uuid"
	"github.com/redis/go-redis/v9"
)

// RedisStore implements StateStore using Redis for distributed state.
type RedisStore struct {
	client *redis.Client
}

// NewRedisStore creates a new Redis-backed state store.
func NewRedisStore(addr, password string, db int) (*RedisStore, error) {
	client := redis.NewClient(&redis.Options{
		Addr:     addr,
		Password: password,
		DB:       db,
	})

	// Ping to verify connectivity
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := client.Ping(ctx).Err(); err != nil {
		return nil, fmt.Errorf("redis: failed to connect to %s: %w", addr, err)
	}

	return &RedisStore{client: client}, nil
}

func metricsKey(variantID string) string {
	return fmt.Sprintf("prism:variant:%s:metrics", variantID)
}

func requestsKey(variantID string) string {
	return fmt.Sprintf("prism:variant:%s:requests", variantID)
}

func latencyKey(variantID string) string {
	return fmt.Sprintf("prism:variant:%s:latency", variantID)
}

func errorsKey(variantID string) string {
	return fmt.Sprintf("prism:variant:%s:errors", variantID)
}

func (r *RedisStore) UpdateVariantMetrics(ctx context.Context, variantID string, metrics *VariantMetrics) error {
	data, err := json.Marshal(metrics)
	if err != nil {
		return err
	}

	key := metricsKey(variantID)
	pipe := r.client.Pipeline()
	pipe.Set(ctx, key, data, 5*time.Minute) // TTL for auto-cleanup
	_, err = pipe.Exec(ctx)
	return err
}

func (r *RedisStore) GetVariantMetrics(ctx context.Context, variantID string) (*VariantMetrics, error) {
	now := time.Now()

	// Compute QPS from sorted set
	qpsWindowStart := float64(now.Add(-qpsWindow).UnixNano())
	qpsWindowEnd := float64(now.UnixNano())
	qpsCount, err := r.client.ZCount(ctx, requestsKey(variantID),
		strconv.FormatFloat(qpsWindowStart, 'f', 0, 64),
		strconv.FormatFloat(qpsWindowEnd, 'f', 0, 64)).Result()
	if err != nil && err != redis.Nil {
		return nil, err
	}

	// Compute error rate
	errWindowStart := float64(now.Add(-errorWindow).UnixNano())
	errWindowEnd := float64(now.UnixNano())

	totalCount, _ := r.client.ZCount(ctx, requestsKey(variantID),
		strconv.FormatFloat(errWindowStart, 'f', 0, 64),
		strconv.FormatFloat(errWindowEnd, 'f', 0, 64)).Result()
	errCount, _ := r.client.ZCount(ctx, errorsKey(variantID),
		strconv.FormatFloat(errWindowStart, 'f', 0, 64),
		strconv.FormatFloat(errWindowEnd, 'f', 0, 64)).Result()

	var errorRate float32
	if totalCount > 0 {
		errorRate = float32(errCount) / float32(totalCount)
	}

	// Get latency samples
	latSamples, _ := r.client.LRange(ctx, latencyKey(variantID), 0, -1).Result()
	samples := make([]int64, 0, len(latSamples))
	for _, s := range latSamples {
		if v, err := strconv.ParseInt(s, 10, 64); err == nil {
			samples = append(samples, v)
		}
	}

	return &VariantMetrics{
		CurrentQPS:  float32(qpsCount) / float32(qpsWindow.Seconds()),
		ErrorRate:   errorRate,
		P50Latency:  computePercentile(samples, 0.50),
		P95Latency:  computePercentile(samples, 0.95),
		P99Latency:  computePercentile(samples, 0.99),
		TTFTP50:     0,
		QueueDepth:  0,
		LastUpdated: now,
	}, nil
}

func (r *RedisStore) IncrementQPS(ctx context.Context, variantID string) error {
	now := time.Now()
	member := uuid.New().String()
	score := float64(now.UnixNano())

	pipe := r.client.Pipeline()
	pipe.ZAdd(ctx, requestsKey(variantID), redis.Z{Score: score, Member: member})
	// Trim entries older than errorWindow
	cutoff := float64(now.Add(-errorWindow).UnixNano())
	pipe.ZRemRangeByScore(ctx, requestsKey(variantID), "-inf", strconv.FormatFloat(cutoff, 'f', 0, 64))
	pipe.Expire(ctx, requestsKey(variantID), 2*errorWindow)
	_, err := pipe.Exec(ctx)
	return err
}

func (r *RedisStore) RecordLatency(ctx context.Context, variantID string, latencyMs int64) error {
	key := latencyKey(variantID)
	pipe := r.client.Pipeline()
	pipe.RPush(ctx, key, latencyMs)
	pipe.LTrim(ctx, key, -maxLatencySamples, -1)
	pipe.Expire(ctx, key, 10*time.Minute)
	_, err := pipe.Exec(ctx)
	return err
}

func (r *RedisStore) RecordError(ctx context.Context, variantID string) error {
	now := time.Now()
	member := uuid.New().String()
	score := float64(now.UnixNano())

	pipe := r.client.Pipeline()
	pipe.ZAdd(ctx, errorsKey(variantID), redis.Z{Score: score, Member: member})
	cutoff := float64(now.Add(-errorWindow).UnixNano())
	pipe.ZRemRangeByScore(ctx, errorsKey(variantID), "-inf", strconv.FormatFloat(cutoff, 'f', 0, 64))
	pipe.Expire(ctx, errorsKey(variantID), 2*errorWindow)
	_, err := pipe.Exec(ctx)
	return err
}

func (r *RedisStore) GetAllMetrics(ctx context.Context) (map[string]*VariantMetrics, error) {
	// Scan for all variant keys
	result := make(map[string]*VariantMetrics)

	var cursor uint64
	for {
		keys, nextCursor, err := r.client.Scan(ctx, cursor, "prism:variant:*:requests", 100).Result()
		if err != nil {
			return nil, err
		}

		for _, key := range keys {
			// Extract variant ID from key
			// key format: prism:variant:{id}:requests
			variantID := extractVariantID(key)
			if variantID == "" {
				continue
			}

			metrics, err := r.GetVariantMetrics(ctx, variantID)
			if err != nil {
				continue // skip on error
			}
			result[variantID] = metrics
		}

		cursor = nextCursor
		if cursor == 0 {
			break
		}
	}

	return result, nil
}

func (r *RedisStore) Close() error {
	return r.client.Close()
}

// extractVariantID parses the variant ID from a key like "prism:variant:{id}:requests".
func extractVariantID(key string) string {
	// prism:variant:VARIANT_ID:requests
	const prefix = "prism:variant:"
	const suffix = ":requests"
	if len(key) <= len(prefix)+len(suffix) {
		return ""
	}
	return key[len(prefix) : len(key)-len(suffix)]
}
