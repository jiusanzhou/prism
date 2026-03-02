//! Plugin Manager — runtime plugin lifecycle management.
//!
//! Manages loaded plugins with support for:
//! - Load: resolve source → compile WASM → register to phases
//! - Unload: remove from all phases
//! - Reload: atomic replace (load new → swap → unload old)
//! - List: query loaded plugin status

use std::collections::HashMap;
use std::sync::RwLock;

use crate::config::PluginConfig;
use crate::plugin_loader::{LoadError, PluginLoader};

/// Plugin runtime state.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    /// Plugin name.
    pub name: String,
    /// Source description.
    pub source_info: String,
    /// SHA-256 of loaded WASM bytes.
    pub sha256: String,
    /// Which phases this plugin is registered for.
    pub phases: Vec<String>,
    /// Whether currently active.
    pub active: bool,
    /// Load timestamp.
    pub loaded_at: std::time::Instant,
}

/// Events emitted by the plugin manager.
#[derive(Debug, Clone)]
pub enum PluginEvent {
    Loaded {
        name: String,
        sha256: String,
    },
    Unloaded {
        name: String,
    },
    Reloaded {
        name: String,
        old_sha256: String,
        new_sha256: String,
    },
    LoadFailed {
        name: String,
        error: String,
    },
}

/// Plugin manager — lifecycle management for WASM plugins.
pub struct PluginManager {
    /// Plugin loader (handles source resolution + caching).
    loader: PluginLoader,
    /// Currently loaded plugins.
    plugins: RwLock<HashMap<String, PluginInfo>>,
    /// Event listeners.
    #[allow(clippy::type_complexity)]
    listeners: RwLock<Vec<Box<dyn Fn(PluginEvent) + Send + Sync>>>,
}

impl PluginManager {
    pub fn new(loader: PluginLoader) -> Self {
        Self {
            loader,
            plugins: RwLock::new(HashMap::new()),
            listeners: RwLock::new(Vec::new()),
        }
    }

    pub fn with_default_loader() -> Self {
        Self::new(PluginLoader::with_default_cache())
    }

    /// Load a plugin from config.
    pub fn load(&self, config: &PluginConfig) -> Result<PluginInfo, LoadError> {
        let resolved = self.loader.resolve(&config.name, &config.source)?;

        let info = PluginInfo {
            name: config.name.clone(),
            source_info: resolved.source_info.clone(),
            sha256: resolved.sha256.clone(),
            phases: config.phases.clone(),
            active: config.enabled,
            loaded_at: std::time::Instant::now(),
        };

        if let Ok(mut plugins) = self.plugins.write() {
            plugins.insert(config.name.clone(), info.clone());
        }

        self.emit(PluginEvent::Loaded {
            name: config.name.clone(),
            sha256: resolved.sha256,
        });

        Ok(info)
    }

    /// Unload a plugin by name.
    pub fn unload(&self, name: &str) -> Result<(), LoadError> {
        if let Ok(mut plugins) = self.plugins.write() {
            if plugins.remove(name).is_some() {
                self.emit(PluginEvent::Unloaded {
                    name: name.to_string(),
                });
                Ok(())
            } else {
                Err(LoadError::NotFound(format!("plugin '{}' not loaded", name)))
            }
        } else {
            Err(LoadError::Other("lock poisoned".into()))
        }
    }

    /// Reload a plugin — load new version, then swap.
    pub fn reload(&self, config: &PluginConfig) -> Result<PluginInfo, LoadError> {
        let resolved = self.loader.resolve(&config.name, &config.source)?;

        let old_sha256 = self
            .plugins
            .read()
            .ok()
            .and_then(|p| p.get(&config.name).map(|i| i.sha256.clone()))
            .unwrap_or_default();

        let info = PluginInfo {
            name: config.name.clone(),
            source_info: resolved.source_info.clone(),
            sha256: resolved.sha256.clone(),
            phases: config.phases.clone(),
            active: config.enabled,
            loaded_at: std::time::Instant::now(),
        };

        if let Ok(mut plugins) = self.plugins.write() {
            plugins.insert(config.name.clone(), info.clone());
        }

        self.emit(PluginEvent::Reloaded {
            name: config.name.clone(),
            old_sha256,
            new_sha256: resolved.sha256,
        });

        Ok(info)
    }

    /// Load all plugins from config list.
    pub fn load_all(
        &self,
        configs: &[PluginConfig],
    ) -> Vec<Result<PluginInfo, (String, LoadError)>> {
        configs
            .iter()
            .map(|c| self.load(c).map_err(|e| (c.name.clone(), e)))
            .collect()
    }

    /// Get info for a loaded plugin.
    pub fn get(&self, name: &str) -> Option<PluginInfo> {
        self.plugins.read().ok()?.get(name).cloned()
    }

    /// List all loaded plugins.
    pub fn list(&self) -> Vec<PluginInfo> {
        self.plugins
            .read()
            .map(|p| p.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Enable/disable a plugin.
    pub fn set_active(&self, name: &str, active: bool) -> Result<(), LoadError> {
        if let Ok(mut plugins) = self.plugins.write() {
            if let Some(info) = plugins.get_mut(name) {
                info.active = active;
                Ok(())
            } else {
                Err(LoadError::NotFound(format!("plugin '{}' not loaded", name)))
            }
        } else {
            Err(LoadError::Other("lock poisoned".into()))
        }
    }

    /// Register an event listener.
    pub fn on_event(&self, listener: Box<dyn Fn(PluginEvent) + Send + Sync>) {
        if let Ok(mut listeners) = self.listeners.write() {
            listeners.push(listener);
        }
    }

    /// Get the underlying loader.
    pub fn loader(&self) -> &PluginLoader {
        &self.loader
    }

    fn emit(&self, event: PluginEvent) {
        if let Ok(listeners) = self.listeners.read() {
            for listener in listeners.iter() {
                listener(event.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PluginConfig, PluginSourceConfig};
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn make_wasm_file() -> (tempfile::NamedTempFile, String) {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"fake wasm module bytes").unwrap();
        tmp.flush().unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        (tmp, path)
    }

    fn make_plugin_config(name: &str, path: &str) -> PluginConfig {
        PluginConfig {
            name: name.to_string(),
            source: PluginSourceConfig::Wasm {
                wasm: path.to_string(),
            },
            phases: vec!["proxy".to_string()],
            config: serde_json::Value::Null,
            enabled: true,
            priority: 100,
        }
    }

    #[test]
    fn test_load_plugin() {
        let (_tmp, path) = make_wasm_file();
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        let config = make_plugin_config("test-plugin", &path);
        let info = manager.load(&config).unwrap();

        assert_eq!(info.name, "test-plugin");
        assert!(info.active);
        assert_eq!(info.phases, vec!["proxy"]);
        assert!(!info.sha256.is_empty());
    }

    #[test]
    fn test_unload_plugin() {
        let (_tmp, path) = make_wasm_file();
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        let config = make_plugin_config("to-unload", &path);
        manager.load(&config).unwrap();

        assert!(manager.get("to-unload").is_some());

        manager.unload("to-unload").unwrap();
        assert!(manager.get("to-unload").is_none());
    }

    #[test]
    fn test_unload_not_found() {
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        let result = manager.unload("nonexistent");
        assert!(result.is_err());
        match result.unwrap_err() {
            LoadError::NotFound(msg) => assert!(msg.contains("nonexistent")),
            other => panic!("expected NotFound, got {:?}", other),
        }
    }

    #[test]
    fn test_reload_plugin() {
        let (_tmp, path) = make_wasm_file();
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        let config = make_plugin_config("reload-me", &path);
        let info1 = manager.load(&config).unwrap();

        // Reload same plugin
        let info2 = manager.reload(&config).unwrap();

        // SHA should be the same since bytes haven't changed
        assert_eq!(info1.sha256, info2.sha256);
        assert_eq!(info2.name, "reload-me");
    }

    #[test]
    fn test_list_plugins() {
        let (_tmp, path) = make_wasm_file();
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        assert!(manager.list().is_empty());

        let config1 = make_plugin_config("plugin-a", &path);
        let config2 = make_plugin_config("plugin-b", &path);
        manager.load(&config1).unwrap();
        manager.load(&config2).unwrap();

        let list = manager.list();
        assert_eq!(list.len(), 2);

        let names: Vec<&str> = list.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"plugin-a"));
        assert!(names.contains(&"plugin-b"));
    }

    #[test]
    fn test_get_plugin() {
        let (_tmp, path) = make_wasm_file();
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        let config = make_plugin_config("get-me", &path);
        manager.load(&config).unwrap();

        let info = manager.get("get-me");
        assert!(info.is_some());
        assert_eq!(info.unwrap().name, "get-me");

        assert!(manager.get("nonexistent").is_none());
    }

    #[test]
    fn test_set_active() {
        let (_tmp, path) = make_wasm_file();
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        let config = make_plugin_config("toggle", &path);
        manager.load(&config).unwrap();

        assert!(manager.get("toggle").unwrap().active);

        manager.set_active("toggle", false).unwrap();
        assert!(!manager.get("toggle").unwrap().active);

        manager.set_active("toggle", true).unwrap();
        assert!(manager.get("toggle").unwrap().active);
    }

    #[test]
    fn test_set_active_not_found() {
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        let result = manager.set_active("missing", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_all() {
        let (_tmp, path) = make_wasm_file();
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        let configs = vec![
            make_plugin_config("batch-a", &path),
            make_plugin_config("batch-b", &path),
            PluginConfig {
                name: "batch-fail".to_string(),
                source: PluginSourceConfig::Wasm {
                    wasm: "/nonexistent.wasm".to_string(),
                },
                phases: vec!["proxy".to_string()],
                config: serde_json::Value::Null,
                enabled: true,
                priority: 100,
            },
        ];

        let results = manager.load_all(&configs);
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert!(results[2].is_err());

        // Successfully loaded plugins should be in the list
        assert_eq!(manager.list().len(), 2);
    }

    #[test]
    fn test_on_event_listener() {
        let (_tmp, path) = make_wasm_file();
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        let event_count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&event_count);
        manager.on_event(Box::new(move |_event| {
            count_clone.fetch_add(1, Ordering::SeqCst);
        }));

        let config = make_plugin_config("evented", &path);
        manager.load(&config).unwrap(); // +1 Loaded event
        manager.unload("evented").unwrap(); // +1 Unloaded event

        assert_eq!(event_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_on_event_reload() {
        let (_tmp, path) = make_wasm_file();
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        let events: Arc<RwLock<Vec<String>>> = Arc::new(RwLock::new(Vec::new()));
        let events_clone = Arc::clone(&events);
        manager.on_event(Box::new(move |event| {
            let label = match &event {
                PluginEvent::Loaded { .. } => "loaded",
                PluginEvent::Unloaded { .. } => "unloaded",
                PluginEvent::Reloaded { .. } => "reloaded",
                PluginEvent::LoadFailed { .. } => "failed",
            };
            if let Ok(mut evts) = events_clone.write() {
                evts.push(label.to_string());
            }
        }));

        let config = make_plugin_config("reload-evt", &path);
        manager.load(&config).unwrap();
        manager.reload(&config).unwrap();

        let evts = events.read().unwrap();
        assert_eq!(*evts, vec!["loaded", "reloaded"]);
    }

    #[test]
    fn test_load_builtin_fails() {
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);

        let config = PluginConfig {
            name: "builtin-plugin".to_string(),
            source: PluginSourceConfig::Builtin {
                builtin: "test".to_string(),
            },
            phases: vec!["route".to_string()],
            config: serde_json::Value::Null,
            enabled: true,
            priority: 100,
        };

        // Loading a builtin through the manager should fail since there are no WASM bytes
        let result = manager.load(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_loader_accessor() {
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let manager = PluginManager::new(loader);
        // Just verify we can access the loader
        assert_eq!(manager.loader().cache_dir(), cache_dir.path());
    }
}
