package discovery

// CatalogData provides a Go-side built-in model catalog that mirrors the Rust catalog.
// This is used by auto-config and discovery to enrich discovered models with
// pricing, capabilities, and quality tier information without requiring FFI calls.
type CatalogData struct {
	models map[string]*CatalogEntry
	// alias → canonical model ID
	aliases map[string]string
}

// CatalogEntry describes a well-known model's metadata.
type CatalogEntry struct {
	ID           string
	DisplayName  string
	Provider     string // "openai", "anthropic", "deepseek", "google"
	QualityTier  string // "S", "A", "B", "C"
	Capabilities CatalogCapabilities
	Pricing      CatalogPricing
	Specialties  []string
	Aliases      []string
}

// CatalogCapabilities describes what a model can do.
type CatalogCapabilities struct {
	MaxContextWindow      int
	MaxOutputTokens       int
	SupportsStreaming      bool
	SupportsFunctionCalling bool
	SupportsVision        bool
	SupportsJSONMode      bool
	SupportsReasoning     bool
}

// CatalogPricing holds pricing per 1M tokens.
type CatalogPricing struct {
	InputPer1MTokens  float64
	OutputPer1MTokens float64
}

// DefaultCatalog creates a catalog populated with all known models.
func DefaultCatalog() *CatalogData {
	c := &CatalogData{
		models:  make(map[string]*CatalogEntry),
		aliases: make(map[string]string),
	}
	c.registerOpenAI()
	c.registerAnthropic()
	c.registerDeepSeek()
	c.registerGoogle()
	return c
}

// Lookup finds a model by ID or alias.
func (c *CatalogData) Lookup(modelID string) *CatalogEntry {
	if entry, ok := c.models[modelID]; ok {
		return entry
	}
	if canonical, ok := c.aliases[modelID]; ok {
		return c.models[canonical]
	}
	return nil
}

// ModelsForProvider returns all models for a given provider.
func (c *CatalogData) ModelsForProvider(provider string) []*CatalogEntry {
	var result []*CatalogEntry
	for _, entry := range c.models {
		if entry.Provider == provider {
			result = append(result, entry)
		}
	}
	return result
}

// AllModels returns all catalog entries.
func (c *CatalogData) AllModels() map[string]*CatalogEntry {
	return c.models
}

func (c *CatalogData) register(entry *CatalogEntry) {
	c.models[entry.ID] = entry
	for _, alias := range entry.Aliases {
		c.aliases[alias] = entry.ID
	}
}

func (c *CatalogData) registerOpenAI() {
	c.register(&CatalogEntry{
		ID: "gpt-4o", DisplayName: "GPT-4o", Provider: "openai", QualityTier: "S",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 128_000, MaxOutputTokens: 16_384,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: true, SupportsJSONMode: true, SupportsReasoning: true,
		},
		Pricing:     CatalogPricing{InputPer1MTokens: 2.50, OutputPer1MTokens: 10.00},
		Specialties: nil,
		Aliases:     []string{"gpt-4o-2024-11-20", "gpt-4o-2024-08-06", "gpt-4o-2024-05-13"},
	})
	c.register(&CatalogEntry{
		ID: "gpt-4o-mini", DisplayName: "GPT-4o Mini", Provider: "openai", QualityTier: "A",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 128_000, MaxOutputTokens: 16_384,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: true, SupportsJSONMode: true, SupportsReasoning: false,
		},
		Pricing: CatalogPricing{InputPer1MTokens: 0.15, OutputPer1MTokens: 0.60},
		Aliases: []string{"gpt-4o-mini-2024-07-18"},
	})
	c.register(&CatalogEntry{
		ID: "gpt-4-turbo", DisplayName: "GPT-4 Turbo", Provider: "openai", QualityTier: "S",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 128_000, MaxOutputTokens: 4_096,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: true, SupportsJSONMode: true, SupportsReasoning: false,
		},
		Pricing: CatalogPricing{InputPer1MTokens: 10.00, OutputPer1MTokens: 30.00},
		Aliases: []string{"gpt-4-turbo-2024-04-09", "gpt-4-turbo-preview"},
	})
	c.register(&CatalogEntry{
		ID: "gpt-3.5-turbo", DisplayName: "GPT-3.5 Turbo", Provider: "openai", QualityTier: "B",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 16_385, MaxOutputTokens: 4_096,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: false, SupportsJSONMode: true, SupportsReasoning: false,
		},
		Pricing: CatalogPricing{InputPer1MTokens: 0.50, OutputPer1MTokens: 1.50},
		Aliases: []string{"gpt-3.5-turbo-0125"},
	})
	c.register(&CatalogEntry{
		ID: "o1", DisplayName: "OpenAI o1", Provider: "openai", QualityTier: "S",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 200_000, MaxOutputTokens: 100_000,
			SupportsStreaming: true, SupportsFunctionCalling: false,
			SupportsVision: false, SupportsJSONMode: false, SupportsReasoning: true,
		},
		Pricing:     CatalogPricing{InputPer1MTokens: 15.00, OutputPer1MTokens: 60.00},
		Specialties: []string{"Reasoning", "Math"},
		Aliases:     []string{"o1-2024-12-17"},
	})
	c.register(&CatalogEntry{
		ID: "o1-mini", DisplayName: "OpenAI o1-mini", Provider: "openai", QualityTier: "A",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 128_000, MaxOutputTokens: 65_536,
			SupportsStreaming: true, SupportsFunctionCalling: false,
			SupportsVision: false, SupportsJSONMode: false, SupportsReasoning: true,
		},
		Pricing:     CatalogPricing{InputPer1MTokens: 3.00, OutputPer1MTokens: 12.00},
		Specialties: []string{"Reasoning", "CodeGeneration"},
		Aliases:     []string{"o1-mini-2024-09-12"},
	})
	c.register(&CatalogEntry{
		ID: "o3-mini", DisplayName: "OpenAI o3-mini", Provider: "openai", QualityTier: "A",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 200_000, MaxOutputTokens: 100_000,
			SupportsStreaming: true, SupportsFunctionCalling: false,
			SupportsVision: false, SupportsJSONMode: false, SupportsReasoning: true,
		},
		Pricing:     CatalogPricing{InputPer1MTokens: 1.10, OutputPer1MTokens: 4.40},
		Specialties: []string{"Reasoning", "Math", "CodeGeneration"},
		Aliases:     []string{"o3-mini-2025-01-31"},
	})
}

func (c *CatalogData) registerAnthropic() {
	c.register(&CatalogEntry{
		ID: "claude-sonnet-4-20250514", DisplayName: "Claude Sonnet 4", Provider: "anthropic", QualityTier: "S",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 200_000, MaxOutputTokens: 8_192,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: true, SupportsJSONMode: true, SupportsReasoning: true,
		},
		Pricing:     CatalogPricing{InputPer1MTokens: 3.00, OutputPer1MTokens: 15.00},
		Specialties: []string{"CodeGeneration", "Reasoning"},
		Aliases:     []string{"claude-sonnet-4"},
	})
	c.register(&CatalogEntry{
		ID: "claude-3-5-sonnet-20241022", DisplayName: "Claude 3.5 Sonnet", Provider: "anthropic", QualityTier: "S",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 200_000, MaxOutputTokens: 8_192,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: true, SupportsJSONMode: true, SupportsReasoning: true,
		},
		Pricing:     CatalogPricing{InputPer1MTokens: 3.00, OutputPer1MTokens: 15.00},
		Specialties: []string{"CodeGeneration", "Reasoning"},
		Aliases:     []string{"claude-3.5-sonnet", "claude-3-5-sonnet-latest"},
	})
	c.register(&CatalogEntry{
		ID: "claude-3-5-haiku-20241022", DisplayName: "Claude 3.5 Haiku", Provider: "anthropic", QualityTier: "A",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 200_000, MaxOutputTokens: 8_192,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: true, SupportsJSONMode: true, SupportsReasoning: false,
		},
		Pricing: CatalogPricing{InputPer1MTokens: 0.80, OutputPer1MTokens: 4.00},
		Aliases: []string{"claude-3.5-haiku", "claude-3-5-haiku-latest"},
	})
	c.register(&CatalogEntry{
		ID: "claude-3-opus-20240229", DisplayName: "Claude 3 Opus", Provider: "anthropic", QualityTier: "S",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 200_000, MaxOutputTokens: 4_096,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: true, SupportsJSONMode: true, SupportsReasoning: true,
		},
		Pricing:     CatalogPricing{InputPer1MTokens: 15.00, OutputPer1MTokens: 75.00},
		Specialties: []string{"Reasoning", "Creative"},
		Aliases:     []string{"claude-3-opus", "claude-3-opus-latest"},
	})
}

func (c *CatalogData) registerDeepSeek() {
	c.register(&CatalogEntry{
		ID: "deepseek-chat", DisplayName: "DeepSeek Chat (V3)", Provider: "deepseek", QualityTier: "A",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 128_000, MaxOutputTokens: 8_192,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: false, SupportsJSONMode: true, SupportsReasoning: false,
		},
		Pricing:     CatalogPricing{InputPer1MTokens: 0.27, OutputPer1MTokens: 1.10},
		Specialties: []string{"Reasoning", "CodeGeneration"},
		Aliases:     []string{"deepseek-v3"},
	})
	c.register(&CatalogEntry{
		ID: "deepseek-reasoner", DisplayName: "DeepSeek Reasoner (R1)", Provider: "deepseek", QualityTier: "S",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 128_000, MaxOutputTokens: 8_192,
			SupportsStreaming: true, SupportsFunctionCalling: false,
			SupportsVision: false, SupportsJSONMode: false, SupportsReasoning: true,
		},
		Pricing:     CatalogPricing{InputPer1MTokens: 0.55, OutputPer1MTokens: 2.19},
		Specialties: []string{"Reasoning", "Math"},
		Aliases:     []string{"deepseek-r1"},
	})
}

func (c *CatalogData) registerGoogle() {
	c.register(&CatalogEntry{
		ID: "gemini-2.0-flash", DisplayName: "Gemini 2.0 Flash", Provider: "google", QualityTier: "A",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 1_048_576, MaxOutputTokens: 8_192,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: true, SupportsJSONMode: true, SupportsReasoning: false,
		},
		Pricing: CatalogPricing{InputPer1MTokens: 0.10, OutputPer1MTokens: 0.40},
		Aliases: []string{"gemini-2.0-flash-001"},
	})
	c.register(&CatalogEntry{
		ID: "gemini-2.5-pro-preview-06-05", DisplayName: "Gemini 2.5 Pro", Provider: "google", QualityTier: "S",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 1_048_576, MaxOutputTokens: 65_536,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: true, SupportsJSONMode: true, SupportsReasoning: true,
		},
		Pricing:     CatalogPricing{InputPer1MTokens: 1.25, OutputPer1MTokens: 10.00},
		Specialties: []string{"Reasoning", "CodeGeneration"},
		Aliases:     []string{"gemini-2.5-pro", "gemini-2.5-pro-preview"},
	})
	c.register(&CatalogEntry{
		ID: "gemini-2.5-flash-preview-05-20", DisplayName: "Gemini 2.5 Flash", Provider: "google", QualityTier: "A",
		Capabilities: CatalogCapabilities{
			MaxContextWindow: 1_048_576, MaxOutputTokens: 65_536,
			SupportsStreaming: true, SupportsFunctionCalling: true,
			SupportsVision: true, SupportsJSONMode: true, SupportsReasoning: true,
		},
		Pricing: CatalogPricing{InputPer1MTokens: 0.15, OutputPer1MTokens: 0.60},
		Aliases: []string{"gemini-2.5-flash", "gemini-2.5-flash-preview"},
	})
}
