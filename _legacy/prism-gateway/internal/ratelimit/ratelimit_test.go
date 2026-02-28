package ratelimit

import (
	"fmt"
	"net/http"
	"sync"
	"testing"
	"time"
)

func TestParseOpenAIHeaders(t *testing.T) {
	tracker := NewTracker()

	headers := http.Header{}
	headers.Set("x-ratelimit-limit-requests", "10000")
	headers.Set("x-ratelimit-remaining-requests", "9999")
	headers.Set("x-ratelimit-reset-requests", "6ms")
	headers.Set("x-ratelimit-limit-tokens", "2000000")
	headers.Set("x-ratelimit-remaining-tokens", "1999950")
	headers.Set("x-ratelimit-reset-tokens", "1ms")

	tracker.UpdateFromHeaders("openai", "acct-1", headers)

	limits := tracker.GetRemaining("openai", "acct-1")
	if limits == nil {
		t.Fatal("expected non-nil limits")
	}

	if limits.RateLimitRequests != 10000 {
		t.Errorf("RateLimitRequests = %d, want 10000", limits.RateLimitRequests)
	}
	if limits.RemainingRequests != 9999 {
		t.Errorf("RemainingRequests = %d, want 9999", limits.RemainingRequests)
	}
	if limits.RateLimitTokens != 2000000 {
		t.Errorf("RateLimitTokens = %d, want 2000000", limits.RateLimitTokens)
	}
	if limits.RemainingTokens != 1999950 {
		t.Errorf("RemainingTokens = %d, want 1999950", limits.RemainingTokens)
	}
	if limits.ResetRequests.IsZero() {
		t.Error("ResetRequests should not be zero")
	}
	if limits.ResetTokens.IsZero() {
		t.Error("ResetTokens should not be zero")
	}
	if limits.LastUpdated.IsZero() {
		t.Error("LastUpdated should not be zero")
	}
}

func TestParseAnthropicHeaders(t *testing.T) {
	tracker := NewTracker()

	headers := http.Header{}
	headers.Set("anthropic-ratelimit-requests-limit", "1000")
	headers.Set("anthropic-ratelimit-requests-remaining", "999")
	headers.Set("anthropic-ratelimit-requests-reset", "2025-01-15T00:00:00Z")
	headers.Set("anthropic-ratelimit-tokens-limit", "100000")
	headers.Set("anthropic-ratelimit-tokens-remaining", "99900")
	headers.Set("anthropic-ratelimit-tokens-reset", "2025-01-15T00:00:00Z")

	tracker.UpdateFromHeaders("anthropic", "acct-1", headers)

	limits := tracker.GetRemaining("anthropic", "acct-1")
	if limits == nil {
		t.Fatal("expected non-nil limits")
	}

	if limits.RateLimitRequests != 1000 {
		t.Errorf("RateLimitRequests = %d, want 1000", limits.RateLimitRequests)
	}
	if limits.RemainingRequests != 999 {
		t.Errorf("RemainingRequests = %d, want 999", limits.RemainingRequests)
	}
	if limits.RateLimitTokens != 100000 {
		t.Errorf("RateLimitTokens = %d, want 100000", limits.RateLimitTokens)
	}
	if limits.RemainingTokens != 99900 {
		t.Errorf("RemainingTokens = %d, want 99900", limits.RemainingTokens)
	}

	expectedReset := time.Date(2025, 1, 15, 0, 0, 0, 0, time.UTC)
	if !limits.ResetRequests.Equal(expectedReset) {
		t.Errorf("ResetRequests = %v, want %v", limits.ResetRequests, expectedReset)
	}
}

func TestIsNearLimit(t *testing.T) {
	tracker := NewTracker()

	// Provider with plenty of remaining capacity
	headers := http.Header{}
	headers.Set("x-ratelimit-limit-requests", "10000")
	headers.Set("x-ratelimit-remaining-requests", "9000")
	headers.Set("x-ratelimit-reset-requests", "1h")
	headers.Set("x-ratelimit-limit-tokens", "2000000")
	headers.Set("x-ratelimit-remaining-tokens", "1800000")
	headers.Set("x-ratelimit-reset-tokens", "1h")

	tracker.UpdateFromHeaders("openai", "acct-1", headers)

	// 90% remaining — should NOT be near limit at 10% threshold
	if tracker.IsNearLimit("openai", "acct-1", 0.10) {
		t.Error("expected NOT near limit with 90% remaining at 10% threshold")
	}

	// Provider near its limits
	headers2 := http.Header{}
	headers2.Set("x-ratelimit-limit-requests", "10000")
	headers2.Set("x-ratelimit-remaining-requests", "50")
	headers2.Set("x-ratelimit-reset-requests", "1h")
	headers2.Set("x-ratelimit-limit-tokens", "2000000")
	headers2.Set("x-ratelimit-remaining-tokens", "1000")
	headers2.Set("x-ratelimit-reset-tokens", "1h")

	tracker.UpdateFromHeaders("openai", "acct-2", headers2)

	// 0.5% remaining — should be near limit at 10% threshold
	if !tracker.IsNearLimit("openai", "acct-2", 0.10) {
		t.Error("expected near limit with 0.5% remaining at 10% threshold")
	}

	// Unknown provider — should NOT be near limit
	if tracker.IsNearLimit("unknown", "acct-1", 0.10) {
		t.Error("expected NOT near limit for unknown provider")
	}
}

func TestConcurrentUpdates(t *testing.T) {
	tracker := NewTracker()

	var wg sync.WaitGroup
	numGoroutines := 100

	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(n int) {
			defer wg.Done()

			providerID := fmt.Sprintf("provider-%d", n%5)
			accountID := fmt.Sprintf("acct-%d", n%3)

			headers := http.Header{}
			headers.Set("x-ratelimit-limit-requests", fmt.Sprintf("%d", 10000-n))
			headers.Set("x-ratelimit-remaining-requests", fmt.Sprintf("%d", 9000-n))
			headers.Set("x-ratelimit-reset-requests", "1s")
			headers.Set("x-ratelimit-limit-tokens", "2000000")
			headers.Set("x-ratelimit-remaining-tokens", "1800000")
			headers.Set("x-ratelimit-reset-tokens", "1s")

			tracker.UpdateFromHeaders(providerID, accountID, headers)

			// Also read concurrently
			_ = tracker.GetRemaining(providerID, accountID)
			_ = tracker.IsNearLimit(providerID, accountID, 0.10)
		}(i)
	}

	wg.Wait()

	// Verify we can still read after concurrent updates
	all := tracker.AllLimits()
	if len(all) == 0 {
		t.Error("expected some limits to be tracked after concurrent updates")
	}
}

func TestGetRemainingNonExistent(t *testing.T) {
	tracker := NewTracker()
	limits := tracker.GetRemaining("nonexistent", "nonexistent")
	if limits != nil {
		t.Errorf("expected nil for nonexistent provider, got %v", limits)
	}
}

func TestNilHeaders(t *testing.T) {
	tracker := NewTracker()
	// Should not panic
	tracker.UpdateFromHeaders("openai", "acct-1", nil)

	limits := tracker.GetRemaining("openai", "acct-1")
	if limits != nil {
		t.Error("expected nil limits for nil headers")
	}
}

func TestNoRateLimitHeaders(t *testing.T) {
	tracker := NewTracker()
	headers := http.Header{}
	headers.Set("Content-Type", "application/json")
	headers.Set("X-Request-Id", "abc123")

	tracker.UpdateFromHeaders("openai", "acct-1", headers)

	limits := tracker.GetRemaining("openai", "acct-1")
	if limits != nil {
		t.Error("expected nil limits for headers without rate limit info")
	}
}

func TestReset(t *testing.T) {
	tracker := NewTracker()

	headers := http.Header{}
	headers.Set("x-ratelimit-limit-requests", "10000")
	headers.Set("x-ratelimit-remaining-requests", "9999")
	headers.Set("x-ratelimit-reset-requests", "1s")
	headers.Set("x-ratelimit-limit-tokens", "2000000")
	headers.Set("x-ratelimit-remaining-tokens", "1999950")
	headers.Set("x-ratelimit-reset-tokens", "1s")

	tracker.UpdateFromHeaders("openai", "acct-1", headers)

	if tracker.GetRemaining("openai", "acct-1") == nil {
		t.Fatal("expected non-nil before reset")
	}

	tracker.Reset()

	if tracker.GetRemaining("openai", "acct-1") != nil {
		t.Error("expected nil after reset")
	}
}
