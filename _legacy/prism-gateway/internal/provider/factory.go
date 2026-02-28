package provider

import "fmt"

// NewProvider creates a Provider by type name and configuration.
// Supported types: "openai", "deepseek", "openai-compatible", "anthropic".
// DeepSeek and other OpenAI-compatible APIs reuse OpenAIProvider — just set the BaseURL.
func NewProvider(providerType string, cfg ProviderConfig) (Provider, error) {
	switch providerType {
	case "openai", "deepseek", "openai-compatible":
		return NewOpenAIProvider(cfg), nil
	case "anthropic":
		return NewAnthropicProvider(cfg), nil
	case "mock":
		return NewMockProvider(cfg.Name), nil
	default:
		return nil, fmt.Errorf("unknown provider type: %s", providerType)
	}
}
