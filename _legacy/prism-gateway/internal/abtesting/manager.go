package abtesting

import (
	"hash/fnv"
	"sort"
)

// Manager manages A/B test experiments and assigns variants to requests.
type Manager struct {
	experiments []Experiment
}

// NewManager creates a new experiment manager with the given experiments.
func NewManager(experiments []Experiment) *Manager {
	return &Manager{
		experiments: experiments,
	}
}

// Assign returns the experiment ID and variant assignment for a request.
// Returns empty strings if no experiment applies.
// Uses consistent hashing on requestID for stable assignment.
func (m *Manager) Assign(requestID string, poolID string, tenantID string, source string) (experimentID string, variantID string) {
	for _, exp := range m.experiments {
		if !exp.Enabled {
			continue
		}

		// Check if request matches experiment target
		if !m.matchesTarget(&exp, poolID, tenantID, source) {
			continue
		}

		// Check sample rate using hash of requestID
		if !m.inSample(requestID, exp.ID, exp.Target.SampleRate) {
			continue
		}

		// Assign variant based on traffic split
		variant := m.assignVariant(requestID, exp.ID, exp.TrafficSplit)
		if variant != "" {
			return exp.ID, variant
		}
	}

	return "", ""
}

// GetExperiment returns an experiment by ID, or nil if not found.
func (m *Manager) GetExperiment(id string) *Experiment {
	for i := range m.experiments {
		if m.experiments[i].ID == id {
			return &m.experiments[i]
		}
	}
	return nil
}

// ListExperiments returns all experiments.
func (m *Manager) ListExperiments() []Experiment {
	result := make([]Experiment, len(m.experiments))
	copy(result, m.experiments)
	return result
}

// matchesTarget checks if a request matches the experiment's target criteria.
func (m *Manager) matchesTarget(exp *Experiment, poolID string, tenantID string, source string) bool {
	// If pool_ids is specified, poolID must match
	if len(exp.Target.PoolIDs) > 0 {
		found := false
		for _, pid := range exp.Target.PoolIDs {
			if pid == poolID {
				found = true
				break
			}
		}
		if !found {
			return false
		}
	}

	// If tenant_ids is specified, tenantID must match
	if len(exp.Target.TenantIDs) > 0 {
		found := false
		for _, tid := range exp.Target.TenantIDs {
			if tid == tenantID {
				found = true
				break
			}
		}
		if !found {
			return false
		}
	}

	// If sources is specified, source must match
	if len(exp.Target.Sources) > 0 {
		found := false
		for _, s := range exp.Target.Sources {
			if s == source {
				found = true
				break
			}
		}
		if !found {
			return false
		}
	}

	return true
}

// inSample checks if a request falls within the experiment's sample rate.
func (m *Manager) inSample(requestID string, experimentID string, sampleRate int) bool {
	if sampleRate >= 100 {
		return true
	}
	if sampleRate <= 0 {
		return false
	}

	h := fnv.New32a()
	h.Write([]byte("sample:" + experimentID + ":" + requestID))
	hash := h.Sum32()

	return int(hash%100) < sampleRate
}

// assignVariant assigns a variant based on the traffic split using consistent hashing.
func (m *Manager) assignVariant(requestID string, experimentID string, trafficSplit map[string]int) string {
	if len(trafficSplit) == 0 {
		return ""
	}

	// Sort variants for deterministic assignment
	variants := make([]string, 0, len(trafficSplit))
	for v := range trafficSplit {
		variants = append(variants, v)
	}
	sort.Strings(variants)

	// Hash requestID + experimentID for consistent assignment
	h := fnv.New32a()
	h.Write([]byte("assign:" + experimentID + ":" + requestID))
	hash := h.Sum32()

	bucket := int(hash % 100)

	// Walk through sorted variants, accumulating percentages
	cumulative := 0
	for _, v := range variants {
		cumulative += trafficSplit[v]
		if bucket < cumulative {
			return v
		}
	}

	// Fallback to last variant (should not happen if percentages sum to 100)
	return variants[len(variants)-1]
}
