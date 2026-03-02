use std::path::{Path, PathBuf};

use super::{ConfigError, ConfigSource, PrismConfig};

/// File configuration source — supports YAML/JSON
pub struct FileSource {
    path: PathBuf,
    #[allow(dead_code)]
    format: ConfigFormat,
}

/// Supported configuration file formats.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum ConfigFormat {
    Yaml,
    Json,
    /// Auto-detect based on file extension.
    Auto,
}

impl FileSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: ConfigFormat::Auto,
        }
    }

    #[allow(dead_code)]
    pub fn with_format(mut self, format: ConfigFormat) -> Self {
        self.format = format;
        self
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn detect_format(&self) -> ConfigFormat {
        match self.format {
            ConfigFormat::Auto => match self.path.extension().and_then(|e| e.to_str()) {
                Some("json") => ConfigFormat::Json,
                Some("yaml") | Some("yml") => ConfigFormat::Yaml,
                _ => ConfigFormat::Yaml, // default
            },
            other => other,
        }
    }
}

impl ConfigSource for FileSource {
    fn name(&self) -> &str {
        "file"
    }

    fn load(&self) -> Result<PrismConfig, ConfigError> {
        let content = std::fs::read_to_string(&self.path)
            .map_err(|e| ConfigError::NotFound(format!("{}: {}", self.path.display(), e)))?;

        // Expand environment variables
        let expanded = expand_env_vars(&content);

        match self.detect_format() {
            ConfigFormat::Yaml | ConfigFormat::Auto => serde_yaml::from_str(&expanded)
                .map_err(|e| ConfigError::ParseError(format!("YAML: {}", e))),
            ConfigFormat::Json => serde_json::from_str(&expanded)
                .map_err(|e| ConfigError::ParseError(format!("JSON: {}", e))),
        }
    }
}

/// Expand `${VAR_NAME}` patterns in a string using environment variables.
pub(crate) fn expand_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    // Simple regex-free approach: scan for ${...}
    loop {
        if let Some(start) = result.find("${") {
            if let Some(end) = result[start..].find('}') {
                let var_name = &result[start + 2..start + end];
                let value = std::env::var(var_name).unwrap_or_default();
                result = format!(
                    "{}{}{}",
                    &result[..start],
                    value,
                    &result[start + end + 1..]
                );
                continue;
            }
        }
        break;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_expand_env_vars() {
        std::env::set_var("TEST_PRISM_VAR", "hello");
        let input = "key: ${TEST_PRISM_VAR}";
        let result = expand_env_vars(input);
        assert_eq!(result, "key: hello");
        std::env::remove_var("TEST_PRISM_VAR");
    }

    #[test]
    fn test_expand_env_vars_missing() {
        let input = "key: ${NONEXISTENT_PRISM_VAR_12345}";
        let result = expand_env_vars(input);
        assert_eq!(result, "key: ");
    }

    #[test]
    fn test_expand_env_vars_multiple() {
        std::env::set_var("TEST_A", "aaa");
        std::env::set_var("TEST_B", "bbb");
        let input = "a: ${TEST_A}, b: ${TEST_B}";
        let result = expand_env_vars(input);
        assert_eq!(result, "a: aaa, b: bbb");
        std::env::remove_var("TEST_A");
        std::env::remove_var("TEST_B");
    }

    #[test]
    fn test_expand_env_vars_no_vars() {
        let input = "no variables here";
        let result = expand_env_vars(input);
        assert_eq!(result, "no variables here");
    }

    #[test]
    fn test_expand_env_vars_unclosed_brace() {
        let input = "key: ${UNCLOSED";
        let result = expand_env_vars(input);
        assert_eq!(result, "key: ${UNCLOSED");
    }

    #[test]
    fn test_file_source_yaml_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.yaml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"
server:
  http_port: 7777
providers:
  test:
    base_url: http://localhost
models:
  - id: m1
    display_name: M1
    quality_tier: A
variants:
  - id: m1@test
    model_id: m1
    provider_id: test
pools:
  - id: p1
    name: Pool1
    variants: [m1@test]
"#
        )
        .unwrap();

        let source = FileSource::new(&path);
        let config = source.load().unwrap();
        assert_eq!(config.server.http_port, 7777);
        assert_eq!(config.providers.len(), 1);
    }

    #[test]
    fn test_file_source_json_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{
  "server": {{ "http_port": 6666 }},
  "providers": {{
    "test": {{ "base_url": "http://localhost" }}
  }},
  "models": [{{ "id": "m1", "display_name": "M1", "quality_tier": "A" }}],
  "variants": [{{ "id": "m1@test", "model_id": "m1", "provider_id": "test" }}],
  "pools": [{{ "id": "p1", "name": "Pool1", "variants": ["m1@test"] }}]
}}"#
        )
        .unwrap();

        let source = FileSource::new(&path);
        let config = source.load().unwrap();
        assert_eq!(config.server.http_port, 6666);
    }

    #[test]
    fn test_file_source_auto_detect_yaml() {
        let source = FileSource::new("/tmp/config.yml");
        match source.detect_format() {
            ConfigFormat::Yaml => {}
            other => panic!("expected Yaml, got {:?}", other),
        }
    }

    #[test]
    fn test_file_source_auto_detect_json() {
        let source = FileSource::new("/tmp/config.json");
        match source.detect_format() {
            ConfigFormat::Json => {}
            other => panic!("expected Json, got {:?}", other),
        }
    }

    #[test]
    fn test_file_source_auto_detect_unknown_defaults_yaml() {
        let source = FileSource::new("/tmp/config.toml");
        match source.detect_format() {
            ConfigFormat::Yaml => {}
            other => panic!("expected Yaml (default), got {:?}", other),
        }
    }

    #[test]
    fn test_file_source_not_found() {
        let source = FileSource::new("/nonexistent/path/config.yaml");
        match source.load() {
            Err(ConfigError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {:?}", other),
        }
    }

    #[test]
    fn test_file_source_supports_watch_false() {
        let source = FileSource::new("/tmp/test.yaml");
        assert!(!source.supports_watch());
    }
}
