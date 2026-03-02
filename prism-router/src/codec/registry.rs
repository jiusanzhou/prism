//! Registry mapping provider/protocol to plugin chains.
//!
//! The registry provides a two-level lookup:
//! 1. **Provider-specific**: a chain registered for a particular `provider_id`.
//! 2. **Protocol-pair**: a chain registered for an `(inbound, outbound)` protocol pair.
//!
//! Provider-specific chains take priority over protocol-pair chains.

use std::collections::HashMap;

use super::chain::ProxyPluginChain;

// ════════════════════════════════════════════════
// CodecRegistry
// ════════════════════════════════════════════════

/// Maps a `provider_id` or protocol pair to a [`ProxyPluginChain`].
pub struct CodecRegistry {
    /// `provider_id` → chain.
    provider_chains: HashMap<String, ProxyPluginChain>,
    /// `(inbound_protocol, outbound_protocol)` → chain.
    protocol_chains: HashMap<(String, String), ProxyPluginChain>,
    /// Global plugin chain — runs on ALL requests regardless of protocol conversion.
    /// Used for cross-cutting concerns: metering, billing, security audit, token counting.
    global_chain: Option<ProxyPluginChain>,
}

impl CodecRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            provider_chains: HashMap::new(),
            protocol_chains: HashMap::new(),
            global_chain: None,
        }
    }

    /// Set the global plugin chain (runs on all requests).
    pub fn set_global_chain(&mut self, chain: ProxyPluginChain) {
        self.global_chain = Some(chain);
    }

    /// Get the global plugin chain (if any).
    pub fn global_chain(&self) -> Option<&ProxyPluginChain> {
        self.global_chain.as_ref()
    }

    /// Register a chain for a specific provider.
    pub fn register_provider(&mut self, provider_id: &str, chain: ProxyPluginChain) {
        self.provider_chains
            .insert(provider_id.to_string(), chain);
    }

    /// Register a chain for a protocol conversion pair.
    pub fn register_protocol_pair(
        &mut self,
        inbound: &str,
        outbound: &str,
        chain: ProxyPluginChain,
    ) {
        self.protocol_chains.insert(
            (inbound.to_string(), outbound.to_string()),
            chain,
        );
    }

    /// Look up a chain: first try provider-specific, then protocol pair, then `None`.
    pub fn get_chain(
        &self,
        provider_id: &str,
        inbound: &str,
        outbound: &str,
    ) -> Option<&ProxyPluginChain> {
        // Provider-specific takes priority.
        if let Some(chain) = self.provider_chains.get(provider_id) {
            return Some(chain);
        }
        // Fall back to protocol pair.
        self.protocol_chains
            .get(&(inbound.to_string(), outbound.to_string()))
    }

    /// Check if any conversion chain is registered for this combination.
    pub fn needs_conversion(
        &self,
        provider_id: &str,
        inbound: &str,
        outbound: &str,
    ) -> bool {
        self.get_chain(provider_id, inbound, outbound).is_some()
    }
}

impl Default for CodecRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{ProxyContext, ProxyPlugin};

    /// Minimal plugin for registry testing.
    struct TagPlugin {
        tag: String,
    }

    impl TagPlugin {
        fn new(tag: &str) -> Self {
            Self {
                tag: tag.to_string(),
            }
        }
    }

    impl ProxyPlugin for TagPlugin {
        fn name(&self) -> &str {
            &self.tag
        }

        fn on_request_body(
            &self,
            _ctx: &mut ProxyContext,
            body: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            let mut new = format!("[{}]", self.tag).into_bytes();
            new.extend_from_slice(&body);
            Ok(new)
        }
    }

    fn make_chain(tag: &str) -> ProxyPluginChain {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(TagPlugin::new(tag)));
        chain
    }

    #[test]
    fn test_empty_registry() {
        let registry = CodecRegistry::new();
        assert!(registry.get_chain("any", "openai", "anthropic").is_none());
        assert!(!registry.needs_conversion("any", "openai", "anthropic"));
    }

    #[test]
    fn test_provider_chain_lookup() {
        let mut registry = CodecRegistry::new();
        registry.register_provider("my-provider", make_chain("provider-chain"));

        let chain = registry.get_chain("my-provider", "openai", "anthropic");
        assert!(chain.is_some());

        // Verify it's the right chain by running it.
        let mut ctx = ProxyContext::new();
        let result = chain
            .unwrap()
            .process_request_body(&mut ctx, b"X".to_vec())
            .unwrap();
        assert_eq!(result, b"[provider-chain]X");
    }

    #[test]
    fn test_protocol_pair_lookup() {
        let mut registry = CodecRegistry::new();
        registry.register_protocol_pair(
            "openai",
            "anthropic",
            make_chain("openai-to-anthropic"),
        );

        // No provider chain → falls back to protocol pair.
        let chain = registry.get_chain("unknown-provider", "openai", "anthropic");
        assert!(chain.is_some());

        let mut ctx = ProxyContext::new();
        let result = chain
            .unwrap()
            .process_request_body(&mut ctx, b"X".to_vec())
            .unwrap();
        assert_eq!(result, b"[openai-to-anthropic]X");
    }

    #[test]
    fn test_provider_takes_priority_over_protocol_pair() {
        let mut registry = CodecRegistry::new();
        registry.register_provider("special-provider", make_chain("special"));
        registry.register_protocol_pair("openai", "anthropic", make_chain("generic"));

        // Provider-specific should win.
        let chain = registry
            .get_chain("special-provider", "openai", "anthropic")
            .unwrap();
        let mut ctx = ProxyContext::new();
        let result = chain
            .process_request_body(&mut ctx, b"X".to_vec())
            .unwrap();
        assert_eq!(result, b"[special]X");

        // Different provider falls back to protocol pair.
        let chain = registry
            .get_chain("other-provider", "openai", "anthropic")
            .unwrap();
        let result = chain
            .process_request_body(&mut ctx, b"X".to_vec())
            .unwrap();
        assert_eq!(result, b"[generic]X");
    }

    #[test]
    fn test_needs_conversion() {
        let mut registry = CodecRegistry::new();
        assert!(!registry.needs_conversion("p", "openai", "anthropic"));

        registry.register_protocol_pair("openai", "anthropic", make_chain("conv"));
        assert!(registry.needs_conversion("any", "openai", "anthropic"));
        assert!(!registry.needs_conversion("any", "anthropic", "openai"));
    }

    #[test]
    fn test_same_protocol_no_conversion() {
        let registry = CodecRegistry::new();
        // Same protocol pair with no registration → no conversion needed.
        assert!(!registry.needs_conversion("p", "openai", "openai"));
    }

    #[test]
    fn test_multiple_protocol_pairs() {
        let mut registry = CodecRegistry::new();
        registry.register_protocol_pair("openai", "anthropic", make_chain("o2a"));
        registry.register_protocol_pair("anthropic", "openai", make_chain("a2o"));

        let mut ctx = ProxyContext::new();

        let chain = registry.get_chain("p", "openai", "anthropic").unwrap();
        let result = chain
            .process_request_body(&mut ctx, b"X".to_vec())
            .unwrap();
        assert_eq!(result, b"[o2a]X");

        let chain = registry.get_chain("p", "anthropic", "openai").unwrap();
        let result = chain
            .process_request_body(&mut ctx, b"X".to_vec())
            .unwrap();
        assert_eq!(result, b"[a2o]X");
    }

    // ── Global chain tests ──

    #[test]
    fn test_global_chain_default_none() {
        let registry = CodecRegistry::new();
        assert!(registry.global_chain().is_none());
    }

    #[test]
    fn test_set_global_chain() {
        let mut registry = CodecRegistry::new();
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(TagPlugin {
            tag: "global".into(),
        }));
        registry.set_global_chain(chain);
        assert!(registry.global_chain().is_some());
        assert_eq!(registry.global_chain().unwrap().plugin_names(), vec!["global"]);
    }

    #[test]
    fn test_global_chain_independent_of_provider() {
        let mut registry = CodecRegistry::new();

        // Register a provider chain
        let mut provider_chain = ProxyPluginChain::new();
        provider_chain.add(Box::new(TagPlugin {
            tag: "provider".into(),
        }));
        registry.register_provider("openai", provider_chain);

        // Register global chain
        let mut global = ProxyPluginChain::new();
        global.add(Box::new(TagPlugin {
            tag: "global".into(),
        }));
        registry.set_global_chain(global);

        // Global chain is always available
        assert!(registry.global_chain().is_some());
        // Provider chain still works independently
        assert!(registry.get_chain("openai", "a", "b").is_some());
        // Non-matching provider has no codec chain
        assert!(registry.get_chain("anthropic", "a", "b").is_none());
    }
}
