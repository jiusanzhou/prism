package router

import (
	"encoding/json"
	"testing"
)

func TestBuildStrategyJSON_Balanced(t *testing.T) {
	costW := float32(0.2)
	qualW := float32(0.6)
	latW := float32(0.2)

	req := &RouteRequest{
		Strategy:      "balanced",
		CostWeight:    &costW,
		QualityWeight: &qualW,
		LatencyWeight: &latW,
	}

	data, err := buildStrategyJSON(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var result map[string]interface{}
	if err := json.Unmarshal(data, &result); err != nil {
		t.Fatalf("invalid JSON: %v", err)
	}

	auto, ok := result["Auto"].(map[string]interface{})
	if !ok {
		t.Fatalf("expected Auto key, got %+v", result)
	}

	obj, ok := auto["objective"].(map[string]interface{})
	if !ok {
		t.Fatalf("expected objective key")
	}

	balanced, ok := obj["Balanced"].(map[string]interface{})
	if !ok {
		t.Fatalf("expected Balanced objective, got %+v", obj)
	}

	if cw := balanced["cost_weight"].(float64); cw < 0.19 || cw > 0.21 {
		t.Errorf("expected cost_weight ~0.2, got %f", cw)
	}
	if qw := balanced["quality_weight"].(float64); qw < 0.59 || qw > 0.61 {
		t.Errorf("expected quality_weight ~0.6, got %f", qw)
	}
}

func TestBuildStrategyJSON_BalancedDefaults(t *testing.T) {
	req := &RouteRequest{
		Strategy: "balanced",
	}

	data, err := buildStrategyJSON(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var result map[string]interface{}
	json.Unmarshal(data, &result)

	auto := result["Auto"].(map[string]interface{})
	obj := auto["objective"].(map[string]interface{})
	balanced := obj["Balanced"].(map[string]interface{})

	if cw := balanced["cost_weight"].(float64); cw < 0.29 || cw > 0.31 {
		t.Errorf("expected default cost_weight ~0.3, got %f", cw)
	}
}

func TestBuildStrategyJSON_CostFirst(t *testing.T) {
	req := &RouteRequest{
		Strategy: "cost_first",
	}

	data, err := buildStrategyJSON(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var result map[string]interface{}
	json.Unmarshal(data, &result)

	auto := result["Auto"].(map[string]interface{})
	obj := auto["objective"].(map[string]interface{})

	if _, ok := obj["CostFirst"]; !ok {
		t.Errorf("expected CostFirst objective, got %+v", obj)
	}
}

func TestBuildStrategyJSON_QualityFirst(t *testing.T) {
	req := &RouteRequest{
		Strategy: "quality_first",
	}

	data, err := buildStrategyJSON(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var result map[string]interface{}
	json.Unmarshal(data, &result)

	auto := result["Auto"].(map[string]interface{})
	obj := auto["objective"].(map[string]interface{})

	if _, ok := obj["QualityFirst"]; !ok {
		t.Errorf("expected QualityFirst objective, got %+v", obj)
	}
}

func TestBuildStrategyJSON_SloFirst(t *testing.T) {
	maxLat := uint32(200)
	maxTtft := uint32(100)

	req := &RouteRequest{
		Strategy:     "slo_first",
		MaxLatencyMs: &maxLat,
		MaxTTFTMs:    &maxTtft,
	}

	data, err := buildStrategyJSON(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var result map[string]interface{}
	json.Unmarshal(data, &result)

	auto := result["Auto"].(map[string]interface{})
	obj := auto["objective"].(map[string]interface{})
	slo := obj["SloFirst"].(map[string]interface{})

	if ml := slo["max_latency_ms"].(float64); ml != 200 {
		t.Errorf("expected max_latency_ms 200, got %f", ml)
	}
	if mt := slo["max_ttft_ms"].(float64); mt != 100 {
		t.Errorf("expected max_ttft_ms 100, got %f", mt)
	}
}

func TestBuildStrategyJSON_Pinned(t *testing.T) {
	req := &RouteRequest{
		Strategy: "pinned:gpt-4o",
	}

	data, err := buildStrategyJSON(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var result map[string]interface{}
	json.Unmarshal(data, &result)

	pinned, ok := result["Pinned"].(map[string]interface{})
	if !ok {
		t.Fatalf("expected Pinned key, got %+v", result)
	}

	if mid := pinned["model_id"].(string); mid != "gpt-4o" {
		t.Errorf("expected model_id 'gpt-4o', got '%s'", mid)
	}
}

func TestBuildStrategyJSON_WithPoolIDs(t *testing.T) {
	req := &RouteRequest{
		Strategy: "cost_first",
		PoolIDs:  []string{"tier-1", "tier-2"},
	}

	data, err := buildStrategyJSON(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var result map[string]interface{}
	json.Unmarshal(data, &result)

	auto := result["Auto"].(map[string]interface{})
	pools, ok := auto["pool_ids"].([]interface{})
	if !ok {
		t.Fatalf("expected pool_ids array, got %+v", auto["pool_ids"])
	}
	if len(pools) != 2 {
		t.Errorf("expected 2 pool_ids, got %d", len(pools))
	}
}

func TestBuildStrategyJSON_EmptyStrategy(t *testing.T) {
	req := &RouteRequest{}

	data, err := buildStrategyJSON(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if data != nil {
		t.Errorf("expected nil for empty strategy, got %s", string(data))
	}
}

func TestBuildAnalyzeContextJSON(t *testing.T) {
	req := &RouteRequest{
		CallerType: "service",
		Source:     "mobile",
		TenantID:   "tenant-42",
	}

	data, err := buildAnalyzeContextJSON(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var result map[string]interface{}
	json.Unmarshal(data, &result)

	if ct := result["caller_type"].(string); ct != "service" {
		t.Errorf("expected caller_type 'service', got '%s'", ct)
	}
	if src := result["source"].(string); src != "mobile" {
		t.Errorf("expected source 'mobile', got '%s'", src)
	}
	if tid := result["tenant_id"].(string); tid != "tenant-42" {
		t.Errorf("expected tenant_id 'tenant-42', got '%s'", tid)
	}
}

func TestBuildAnalyzeContextJSON_Empty(t *testing.T) {
	req := &RouteRequest{}

	data, err := buildAnalyzeContextJSON(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if data != nil {
		t.Errorf("expected nil for empty context, got %s", string(data))
	}
}

func TestBuildAnalyzeContextJSON_WithTags(t *testing.T) {
	req := &RouteRequest{
		CallerType: "user",
		Tags:       map[string]string{"env": "production", "team": "ml"},
	}

	data, err := buildAnalyzeContextJSON(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var result map[string]interface{}
	json.Unmarshal(data, &result)

	tags, ok := result["tags"].(map[string]interface{})
	if !ok {
		t.Fatalf("expected tags map")
	}
	if tags["env"] != "production" {
		t.Errorf("expected tag env=production, got %v", tags["env"])
	}
}

func TestBuildStrategyJSON_RustEnumFormat(t *testing.T) {
	// Verify the output exactly matches what Rust expects to deserialize
	tests := []struct {
		strategy    string
		expectKey   string // top-level key in the JSON
	}{
		{"balanced", "Auto"},
		{"auto", "Auto"},
		{"cost_first", "Auto"},
		{"quality_first", "Auto"},
		{"slo_first", "Auto"},
		{"pinned:gpt-4o", "Pinned"},
	}

	for _, tt := range tests {
		t.Run(tt.strategy, func(t *testing.T) {
			req := &RouteRequest{Strategy: tt.strategy}
			data, err := buildStrategyJSON(req)
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}

			var result map[string]interface{}
			if err := json.Unmarshal(data, &result); err != nil {
				t.Fatalf("invalid JSON: %v", err)
			}

			if _, ok := result[tt.expectKey]; !ok {
				t.Errorf("expected top-level key '%s', got keys %+v", tt.expectKey, result)
			}

			// Must have exactly one top-level key (Rust tagged enum)
			if len(result) != 1 {
				t.Errorf("expected exactly 1 top-level key, got %d: %+v", len(result), result)
			}
		})
	}
}
