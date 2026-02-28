package abtesting

import (
	"fmt"
	"testing"
)

func TestAssignExperiment(t *testing.T) {
	experiments := []Experiment{
		{
			ID:      "exp-1",
			Name:    "GPT-4 vs Claude",
			Enabled: true,
			TrafficSplit: map[string]int{
				"gpt4@openai":     50,
				"claude@anthropic": 50,
			},
			Target: ExperimentTarget{
				PoolIDs:    []string{"default"},
				SampleRate: 100,
			},
		},
	}

	mgr := NewManager(experiments)
	expID, variantID := mgr.Assign("req-123", "default", "", "")

	if expID != "exp-1" {
		t.Errorf("Expected experiment 'exp-1', got '%s'", expID)
	}
	if variantID == "" {
		t.Error("Expected a variant assignment, got empty")
	}
	// Must be one of the defined variants
	if variantID != "gpt4@openai" && variantID != "claude@anthropic" {
		t.Errorf("Unexpected variant: %s", variantID)
	}
}

func TestAssignConsistentHashing(t *testing.T) {
	experiments := []Experiment{
		{
			ID:      "exp-1",
			Enabled: true,
			TrafficSplit: map[string]int{
				"variant-a": 50,
				"variant-b": 50,
			},
			Target: ExperimentTarget{
				SampleRate: 100,
			},
		},
	}

	mgr := NewManager(experiments)

	// Same requestID should always get the same variant
	_, firstVariant := mgr.Assign("consistent-req-id", "", "", "")
	for i := 0; i < 100; i++ {
		_, variant := mgr.Assign("consistent-req-id", "", "", "")
		if variant != firstVariant {
			t.Fatalf("Consistent hashing broken: got %s vs %s on iteration %d", variant, firstVariant, i)
		}
	}
}

func TestAssignSampleRate(t *testing.T) {
	experiments := []Experiment{
		{
			ID:      "exp-sampled",
			Enabled: true,
			TrafficSplit: map[string]int{
				"variant-a": 100,
			},
			Target: ExperimentTarget{
				SampleRate: 50, // Only 50% of traffic
			},
		},
	}

	mgr := NewManager(experiments)

	matched := 0
	total := 1000
	for i := 0; i < total; i++ {
		reqID := "req-" + string(rune(i)) + "-" + t.Name()
		expID, _ := mgr.Assign(reqID, "", "", "")
		if expID != "" {
			matched++
		}
	}

	// Expect roughly 50% matched, allow 40-60% range
	ratio := float64(matched) / float64(total) * 100
	if ratio < 35 || ratio > 65 {
		t.Errorf("Sample rate 50%% but got %.1f%% matched (%d/%d)", ratio, matched, total)
	}
}

func TestAssignTargetFiltering(t *testing.T) {
	experiments := []Experiment{
		{
			ID:      "exp-pool",
			Enabled: true,
			TrafficSplit: map[string]int{
				"variant-a": 100,
			},
			Target: ExperimentTarget{
				PoolIDs:    []string{"premium"},
				SampleRate: 100,
			},
		},
		{
			ID:      "exp-tenant",
			Enabled: true,
			TrafficSplit: map[string]int{
				"variant-b": 100,
			},
			Target: ExperimentTarget{
				TenantIDs:  []string{"tenant-42"},
				SampleRate: 100,
			},
		},
		{
			ID:      "exp-source",
			Enabled: true,
			TrafficSplit: map[string]int{
				"variant-c": 100,
			},
			Target: ExperimentTarget{
				Sources:    []string{"mobile"},
				SampleRate: 100,
			},
		},
	}

	mgr := NewManager(experiments)

	// Pool match
	expID, _ := mgr.Assign("req-1", "premium", "", "")
	if expID != "exp-pool" {
		t.Errorf("Expected exp-pool for pool 'premium', got '%s'", expID)
	}

	// Tenant match
	expID, _ = mgr.Assign("req-2", "", "tenant-42", "")
	if expID != "exp-tenant" {
		t.Errorf("Expected exp-tenant for tenant 'tenant-42', got '%s'", expID)
	}

	// Source match
	expID, _ = mgr.Assign("req-3", "", "", "mobile")
	if expID != "exp-source" {
		t.Errorf("Expected exp-source for source 'mobile', got '%s'", expID)
	}
}

func TestAssignNoMatch(t *testing.T) {
	experiments := []Experiment{
		{
			ID:      "exp-1",
			Enabled: true,
			TrafficSplit: map[string]int{
				"variant-a": 100,
			},
			Target: ExperimentTarget{
				PoolIDs:    []string{"premium"},
				SampleRate: 100,
			},
		},
	}

	mgr := NewManager(experiments)

	// Request for wrong pool
	expID, variantID := mgr.Assign("req-1", "default", "", "")
	if expID != "" || variantID != "" {
		t.Errorf("Expected no match, got exp='%s' variant='%s'", expID, variantID)
	}

	// Disabled experiment
	experiments[0].Enabled = false
	mgr = NewManager(experiments)
	expID, variantID = mgr.Assign("req-1", "premium", "", "")
	if expID != "" || variantID != "" {
		t.Errorf("Expected no match for disabled experiment, got exp='%s' variant='%s'", expID, variantID)
	}
}

func TestListExperiments(t *testing.T) {
	experiments := []Experiment{
		{ID: "exp-1", Name: "Experiment 1", Enabled: true},
		{ID: "exp-2", Name: "Experiment 2", Enabled: false},
		{ID: "exp-3", Name: "Experiment 3", Enabled: true},
	}

	mgr := NewManager(experiments)
	list := mgr.ListExperiments()

	if len(list) != 3 {
		t.Errorf("Expected 3 experiments, got %d", len(list))
	}

	// Verify it's a copy
	list[0].Name = "Modified"
	if mgr.experiments[0].Name == "Modified" {
		t.Error("ListExperiments should return a copy, not a reference")
	}
}

func TestTrafficSplitDistribution(t *testing.T) {
	experiments := []Experiment{
		{
			ID:      "exp-dist",
			Enabled: true,
			TrafficSplit: map[string]int{
				"variant-a": 50,
				"variant-b": 50,
			},
			Target: ExperimentTarget{
				SampleRate: 100,
			},
		},
	}

	mgr := NewManager(experiments)

	counts := map[string]int{"variant-a": 0, "variant-b": 0}
	total := 10000
	for i := 0; i < total; i++ {
		reqID := fmt.Sprintf("dist-req-%d", i)
		_, variant := mgr.Assign(reqID, "", "", "")
		counts[variant]++
	}

	// With 10000 requests and 50/50 split, each variant should get 40-60%
	for v, count := range counts {
		ratio := float64(count) / float64(total) * 100
		if ratio < 40 || ratio > 60 {
			t.Errorf("Variant %s got %.1f%% (%d/%d), expected ~50%%", v, ratio, count, total)
		}
	}
}

func TestGetExperiment(t *testing.T) {
	experiments := []Experiment{
		{ID: "exp-1", Name: "First"},
		{ID: "exp-2", Name: "Second"},
	}

	mgr := NewManager(experiments)

	exp := mgr.GetExperiment("exp-1")
	if exp == nil {
		t.Fatal("Expected to find exp-1")
	}
	if exp.Name != "First" {
		t.Errorf("Expected name 'First', got '%s'", exp.Name)
	}

	exp = mgr.GetExperiment("nonexistent")
	if exp != nil {
		t.Error("Expected nil for nonexistent experiment")
	}
}
