//! Proxy plugin chain — executes plugins in sequence for request,
//! reverse sequence for response.
//!
//! Request phases:  `on_request_headers` → `on_request_body` — plugins run **in order** (A→B→C).
//! Response phases: `on_response_headers` → `on_response_body` — plugins run **in reverse** (C→B→A).

use super::{Action, ContentKind, ProxyContext, ProxyPlugin};

// ════════════════════════════════════════════════
// PluginConfig
// ════════════════════════════════════════════════

/// Configuration for a single plugin in the chain.
pub struct PluginConfig {
    /// Plugin name (cached from `plugin.name()` at registration).
    pub name: String,
    /// The plugin implementation.
    pub plugin: Box<dyn ProxyPlugin>,
    /// Whether this plugin is enabled.
    pub enabled: bool,
}

// ════════════════════════════════════════════════
// ProxyPluginChain
// ════════════════════════════════════════════════

/// The plugin chain.
pub struct ProxyPluginChain {
    plugins: Vec<PluginConfig>,
}

impl ProxyPluginChain {
    /// Create an empty plugin chain.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Add a plugin to the chain (enabled by default).
    pub fn add(&mut self, plugin: Box<dyn ProxyPlugin>) {
        let name = plugin.name().to_string();
        self.plugins.push(PluginConfig {
            name,
            plugin,
            enabled: true,
        });
    }

    /// Add a plugin with config JSON. The plugin's `on_config` is called immediately.
    pub fn add_with_config(
        &mut self,
        mut plugin: Box<dyn ProxyPlugin>,
        config: &serde_json::Value,
    ) -> Result<(), String> {
        plugin.on_config(config)?;
        let name = plugin.name().to_string();
        self.plugins.push(PluginConfig {
            name,
            plugin,
            enabled: true,
        });
        Ok(())
    }

    /// Process request headers through the chain (in order).
    /// Returns `Action::Stop` as soon as any plugin returns Stop.
    pub fn process_request_headers(&self, ctx: &mut ProxyContext) -> Action {
        for entry in &self.plugins {
            if !entry.enabled {
                continue;
            }
            if entry.plugin.on_request_headers(ctx) == Action::Stop {
                return Action::Stop;
            }
        }
        Action::Continue
    }

    /// Process request body through the chain (in order).
    /// Each plugin transforms the body; short-circuits on error.
    pub fn process_request_body(
        &self,
        ctx: &mut ProxyContext,
        mut body: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        for entry in &self.plugins {
            if !entry.enabled {
                continue;
            }
            body = entry.plugin.on_request_body(ctx, body)?;
        }
        Ok(body)
    }

    /// Process response headers through the chain (in REVERSE order).
    /// Returns `Action::Stop` as soon as any plugin returns Stop.
    pub fn process_response_headers(&self, ctx: &mut ProxyContext) -> Action {
        for entry in self.plugins.iter().rev() {
            if !entry.enabled {
                continue;
            }
            if entry.plugin.on_response_headers(ctx) == Action::Stop {
                return Action::Stop;
            }
        }
        Action::Continue
    }

    /// Process response body through the chain (in REVERSE order).
    /// Each plugin transforms the body; short-circuits on error.
    pub fn process_response_body(
        &self,
        ctx: &mut ProxyContext,
        mut body: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        for entry in self.plugins.iter().rev() {
            if !entry.enabled {
                continue;
            }
            body = entry.plugin.on_response_body(ctx, body)?;
        }
        Ok(body)
    }

    /// Process a streaming response chunk through the chain (in REVERSE order).
    /// If any plugin returns `None`, the chunk is dropped (filtered out).
    pub fn process_response_stream_chunk(
        &self,
        ctx: &mut ProxyContext,
        chunk: &str,
    ) -> Result<Option<String>, String> {
        let mut current = chunk.to_string();
        for entry in self.plugins.iter().rev() {
            if !entry.enabled {
                continue;
            }
            match entry.plugin.on_response_stream_chunk(ctx, &current)? {
                Some(transformed) => current = transformed,
                None => return Ok(None),
            }
        }
        Ok(Some(current))
    }

    /// Process binary request body through the chain (in order).
    /// Only calls plugins whose `supported_content_kinds()` includes Binary or Multipart.
    pub fn process_request_binary(
        &self,
        ctx: &mut ProxyContext,
        mut body: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        let target = ctx.request_content_kind;
        for entry in &self.plugins {
            if !entry.enabled {
                continue;
            }
            if !entry.plugin.supported_content_kinds().contains(&target) {
                continue;
            }
            body = entry.plugin.on_request_binary(ctx, body)?;
        }
        Ok(body)
    }

    /// Process binary response body through the chain (in REVERSE order).
    /// Only calls plugins whose `supported_content_kinds()` includes Binary or Multipart.
    pub fn process_response_binary(
        &self,
        ctx: &mut ProxyContext,
        mut body: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        let target = ctx.response_content_kind;
        for entry in self.plugins.iter().rev() {
            if !entry.enabled {
                continue;
            }
            if !entry.plugin.supported_content_kinds().contains(&target) {
                continue;
            }
            body = entry.plugin.on_response_binary(ctx, body)?;
        }
        Ok(body)
    }

    /// Process a binary streaming response chunk through the chain (in REVERSE order).
    /// Only calls plugins whose `supported_content_kinds()` includes the response content kind.
    pub fn process_response_binary_chunk(
        &self,
        ctx: &mut ProxyContext,
        chunk: &[u8],
    ) -> Result<Option<Vec<u8>>, String> {
        let target = ctx.response_content_kind;
        let mut current = chunk.to_vec();
        for entry in self.plugins.iter().rev() {
            if !entry.enabled {
                continue;
            }
            if !entry.plugin.supported_content_kinds().contains(&target) {
                continue;
            }
            match entry.plugin.on_response_binary_chunk(ctx, &current)? {
                Some(transformed) => current = transformed,
                None => return Ok(None),
            }
        }
        Ok(Some(current))
    }

    /// Process log phase through the chain (in order).
    /// Called at request completion for metering, billing, auditing.
    pub fn process_log(&self, ctx: &mut ProxyContext) {
        for entry in &self.plugins {
            if !entry.enabled {
                continue;
            }
            entry.plugin.on_log(ctx);
        }
    }

    /// Enable or disable a plugin by name.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        for entry in &mut self.plugins {
            if entry.name == name {
                entry.enabled = enabled;
            }
        }
    }

    /// List plugin names (all, regardless of enabled state).
    pub fn plugin_names(&self) -> Vec<&str> {
        self.plugins.iter().map(|e| e.name.as_str()).collect()
    }
}

impl Default for ProxyPluginChain {
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

    // ── Test plugins ──

    /// Plugin that does nothing — all default implementations.
    struct PassthroughPlugin;

    impl ProxyPlugin for PassthroughPlugin {
        fn name(&self) -> &str {
            "passthrough"
        }
    }

    /// A plugin that transforms request/response for testing.
    struct MockTransformPlugin {
        /// Prefix to add to request body.
        request_prefix: Vec<u8>,
        /// Prefix to add to response body.
        response_prefix: Vec<u8>,
        /// Header to inject on request.
        inject_header: Option<(String, String)>,
        /// If true, returns Action::Stop on request headers.
        stop_on_request: bool,
        /// If true, returns Action::Stop on response headers.
        stop_on_response: bool,
        /// If set, filters out stream chunks containing this substring.
        filter_stream_substring: Option<String>,
    }

    impl MockTransformPlugin {
        fn new(name_suffix: &str) -> Self {
            Self {
                request_prefix: format!("[{}]", name_suffix).into_bytes(),
                response_prefix: format!("<{}>", name_suffix).into_bytes(),
                inject_header: None,
                stop_on_request: false,
                stop_on_response: false,
                filter_stream_substring: None,
            }
        }

        fn with_stop_on_request(mut self) -> Self {
            self.stop_on_request = true;
            self
        }

        fn with_stop_on_response(mut self) -> Self {
            self.stop_on_response = true;
            self
        }

        fn with_header(mut self, key: &str, value: &str) -> Self {
            self.inject_header = Some((key.to_string(), value.to_string()));
            self
        }

        fn with_stream_filter(mut self, substring: &str) -> Self {
            self.filter_stream_substring = Some(substring.to_string());
            self
        }
    }

    /// We store the name on the struct since we need distinct names.
    struct NamedMockPlugin {
        name: String,
        inner: MockTransformPlugin,
    }

    impl NamedMockPlugin {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                inner: MockTransformPlugin::new(name),
            }
        }

        fn with_inner(name: &str, inner: MockTransformPlugin) -> Self {
            Self {
                name: name.to_string(),
                inner,
            }
        }
    }

    impl ProxyPlugin for NamedMockPlugin {
        fn name(&self) -> &str {
            &self.name
        }

        fn on_request_headers(&self, ctx: &mut ProxyContext) -> Action {
            if let Some((ref k, ref v)) = self.inner.inject_header {
                ctx.request_headers.insert(k.clone(), v.clone());
            }
            if self.inner.stop_on_request {
                Action::Stop
            } else {
                Action::Continue
            }
        }

        fn on_request_body(
            &self,
            _ctx: &mut ProxyContext,
            body: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            let mut new_body = self.inner.request_prefix.clone();
            new_body.extend_from_slice(&body);
            Ok(new_body)
        }

        fn on_response_headers(&self, _ctx: &mut ProxyContext) -> Action {
            if self.inner.stop_on_response {
                Action::Stop
            } else {
                Action::Continue
            }
        }

        fn on_response_body(
            &self,
            _ctx: &mut ProxyContext,
            body: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            let mut new_body = self.inner.response_prefix.clone();
            new_body.extend_from_slice(&body);
            Ok(new_body)
        }

        fn on_response_stream_chunk(
            &self,
            _ctx: &mut ProxyContext,
            chunk: &str,
        ) -> Result<Option<String>, String> {
            if let Some(ref filter) = self.inner.filter_stream_substring {
                if chunk.contains(filter.as_str()) {
                    return Ok(None);
                }
            }
            // Prefix the chunk with plugin name for ordering verification.
            Ok(Some(format!("[{}]{}", self.name, chunk)))
        }
    }

    // ── Empty chain tests ──

    #[test]
    fn test_empty_chain_request_headers() {
        let chain = ProxyPluginChain::new();
        let mut ctx = ProxyContext::new();
        assert_eq!(chain.process_request_headers(&mut ctx), Action::Continue);
    }

    #[test]
    fn test_empty_chain_request_body() {
        let chain = ProxyPluginChain::new();
        let mut ctx = ProxyContext::new();
        let body = b"hello".to_vec();
        assert_eq!(
            chain.process_request_body(&mut ctx, body.clone()).unwrap(),
            body
        );
    }

    #[test]
    fn test_empty_chain_response_headers() {
        let chain = ProxyPluginChain::new();
        let mut ctx = ProxyContext::new();
        assert_eq!(chain.process_response_headers(&mut ctx), Action::Continue);
    }

    #[test]
    fn test_empty_chain_response_body() {
        let chain = ProxyPluginChain::new();
        let mut ctx = ProxyContext::new();
        let body = b"response".to_vec();
        assert_eq!(
            chain.process_response_body(&mut ctx, body.clone()).unwrap(),
            body
        );
    }

    #[test]
    fn test_empty_chain_stream_chunk() {
        let chain = ProxyPluginChain::new();
        let mut ctx = ProxyContext::new();
        let result = chain
            .process_response_stream_chunk(&mut ctx, "data: test\n\n")
            .unwrap();
        assert_eq!(result, Some("data: test\n\n".to_string()));
    }

    // ── Single plugin tests ──

    #[test]
    fn test_single_plugin_request_header_injection() {
        let mut chain = ProxyPluginChain::new();
        let plugin = NamedMockPlugin::with_inner(
            "injector",
            MockTransformPlugin::new("injector").with_header("x-custom", "value1"),
        );
        chain.add(Box::new(plugin));

        let mut ctx = ProxyContext::new();
        let action = chain.process_request_headers(&mut ctx);
        assert_eq!(action, Action::Continue);
        assert_eq!(
            ctx.request_headers.get("x-custom"),
            Some(&"value1".to_string())
        );
    }

    #[test]
    fn test_single_plugin_request_body_transform() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(NamedMockPlugin::new("A")));

        let mut ctx = ProxyContext::new();
        let result = chain
            .process_request_body(&mut ctx, b"body".to_vec())
            .unwrap();
        assert_eq!(result, b"[A]body");
    }

    #[test]
    fn test_single_plugin_response_body_transform() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(NamedMockPlugin::new("A")));

        let mut ctx = ProxyContext::new();
        let result = chain
            .process_response_body(&mut ctx, b"resp".to_vec())
            .unwrap();
        assert_eq!(result, b"<A>resp");
    }

    // ── Multi-plugin order tests ──

    #[test]
    fn test_multi_plugin_request_body_order() {
        // Request: A → B → C
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(NamedMockPlugin::new("A")));
        chain.add(Box::new(NamedMockPlugin::new("B")));
        chain.add(Box::new(NamedMockPlugin::new("C")));

        let mut ctx = ProxyContext::new();
        let result = chain.process_request_body(&mut ctx, b"X".to_vec()).unwrap();
        // A prefixes [A], then B prefixes [B], then C prefixes [C]
        assert_eq!(result, b"[C][B][A]X");
    }

    #[test]
    fn test_multi_plugin_response_body_reverse_order() {
        // Response: C → B → A (reverse)
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(NamedMockPlugin::new("A")));
        chain.add(Box::new(NamedMockPlugin::new("B")));
        chain.add(Box::new(NamedMockPlugin::new("C")));

        let mut ctx = ProxyContext::new();
        let result = chain
            .process_response_body(&mut ctx, b"X".to_vec())
            .unwrap();
        // Reverse order: C prefixes <C>, then B prefixes <B>, then A prefixes <A>
        assert_eq!(result, b"<A><B><C>X");
    }

    #[test]
    fn test_multi_plugin_stream_chunk_reverse_order() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(NamedMockPlugin::new("A")));
        chain.add(Box::new(NamedMockPlugin::new("B")));
        chain.add(Box::new(NamedMockPlugin::new("C")));

        let mut ctx = ProxyContext::new();
        let result = chain
            .process_response_stream_chunk(&mut ctx, "data")
            .unwrap();
        // Reverse: C then B then A
        assert_eq!(result, Some("[A][B][C]data".to_string()));
    }

    // ── Action::Stop tests ──

    #[test]
    fn test_stop_on_request_headers() {
        let mut chain = ProxyPluginChain::new();
        // A adds a header, B stops, C should never run.
        chain.add(Box::new(NamedMockPlugin::with_inner(
            "A",
            MockTransformPlugin::new("A").with_header("x-a", "1"),
        )));
        chain.add(Box::new(NamedMockPlugin::with_inner(
            "B",
            MockTransformPlugin::new("B").with_stop_on_request(),
        )));
        chain.add(Box::new(NamedMockPlugin::with_inner(
            "C",
            MockTransformPlugin::new("C").with_header("x-c", "3"),
        )));

        let mut ctx = ProxyContext::new();
        let action = chain.process_request_headers(&mut ctx);
        assert_eq!(action, Action::Stop);
        // A ran (injected x-a), but C did not run (no x-c).
        assert_eq!(ctx.request_headers.get("x-a"), Some(&"1".to_string()));
        assert_eq!(ctx.request_headers.get("x-c"), None);
    }

    #[test]
    fn test_stop_on_response_headers() {
        let mut chain = ProxyPluginChain::new();
        // In reverse order: C runs first, then B (which stops), then A never runs.
        chain.add(Box::new(NamedMockPlugin::with_inner(
            "A",
            MockTransformPlugin::new("A").with_header("x-a", "1"),
        )));
        chain.add(Box::new(NamedMockPlugin::with_inner(
            "B",
            MockTransformPlugin::new("B").with_stop_on_response(),
        )));
        chain.add(Box::new(NamedMockPlugin::new("C")));

        let mut ctx = ProxyContext::new();
        let action = chain.process_response_headers(&mut ctx);
        assert_eq!(action, Action::Stop);
    }

    // ── Stream chunk filtering ──

    #[test]
    fn test_stream_chunk_filter() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(NamedMockPlugin::with_inner(
            "filter",
            MockTransformPlugin::new("filter").with_stream_filter("[DONE]"),
        )));

        let mut ctx = ProxyContext::new();

        // Normal chunk passes through.
        let result = chain
            .process_response_stream_chunk(&mut ctx, "data: {}")
            .unwrap();
        assert!(result.is_some());

        // Chunk containing [DONE] is filtered out.
        let result = chain
            .process_response_stream_chunk(&mut ctx, "data: [DONE]")
            .unwrap();
        assert_eq!(result, None);
    }

    // ── Enable/disable tests ──

    #[test]
    fn test_disable_plugin() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(NamedMockPlugin::new("A")));
        chain.add(Box::new(NamedMockPlugin::new("B")));

        // Disable B → only A should transform.
        chain.set_enabled("B", false);

        let mut ctx = ProxyContext::new();
        let result = chain.process_request_body(&mut ctx, b"X".to_vec()).unwrap();
        assert_eq!(result, b"[A]X");
    }

    #[test]
    fn test_reenable_plugin() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(NamedMockPlugin::new("A")));
        chain.add(Box::new(NamedMockPlugin::new("B")));

        chain.set_enabled("B", false);
        chain.set_enabled("B", true);

        let mut ctx = ProxyContext::new();
        let result = chain.process_request_body(&mut ctx, b"X".to_vec()).unwrap();
        assert_eq!(result, b"[B][A]X");
    }

    #[test]
    fn test_disable_all_plugins() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(NamedMockPlugin::new("A")));
        chain.add(Box::new(NamedMockPlugin::new("B")));

        chain.set_enabled("A", false);
        chain.set_enabled("B", false);

        let mut ctx = ProxyContext::new();
        let body = b"original".to_vec();
        let result = chain.process_request_body(&mut ctx, body.clone()).unwrap();
        assert_eq!(result, body);
    }

    // ── plugin_names ──

    #[test]
    fn test_plugin_names() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(NamedMockPlugin::new("alpha")));
        chain.add(Box::new(NamedMockPlugin::new("beta")));
        chain.add(Box::new(NamedMockPlugin::new("gamma")));

        assert_eq!(chain.plugin_names(), vec!["alpha", "beta", "gamma"]);
    }

    // ── add_with_config ──

    #[test]
    fn test_add_with_config() {
        let mut chain = ProxyPluginChain::new();

        struct ConfigPlugin {
            prefix: String,
        }

        impl ProxyPlugin for ConfigPlugin {
            fn name(&self) -> &str {
                "config-plugin"
            }

            fn on_config(&mut self, config: &serde_json::Value) -> Result<(), String> {
                self.prefix = config
                    .get("prefix")
                    .and_then(|v| v.as_str())
                    .ok_or("missing prefix")?
                    .to_string();
                Ok(())
            }

            fn on_request_body(
                &self,
                _ctx: &mut ProxyContext,
                body: Vec<u8>,
            ) -> Result<Vec<u8>, String> {
                let mut new = self.prefix.as_bytes().to_vec();
                new.extend_from_slice(&body);
                Ok(new)
            }
        }

        let plugin = ConfigPlugin {
            prefix: String::new(),
        };
        let config = serde_json::json!({"prefix": ">>>"});
        chain.add_with_config(Box::new(plugin), &config).unwrap();

        let mut ctx = ProxyContext::new();
        let result = chain
            .process_request_body(&mut ctx, b"data".to_vec())
            .unwrap();
        assert_eq!(result, b">>>data");
    }

    #[test]
    fn test_add_with_config_error() {
        let mut chain = ProxyPluginChain::new();

        struct FailConfigPlugin;

        impl ProxyPlugin for FailConfigPlugin {
            fn name(&self) -> &str {
                "fail-config"
            }

            fn on_config(&mut self, _config: &serde_json::Value) -> Result<(), String> {
                Err("bad config".to_string())
            }
        }

        let result = chain.add_with_config(Box::new(FailConfigPlugin), &serde_json::json!({}));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "bad config");
        // Plugin should not have been added.
        assert!(chain.plugin_names().is_empty());
    }

    // ── Passthrough plugin in chain ──

    #[test]
    fn test_passthrough_plugin_in_chain() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(PassthroughPlugin));

        let mut ctx = ProxyContext::new();
        assert_eq!(chain.process_request_headers(&mut ctx), Action::Continue);

        let body = b"unchanged".to_vec();
        assert_eq!(
            chain.process_request_body(&mut ctx, body.clone()).unwrap(),
            body
        );
        assert_eq!(
            chain.process_response_body(&mut ctx, body.clone()).unwrap(),
            body
        );
        assert_eq!(chain.process_response_headers(&mut ctx), Action::Continue);

        let chunk = chain
            .process_response_stream_chunk(&mut ctx, "test")
            .unwrap();
        assert_eq!(chunk, Some("test".to_string()));
    }

    // ── on_log tests ──

    #[test]
    fn test_process_log_empty_chain() {
        let chain = ProxyPluginChain::new();
        let mut ctx = ProxyContext::new();
        chain.process_log(&mut ctx); // should not panic
    }

    #[test]
    fn test_process_log_records_data() {
        struct MeteringPlugin;

        impl ProxyPlugin for MeteringPlugin {
            fn name(&self) -> &str {
                "metering"
            }

            fn on_log(&self, ctx: &mut ProxyContext) {
                // Simulate metering: store a billing record in ctx data
                let variant = ctx.selected_variant.clone();
                let status = ctx.response_status.unwrap_or(0);
                ctx.set("billing_record", format!("{}:{}", variant, status));
            }
        }

        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(MeteringPlugin));

        let mut ctx = ProxyContext::new();
        ctx.selected_variant = "gpt4-fast".to_string();
        ctx.response_status = Some(200);
        chain.process_log(&mut ctx);

        assert_eq!(
            ctx.get::<String>("billing_record"),
            Some(&"gpt4-fast:200".to_string())
        );
    }

    #[test]
    fn test_process_log_multi_plugin_order() {
        struct AppendPlugin {
            tag: String,
        }

        impl ProxyPlugin for AppendPlugin {
            fn name(&self) -> &str {
                "append"
            }

            fn on_log(&self, ctx: &mut ProxyContext) {
                let current = ctx.get::<String>("log_order").cloned().unwrap_or_default();
                ctx.set("log_order", format!("{}{}", current, self.tag));
            }
        }

        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(AppendPlugin { tag: "A".into() }));
        chain.add(Box::new(AppendPlugin { tag: "B".into() }));
        chain.add(Box::new(AppendPlugin { tag: "C".into() }));

        let mut ctx = ProxyContext::new();
        chain.process_log(&mut ctx);

        // on_log runs in order (same as request)
        assert_eq!(ctx.get::<String>("log_order"), Some(&"ABC".to_string()));
    }

    #[test]
    fn test_process_log_disabled_plugin_skipped() {
        struct CounterPlugin;

        impl ProxyPlugin for CounterPlugin {
            fn name(&self) -> &str {
                "counter"
            }

            fn on_log(&self, ctx: &mut ProxyContext) {
                let count = ctx.get::<u32>("count").copied().unwrap_or(0);
                ctx.set("count", count + 1);
            }
        }

        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(CounterPlugin));

        // Rename for set_enabled — need named plugin
        struct NamedCounter(String);
        impl ProxyPlugin for NamedCounter {
            fn name(&self) -> &str {
                &self.0
            }
            fn on_log(&self, ctx: &mut ProxyContext) {
                let count = ctx.get::<u32>("count").copied().unwrap_or(0);
                ctx.set("count", count + 1);
            }
        }

        let mut chain2 = ProxyPluginChain::new();
        chain2.add(Box::new(NamedCounter("a".into())));
        chain2.add(Box::new(NamedCounter("b".into())));
        chain2.set_enabled("b", false);

        let mut ctx = ProxyContext::new();
        chain2.process_log(&mut ctx);
        assert_eq!(ctx.get::<u32>("count"), Some(&1));
    }

    // ── Binary/multipart dispatch tests ──

    /// Plugin that only supports Binary content kind.
    struct BinaryOnlyPlugin {
        name: String,
    }

    impl ProxyPlugin for BinaryOnlyPlugin {
        fn name(&self) -> &str {
            &self.name
        }

        fn supported_content_kinds(&self) -> Vec<ContentKind> {
            vec![ContentKind::Binary]
        }

        fn on_request_binary(
            &self,
            _ctx: &mut ProxyContext,
            mut body: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            // Prepend a marker byte
            let mut new = vec![0xBB];
            new.append(&mut body);
            Ok(new)
        }

        fn on_response_binary(
            &self,
            _ctx: &mut ProxyContext,
            mut body: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            let mut new = vec![0xAA];
            new.append(&mut body);
            Ok(new)
        }

        fn on_response_binary_chunk(
            &self,
            _ctx: &mut ProxyContext,
            chunk: &[u8],
        ) -> Result<Option<Vec<u8>>, String> {
            let mut new = vec![0xCC];
            new.extend_from_slice(chunk);
            Ok(Some(new))
        }
    }

    #[test]
    fn test_binary_plugin_called_for_binary_request() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(BinaryOnlyPlugin { name: "bin".into() }));

        let mut ctx = ProxyContext::new();
        ctx.request_content_kind = ContentKind::Binary;
        let body = vec![0x01, 0x02];
        let result = chain.process_request_binary(&mut ctx, body).unwrap();
        assert_eq!(result, vec![0xBB, 0x01, 0x02]);
    }

    #[test]
    fn test_binary_plugin_skipped_for_json_request() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(BinaryOnlyPlugin { name: "bin".into() }));

        let mut ctx = ProxyContext::new();
        ctx.request_content_kind = ContentKind::Json; // Binary plugin won't match
        let body = vec![0x01, 0x02];
        let result = chain
            .process_request_binary(&mut ctx, body.clone())
            .unwrap();
        // Body unchanged — plugin was skipped
        assert_eq!(result, body);
    }

    #[test]
    fn test_json_default_plugin_skipped_for_binary() {
        // PassthroughPlugin has default supported_content_kinds = [Json]
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(PassthroughPlugin));

        let mut ctx = ProxyContext::new();
        ctx.request_content_kind = ContentKind::Binary;
        let body = vec![0x01, 0x02];
        let result = chain
            .process_request_binary(&mut ctx, body.clone())
            .unwrap();
        // PassthroughPlugin only supports Json, so it's skipped
        assert_eq!(result, body);
    }

    #[test]
    fn test_binary_response_dispatch() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(BinaryOnlyPlugin { name: "bin".into() }));

        let mut ctx = ProxyContext::new();
        ctx.response_content_kind = ContentKind::Binary;
        let body = vec![0x10, 0x20];
        let result = chain.process_response_binary(&mut ctx, body).unwrap();
        assert_eq!(result, vec![0xAA, 0x10, 0x20]);
    }

    #[test]
    fn test_binary_response_chunk_dispatch() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(BinaryOnlyPlugin { name: "bin".into() }));

        let mut ctx = ProxyContext::new();
        ctx.response_content_kind = ContentKind::Binary;
        let chunk = &[0x30, 0x40];
        let result = chain
            .process_response_binary_chunk(&mut ctx, chunk)
            .unwrap();
        assert_eq!(result, Some(vec![0xCC, 0x30, 0x40]));
    }

    #[test]
    fn test_binary_passthrough_no_mangling() {
        // Empty chain: binary data passes through untouched
        let chain = ProxyPluginChain::new();
        let mut ctx = ProxyContext::new();
        ctx.response_content_kind = ContentKind::Binary;

        let binary_data = (0..256).map(|i| i as u8).collect::<Vec<u8>>();
        let result = chain
            .process_response_binary(&mut ctx, binary_data.clone())
            .unwrap();
        assert_eq!(result, binary_data);
    }

    #[test]
    fn test_sse_streaming_still_works_regression() {
        // Ensure existing SSE/string streaming path is unaffected
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(NamedMockPlugin::new("A")));

        let mut ctx = ProxyContext::new();
        let result = chain
            .process_response_stream_chunk(&mut ctx, "data: {\"text\":\"hello\"}\n\n")
            .unwrap();
        assert_eq!(
            result,
            Some("[A]data: {\"text\":\"hello\"}\n\n".to_string())
        );
    }

    #[test]
    fn test_multi_binary_plugin_reverse_order() {
        // Response binary should run in reverse order like response body
        struct MarkerPlugin {
            name: String,
            marker: u8,
        }
        impl ProxyPlugin for MarkerPlugin {
            fn name(&self) -> &str {
                &self.name
            }
            fn supported_content_kinds(&self) -> Vec<ContentKind> {
                vec![ContentKind::Binary]
            }
            fn on_response_binary(
                &self,
                _ctx: &mut ProxyContext,
                mut body: Vec<u8>,
            ) -> Result<Vec<u8>, String> {
                let mut new = vec![self.marker];
                new.append(&mut body);
                Ok(new)
            }
        }

        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(MarkerPlugin {
            name: "A".into(),
            marker: 0xAA,
        }));
        chain.add(Box::new(MarkerPlugin {
            name: "B".into(),
            marker: 0xBB,
        }));
        chain.add(Box::new(MarkerPlugin {
            name: "C".into(),
            marker: 0xCC,
        }));

        let mut ctx = ProxyContext::new();
        ctx.response_content_kind = ContentKind::Binary;
        let result = chain.process_response_binary(&mut ctx, vec![0xFF]).unwrap();
        // Reverse: C then B then A
        assert_eq!(result, vec![0xAA, 0xBB, 0xCC, 0xFF]);
    }
}
