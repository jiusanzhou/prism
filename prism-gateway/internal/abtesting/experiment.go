package abtesting

// Experiment defines an A/B test configuration
type Experiment struct {
	ID          string         `yaml:"id" json:"id"`
	Name        string         `yaml:"name" json:"name"`
	Description string         `yaml:"description" json:"description"`
	Enabled     bool           `yaml:"enabled" json:"enabled"`
	// Traffic split: variant_id → percentage (must sum to 100)
	TrafficSplit map[string]int `yaml:"traffic_split" json:"traffic_split"`
	// Target: which requests this experiment applies to
	Target      ExperimentTarget `yaml:"target" json:"target"`
	// Metrics to track for comparison
	Metrics     []string `yaml:"metrics" json:"metrics"`
	// Start/end time (optional)
	StartTime *string `yaml:"start_time,omitempty" json:"start_time,omitempty"`
	EndTime   *string `yaml:"end_time,omitempty" json:"end_time,omitempty"`
}

// ExperimentTarget defines which requests an experiment applies to
type ExperimentTarget struct {
	// Apply to specific model pools
	PoolIDs []string `yaml:"pool_ids,omitempty" json:"pool_ids,omitempty"`
	// Apply to specific tenant IDs
	TenantIDs []string `yaml:"tenant_ids,omitempty" json:"tenant_ids,omitempty"`
	// Apply to specific source platforms
	Sources []string `yaml:"sources,omitempty" json:"sources,omitempty"`
	// Percentage of total traffic to include in experiment (0-100)
	SampleRate int `yaml:"sample_rate" json:"sample_rate"`
}
