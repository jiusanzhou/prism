package config

import (
	"encoding/json"
	"testing"
)

func TestSanitizeConfig(t *testing.T) {
	cfg := &PrismConfig{
		Providers: map[string]ProviderConfig{
			"openai":   {Type: "openai", APIKey: "sk-test-key-1234567890abcdef"},
			"deepseek": {Type: "deepseek", APIKey: "ds-key-abcdef"},
		},
		Storage: StorageConfig{
			Postgres: PostgresConfig{DSN: "postgres://user:pass@localhost/prism"},
		},
	}
	s := SanitizeConfig(cfg)
	data, _ := json.MarshalIndent(s, "", "  ")
	t.Logf("sanitized JSON:\n%s", string(data))

	if s.Providers["openai"].APIKey == "sk-test-key-1234567890abcdef" {
		t.Error("openai API key not masked")
	}
	if s.Providers["deepseek"].APIKey == "ds-key-abcdef" {
		t.Error("deepseek API key not masked")
	}
	if s.Storage.Postgres.DSN != "***" {
		t.Errorf("postgres DSN not masked: %s", s.Storage.Postgres.DSN)
	}
}
