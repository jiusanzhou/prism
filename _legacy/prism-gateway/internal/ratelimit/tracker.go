// Package ratelimit provides rate limit tracking from provider response headers.
//
// It reads rate limit information from HTTP response headers (OpenAI and Anthropic
// formats) and maintains real-time remaining quota per provider/account. This info
// can be used by the routing engine to avoid providers that are near their limits.
package ratelimit

import (
	"fmt"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Tracker reads rate limit info from HTTP response headers and maintains
// real-time remaining quota per provider/account.
type Tracker struct {
	mu     sync.RWMutex
	limits map[string]*AccountLimits // key: "provider_id:account_id"
}

// AccountLimits holds the rate limit state for a single provider/account pair.
type AccountLimits struct {
	// From response headers — total limits
	RateLimitRequests int // x-ratelimit-limit-requests
	RateLimitTokens   int // x-ratelimit-limit-tokens

	// From response headers — remaining
	RemainingRequests int       // x-ratelimit-remaining-requests
	RemainingTokens   int       // x-ratelimit-remaining-tokens
	ResetRequests     time.Time // x-ratelimit-reset-requests
	ResetTokens       time.Time // x-ratelimit-reset-tokens

	LastUpdated time.Time
}

// NewTracker creates a new rate limit tracker.
func NewTracker() *Tracker {
	return &Tracker{
		limits: make(map[string]*AccountLimits),
	}
}

// UpdateFromHeaders parses rate limit headers from a provider response and updates
// the tracker. It auto-detects OpenAI vs Anthropic header formats.
func (t *Tracker) UpdateFromHeaders(providerID, accountID string, headers http.Header) {
	if headers == nil {
		return
	}

	key := makeKey(providerID, accountID)
	limits := &AccountLimits{
		LastUpdated: time.Now(),
	}

	// Try OpenAI format first (x-ratelimit-*)
	if v := headers.Get("x-ratelimit-limit-requests"); v != "" {
		limits.RateLimitRequests = parseInt(v)
		limits.RemainingRequests = parseInt(headers.Get("x-ratelimit-remaining-requests"))
		limits.RateLimitTokens = parseInt(headers.Get("x-ratelimit-limit-tokens"))
		limits.RemainingTokens = parseInt(headers.Get("x-ratelimit-remaining-tokens"))
		limits.ResetRequests = parseResetTime(headers.Get("x-ratelimit-reset-requests"))
		limits.ResetTokens = parseResetTime(headers.Get("x-ratelimit-reset-tokens"))
	} else if v := headers.Get("anthropic-ratelimit-requests-limit"); v != "" {
		// Anthropic format (anthropic-ratelimit-*)
		limits.RateLimitRequests = parseInt(v)
		limits.RemainingRequests = parseInt(headers.Get("anthropic-ratelimit-requests-remaining"))
		limits.RateLimitTokens = parseInt(headers.Get("anthropic-ratelimit-tokens-limit"))
		limits.RemainingTokens = parseInt(headers.Get("anthropic-ratelimit-tokens-remaining"))
		limits.ResetRequests = parseISO8601(headers.Get("anthropic-ratelimit-requests-reset"))
		limits.ResetTokens = parseISO8601(headers.Get("anthropic-ratelimit-tokens-reset"))
	} else {
		// No rate limit headers found
		return
	}

	t.mu.Lock()
	t.limits[key] = limits
	t.mu.Unlock()
}

// GetRemaining returns the current rate limit state for a provider/account.
// Returns nil if no data has been recorded.
func (t *Tracker) GetRemaining(providerID, accountID string) *AccountLimits {
	key := makeKey(providerID, accountID)

	t.mu.RLock()
	defer t.mu.RUnlock()

	limits, ok := t.limits[key]
	if !ok {
		return nil
	}

	// Return a copy to avoid data races
	copy := *limits
	return &copy
}

// IsNearLimit returns true if remaining requests or tokens are below the
// given threshold percentage of the total limit.
// thresholdPct should be between 0.0 and 1.0 (e.g. 0.1 = 10%).
func (t *Tracker) IsNearLimit(providerID, accountID string, thresholdPct float64) bool {
	limits := t.GetRemaining(providerID, accountID)
	if limits == nil {
		return false // No data — assume not limited
	}

	// Check if limits have reset
	now := time.Now()
	if !limits.ResetRequests.IsZero() && now.After(limits.ResetRequests) &&
		!limits.ResetTokens.IsZero() && now.After(limits.ResetTokens) {
		return false // Limits have reset
	}

	// Check request limit
	if limits.RateLimitRequests > 0 {
		remaining := float64(limits.RemainingRequests) / float64(limits.RateLimitRequests)
		if remaining < thresholdPct {
			return true
		}
	}

	// Check token limit
	if limits.RateLimitTokens > 0 {
		remaining := float64(limits.RemainingTokens) / float64(limits.RateLimitTokens)
		if remaining < thresholdPct {
			return true
		}
	}

	return false
}

// Reset removes all tracked limits.
func (t *Tracker) Reset() {
	t.mu.Lock()
	t.limits = make(map[string]*AccountLimits)
	t.mu.Unlock()
}

// AllLimits returns a snapshot of all tracked limits.
func (t *Tracker) AllLimits() map[string]*AccountLimits {
	t.mu.RLock()
	defer t.mu.RUnlock()

	result := make(map[string]*AccountLimits, len(t.limits))
	for k, v := range t.limits {
		copy := *v
		result[k] = &copy
	}
	return result
}

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

func makeKey(providerID, accountID string) string {
	return providerID + ":" + accountID
}

// parseInt parses a string to int, returning 0 on failure.
func parseInt(s string) int {
	if s == "" {
		return 0
	}
	n, err := strconv.Atoi(s)
	if err != nil {
		return 0
	}
	return n
}

// parseResetTime parses a reset time value which can be in various formats:
// - Duration-like: "6ms", "1s", "2m30s", "1h"
// - ISO 8601: "2024-01-01T00:00:00Z"
func parseResetTime(s string) time.Time {
	if s == "" {
		return time.Time{}
	}

	// Try ISO 8601 first
	if t := parseISO8601(s); !t.IsZero() {
		return t
	}

	// Try duration format (e.g. "6ms", "1s", "2m30s")
	d, err := parseDuration(s)
	if err == nil {
		return time.Now().Add(d)
	}

	return time.Time{}
}

// parseISO8601 parses an ISO 8601 timestamp.
func parseISO8601(s string) time.Time {
	if s == "" {
		return time.Time{}
	}

	formats := []string{
		time.RFC3339,
		time.RFC3339Nano,
		"2006-01-02T15:04:05Z",
		"2006-01-02T15:04:05.000Z",
	}

	for _, format := range formats {
		if t, err := time.Parse(format, s); err == nil {
			return t
		}
	}

	return time.Time{}
}

// parseDuration parses a duration string like "6ms", "1s", "2m30s".
// This handles the OpenAI rate limit reset format which uses Go-like durations.
func parseDuration(s string) (time.Duration, error) {
	s = strings.TrimSpace(s)
	if s == "" {
		return 0, fmt.Errorf("empty duration")
	}

	// Go's time.ParseDuration handles "6ms", "1s", "2m30s", "1h" etc.
	return time.ParseDuration(s)
}
