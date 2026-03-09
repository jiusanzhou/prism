use std::collections::HashMap;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::types::TaskType;

// ──────────────────────────────────────────────
// Request profile types
// ──────────────────────────────────────────────

/// How large the effective context window needs to be
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextSize {
    /// < 4 K tokens
    Small,
    /// 4 K – 32 K
    Medium,
    /// 32 K – 128 K
    Large,
    /// > 128 K
    XLarge,
}

/// Primary language of the request
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    English,
    Chinese,
    Japanese,
    Korean,
    Other,
}

/// The output of request analysis – consumed by the strategy engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestProfile {
    /// Complexity score in [0.0, 1.0]
    pub complexity: f32,
    /// Classified task type
    pub task_type: TaskType,
    /// Estimated total tokens (input + output)
    pub estimated_tokens: u32,
    /// Whether the request needs reasoning capabilities
    pub requires_reasoning: bool,
    /// Whether the request contains images / multi-modal content
    pub requires_vision: bool,
    /// Detected language
    pub language: Language,
    /// Multi-turn conversation?
    pub is_multi_turn: bool,
    /// Required context window bucket
    pub context_window_need: ContextSize,
}

// ──────────────────────────────────────────────
// Analyze context
// ──────────────────────────────────────────────

/// Context about the request source, used for conditional rule loading.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalyzeContext {
    /// Caller type: "user", "service", "internal", etc.
    pub caller_type: Option<String>,
    /// Source platform: "web", "mobile", "api", "slack", "discord", etc.
    pub source: Option<String>,
    /// Tenant or org identifier
    pub tenant_id: Option<String>,
    /// Custom tags for rule matching
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

// ──────────────────────────────────────────────
// Analyzer trait
// ──────────────────────────────────────────────

/// A simplified chat message used for analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Trait for analysing an incoming request.
pub trait RequestAnalyzer: Send + Sync {
    fn analyze(&self, messages: &[ChatMessage]) -> RequestProfile;

    /// Analyze with context — default falls back to analyze()
    fn analyze_with_context(
        &self,
        messages: &[ChatMessage],
        _ctx: &AnalyzeContext,
    ) -> RequestProfile {
        self.analyze(messages)
    }
}

// ──────────────────────────────────────────────
// Rule-based analyzer (original, keyword-based)
// ──────────────────────────────────────────────

/// A fast, keyword-based analyzer (L0 / rule layer).
pub struct RuleBasedAnalyzer;

impl RuleBasedAnalyzer {
    pub fn new() -> Self {
        Self
    }

    /// Shared analysis logic used by both this analyzer and as fallback for others.
    pub fn analyze_text(messages: &[ChatMessage]) -> RequestProfile {
        // Concatenate all user messages for keyword scanning
        let text: String = messages
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        let text_lower = text.to_lowercase();
        let total_chars = text.len();

        // Very rough token estimate (1 token ≈ 4 chars for English, 2 for CJK)
        let estimated_tokens = (total_chars as u32 / 3).max(1);

        // Task type detection via keywords
        let task_type = if text_lower.contains("code")
            || text_lower.contains("function")
            || text_lower.contains("implement")
            || text_lower.contains("程序")
            || text_lower.contains("代码")
        {
            TaskType::CodeGeneration
        } else if text_lower.contains("reason")
            || text_lower.contains("why")
            || text_lower.contains("explain step")
            || text_lower.contains("推理")
            || text_lower.contains("分析")
        {
            TaskType::Reasoning
        } else if text_lower.contains("translate") || text_lower.contains("翻译") {
            TaskType::Translation
        } else if text_lower.contains("summarize")
            || text_lower.contains("summary")
            || text_lower.contains("总结")
            || text_lower.contains("摘要")
        {
            TaskType::Summarization
        } else if text_lower.contains("write")
            || text_lower.contains("story")
            || text_lower.contains("poem")
            || text_lower.contains("写")
            || text_lower.contains("创作")
        {
            TaskType::Creative
        } else if text_lower.contains("image")
            || text_lower.contains("picture")
            || text_lower.contains("看")
            || text_lower.contains("图")
        {
            TaskType::MultiModal
        } else if text_lower.contains("math")
            || text_lower.contains("calculate")
            || text_lower.contains("计算")
            || text_lower.contains("数学")
        {
            TaskType::Math
        } else {
            TaskType::SimpleQA
        };

        let requires_reasoning = matches!(task_type, TaskType::Reasoning | TaskType::Math);
        let requires_vision = matches!(task_type, TaskType::MultiModal);

        let context_window_need = match estimated_tokens {
            0..=4000 => ContextSize::Small,
            4001..=32000 => ContextSize::Medium,
            32001..=128000 => ContextSize::Large,
            _ => ContextSize::XLarge,
        };

        let complexity = match task_type {
            TaskType::SimpleQA | TaskType::Conversation => 0.2,
            TaskType::Translation | TaskType::Summarization => 0.4,
            TaskType::Creative | TaskType::DataExtraction => 0.5,
            TaskType::CodeGeneration | TaskType::CodeReview => 0.7,
            TaskType::Reasoning | TaskType::Math => 0.8,
            TaskType::MultiModal => 0.6,
            TaskType::Unknown => 0.5,
        };

        let is_multi_turn = messages.len() > 2;

        // Simple language detection
        let language = if text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
            Language::Chinese
        } else {
            Language::English
        };

        RequestProfile {
            complexity,
            task_type,
            estimated_tokens,
            requires_reasoning,
            requires_vision,
            language,
            is_multi_turn,
            context_window_need,
        }
    }
}

impl Default for RuleBasedAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestAnalyzer for RuleBasedAnalyzer {
    fn analyze(&self, messages: &[ChatMessage]) -> RequestProfile {
        Self::analyze_text(messages)
    }
}

// ──────────────────────────────────────────────
// Configurable rule analyzer
// ──────────────────────────────────────────────

/// Top-level configuration for the configurable rule analyzer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzerConfig {
    /// Default rules applied when no scope matches
    pub default_rules: Vec<AnalyzerRule>,
    /// Scoped rule sets: matched by caller_type, source, tenant_id, or tags
    #[serde(default)]
    pub scopes: Vec<RuleScope>,
}

/// A scoped rule set that activates when context matches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleScope {
    /// Name for debugging
    pub name: String,
    /// Match conditions (all must match — AND logic)
    pub match_on: RuleScopeMatch,
    /// Rules to apply when this scope matches
    pub rules: Vec<AnalyzerRule>,
}

/// Conditions for a scope to match (AND across fields, OR within each field).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleScopeMatch {
    /// Caller types to match (OR within)
    pub caller_types: Option<Vec<String>>,
    /// Source platforms to match (OR within)
    pub sources: Option<Vec<String>>,
    /// Tenant IDs to match (OR within)
    pub tenant_ids: Option<Vec<String>>,
    /// Tags that must all match (AND within)
    pub tags: Option<HashMap<String, String>>,
}

/// A single analysis rule with keyword/pattern matching and task assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzerRule {
    /// Keywords to match (case-insensitive, OR logic)
    pub keywords: Vec<String>,
    /// Optional regex patterns (OR logic)
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Task type to assign on match
    pub task_type: TaskType,
    /// Complexity override
    pub complexity: f32,
    /// Force reasoning requirement
    #[serde(default)]
    pub require_reasoning: bool,
    /// Force vision requirement
    #[serde(default)]
    pub require_vision: bool,
    /// Priority (higher = checked first, default 0)
    #[serde(default)]
    pub priority: i32,
}

/// Compiled version of an `AnalyzerRule` with pre-compiled regex patterns.
struct CompiledRule {
    keywords: Vec<String>,
    patterns: Vec<Regex>,
    task_type: TaskType,
    complexity: f32,
    require_reasoning: bool,
    require_vision: bool,
    #[allow(dead_code)]
    priority: i32,
}

/// Compiled scope with pre-compiled rules.
struct CompiledScope {
    #[allow(dead_code)]
    name: String,
    match_on: RuleScopeMatch,
    rules: Vec<CompiledRule>,
}

/// An analyzer that loads rules from a configuration structure.
///
/// Supports scoped rules matched by `AnalyzeContext`, with regex pattern support
/// and priority-based rule ordering. Falls back to `RuleBasedAnalyzer` behavior
/// when no rules match.
pub struct ConfigurableRuleAnalyzer {
    default_rules: Vec<CompiledRule>,
    scopes: Vec<CompiledScope>,
}

impl ConfigurableRuleAnalyzer {
    /// Create a new configurable analyzer, compiling all regex patterns upfront.
    ///
    /// # Panics
    /// Panics if any regex pattern in the config is invalid.
    pub fn new(config: AnalyzerConfig) -> Self {
        let default_rules = Self::compile_rules(config.default_rules);
        let scopes = config
            .scopes
            .into_iter()
            .map(|s| CompiledScope {
                name: s.name,
                match_on: s.match_on,
                rules: Self::compile_rules(s.rules),
            })
            .collect();

        Self {
            default_rules,
            scopes,
        }
    }

    fn compile_rules(mut rules: Vec<AnalyzerRule>) -> Vec<CompiledRule> {
        // Sort by priority descending so higher priority rules are checked first
        rules.sort_by(|a, b| b.priority.cmp(&a.priority));

        rules
            .into_iter()
            .map(|r| {
                let patterns = r
                    .patterns
                    .iter()
                    .map(|p| {
                        Regex::new(p)
                            .unwrap_or_else(|e| panic!("invalid regex pattern '{}': {}", p, e))
                    })
                    .collect();
                CompiledRule {
                    keywords: r.keywords.into_iter().map(|k| k.to_lowercase()).collect(),
                    patterns,
                    task_type: r.task_type,
                    complexity: r.complexity,
                    require_reasoning: r.require_reasoning,
                    require_vision: r.require_vision,
                    priority: r.priority,
                }
            })
            .collect()
    }

    /// Check if a scope matches the given context.
    fn scope_matches(scope: &CompiledScope, ctx: &AnalyzeContext) -> bool {
        let match_on = &scope.match_on;

        // Each present field must match (AND across fields)
        if let Some(ref caller_types) = match_on.caller_types {
            match &ctx.caller_type {
                Some(ct) => {
                    if !caller_types.iter().any(|t| t == ct) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        if let Some(ref sources) = match_on.sources {
            match &ctx.source {
                Some(src) => {
                    if !sources.iter().any(|s| s == src) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        if let Some(ref tenant_ids) = match_on.tenant_ids {
            match &ctx.tenant_id {
                Some(tid) => {
                    if !tenant_ids.iter().any(|t| t == tid) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        if let Some(ref required_tags) = match_on.tags {
            for (key, value) in required_tags {
                match ctx.tags.get(key) {
                    Some(v) if v == value => {}
                    _ => return false,
                }
            }
        }

        true
    }

    /// Try to match rules against the text. Returns the first matching rule's profile.
    fn try_match_rules(
        rules: &[CompiledRule],
        text_lower: &str,
        messages: &[ChatMessage],
    ) -> Option<RequestProfile> {
        for rule in rules {
            let keyword_match = rule
                .keywords
                .iter()
                .any(|kw| text_lower.contains(kw.as_str()));
            let pattern_match = rule.patterns.iter().any(|re| re.is_match(text_lower));

            if keyword_match || pattern_match {
                let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
                let estimated_tokens = (total_chars as u32 / 3).max(1);
                let is_multi_turn = messages.len() > 2;

                let language = {
                    let text: String = messages
                        .iter()
                        .filter(|m| m.role == "user")
                        .map(|m| m.content.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    if text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
                        Language::Chinese
                    } else {
                        Language::English
                    }
                };

                let context_window_need = match estimated_tokens {
                    0..=4000 => ContextSize::Small,
                    4001..=32000 => ContextSize::Medium,
                    32001..=128000 => ContextSize::Large,
                    _ => ContextSize::XLarge,
                };

                return Some(RequestProfile {
                    complexity: rule.complexity,
                    task_type: rule.task_type,
                    estimated_tokens,
                    requires_reasoning: rule.require_reasoning,
                    requires_vision: rule.require_vision,
                    language,
                    is_multi_turn,
                    context_window_need,
                });
            }
        }
        None
    }

    /// Extract lowercased user text from messages.
    fn extract_text_lower(messages: &[ChatMessage]) -> String {
        messages
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }
}

impl RequestAnalyzer for ConfigurableRuleAnalyzer {
    fn analyze(&self, messages: &[ChatMessage]) -> RequestProfile {
        let text_lower = Self::extract_text_lower(messages);

        // Without context, only use default rules
        if let Some(profile) = Self::try_match_rules(&self.default_rules, &text_lower, messages) {
            return profile;
        }

        // Fall back to built-in rule-based analysis
        RuleBasedAnalyzer::analyze_text(messages)
    }

    fn analyze_with_context(
        &self,
        messages: &[ChatMessage],
        ctx: &AnalyzeContext,
    ) -> RequestProfile {
        let text_lower = Self::extract_text_lower(messages);

        // Find matching scope and apply its rules
        for scope in &self.scopes {
            if Self::scope_matches(scope, ctx) {
                if let Some(profile) = Self::try_match_rules(&scope.rules, &text_lower, messages) {
                    return profile;
                }
            }
        }

        // No scope matched or no rules in scope matched — try default rules
        if let Some(profile) = Self::try_match_rules(&self.default_rules, &text_lower, messages) {
            return profile;
        }

        // Fall back to built-in rule-based analysis
        RuleBasedAnalyzer::analyze_text(messages)
    }
}

// ──────────────────────────────────────────────
// Webhook analyzer
// ──────────────────────────────────────────────

/// Request body sent to the webhook endpoint.
#[derive(Debug, Serialize)]
struct WebhookRequest<'a> {
    messages: &'a [ChatMessage],
    context: &'a AnalyzeContext,
}

/// An analyzer that calls an external HTTP endpoint for analysis.
///
/// On timeout or error, falls back to a configurable fallback analyzer.
/// Uses `ureq` for minimal blocking HTTP calls (no async runtime required).
pub struct WebhookAnalyzer {
    url: String,
    timeout: Duration,
    fallback: Box<dyn RequestAnalyzer>,
}

impl WebhookAnalyzer {
    /// Create a new webhook analyzer.
    ///
    /// - `url`: HTTP(S) endpoint to POST to
    /// - `timeout`: request timeout
    /// - `fallback`: analyzer to use when the webhook fails
    pub fn new(url: String, timeout: Duration, fallback: Box<dyn RequestAnalyzer>) -> Self {
        Self {
            url,
            timeout,
            fallback,
        }
    }

    fn call_webhook(
        &self,
        messages: &[ChatMessage],
        ctx: &AnalyzeContext,
    ) -> Result<RequestProfile, String> {
        let body = WebhookRequest {
            messages,
            context: ctx,
        };

        let response = ureq::AgentBuilder::new()
            .timeout_connect(self.timeout)
            .timeout_read(self.timeout)
            .timeout_write(self.timeout)
            .build()
            .post(&self.url)
            .set("Content-Type", "application/json")
            .send_json(serde_json::to_value(&body).map_err(|e| e.to_string())?)
            .map_err(|e| format!("webhook request failed: {}", e))?;

        let profile: RequestProfile = response
            .into_json()
            .map_err(|e| format!("failed to parse webhook response: {}", e))?;

        Ok(profile)
    }
}

impl RequestAnalyzer for WebhookAnalyzer {
    fn analyze(&self, messages: &[ChatMessage]) -> RequestProfile {
        let ctx = AnalyzeContext::default();
        self.analyze_with_context(messages, &ctx)
    }

    fn analyze_with_context(
        &self,
        messages: &[ChatMessage],
        ctx: &AnalyzeContext,
    ) -> RequestProfile {
        match self.call_webhook(messages, ctx) {
            Ok(profile) => profile,
            Err(_) => self.fallback.analyze_with_context(messages, ctx),
        }
    }
}

// ──────────────────────────────────────────────
// WASM analyzer (behind feature flag)
// ──────────────────────────────────────────────

/// An analyzer that loads and executes a WASM module for request analysis.
///
/// ## WASM Contract
///
/// The WASM module must export:
/// - `memory`: a `WebAssembly.Memory` instance
/// - `alloc(size: i32) -> i32`: allocate `size` bytes, return pointer
/// - `dealloc(ptr: i32, size: i32)`: free allocated memory
/// - `analyze(ptr: i32, len: i32) -> i32`: analyze the JSON input at `ptr`/`len`,
///   return a pointer to a length-prefixed (4-byte LE u32) JSON response
///
/// Input is JSON: `{ "messages": [...], "context": {...} }`
/// Output is JSON matching `RequestProfile`.
#[cfg(feature = "wasm")]
pub struct WasmAnalyzer {
    engine: wasmtime::Engine,
    module: wasmtime::Module,
    fallback: Box<dyn RequestAnalyzer>,
}

#[cfg(feature = "wasm")]
impl WasmAnalyzer {
    /// Create a new WASM analyzer from a precompiled module file.
    pub fn from_file(
        path: &std::path::Path,
        fallback: Box<dyn RequestAnalyzer>,
    ) -> Result<Self, String> {
        let engine = wasmtime::Engine::default();
        let module =
            wasmtime::Module::from_file(&engine, path).map_err(|e| format!("wasm load: {}", e))?;
        Ok(Self {
            engine,
            module,
            fallback,
        })
    }

    /// Create a new WASM analyzer from raw bytes.
    pub fn from_bytes(bytes: &[u8], fallback: Box<dyn RequestAnalyzer>) -> Result<Self, String> {
        let engine = wasmtime::Engine::default();
        let module =
            wasmtime::Module::new(&engine, bytes).map_err(|e| format!("wasm compile: {}", e))?;
        Ok(Self {
            engine,
            module,
            fallback,
        })
    }

    fn run_wasm(
        &self,
        messages: &[ChatMessage],
        ctx: &AnalyzeContext,
    ) -> Result<RequestProfile, String> {
        let mut store = wasmtime::Store::new(&self.engine, ());
        let instance = wasmtime::Instance::new(&mut store, &self.module, &[])
            .map_err(|e| format!("wasm instantiate: {}", e))?;

        // Get exported functions and memory
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or("wasm module must export 'memory'")?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "alloc")
            .map_err(|e| format!("wasm 'alloc' export: {}", e))?;
        let analyze_fn = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "analyze")
            .map_err(|e| format!("wasm 'analyze' export: {}", e))?;

        // Prepare input JSON
        let input = serde_json::json!({
            "messages": messages,
            "context": ctx,
        });
        let input_bytes = serde_json::to_vec(&input).map_err(|e| e.to_string())?;
        let input_len = input_bytes.len() as i32;

        // Allocate memory in WASM and write input
        let input_ptr = alloc
            .call(&mut store, input_len)
            .map_err(|e| format!("wasm alloc: {}", e))?;

        memory.data_mut(&mut store)[input_ptr as usize..input_ptr as usize + input_bytes.len()]
            .copy_from_slice(&input_bytes);

        // Call analyze
        let result_ptr = analyze_fn
            .call(&mut store, (input_ptr, input_len))
            .map_err(|e| format!("wasm analyze: {}", e))?;

        // Read result: first 4 bytes are the length (little-endian u32)
        let mem_data = memory.data(&store);
        if result_ptr as usize + 4 > mem_data.len() {
            return Err("wasm result pointer out of bounds".into());
        }
        let result_len = u32::from_le_bytes([
            mem_data[result_ptr as usize],
            mem_data[result_ptr as usize + 1],
            mem_data[result_ptr as usize + 2],
            mem_data[result_ptr as usize + 3],
        ]) as usize;

        let result_start = result_ptr as usize + 4;
        if result_start + result_len > mem_data.len() {
            return Err("wasm result data out of bounds".into());
        }
        let result_bytes = &mem_data[result_start..result_start + result_len];

        let profile: RequestProfile = serde_json::from_slice(result_bytes)
            .map_err(|e| format!("wasm result parse: {}", e))?;

        Ok(profile)
    }
}

#[cfg(feature = "wasm")]
impl RequestAnalyzer for WasmAnalyzer {
    fn analyze(&self, messages: &[ChatMessage]) -> RequestProfile {
        let ctx = AnalyzeContext::default();
        self.analyze_with_context(messages, &ctx)
    }

    fn analyze_with_context(
        &self,
        messages: &[ChatMessage],
        ctx: &AnalyzeContext,
    ) -> RequestProfile {
        match self.run_wasm(messages, ctx) {
            Ok(profile) => profile,
            Err(_) => self.fallback.analyze_with_context(messages, ctx),
        }
    }
}

// ──────────────────────────────────────────────
// Analyzer chain (combinator)
// ──────────────────────────────────────────────

/// A combinator that tries multiple analyzers in order.
///
/// When `first_definitive` is true, the first analyzer that returns a "definitive"
/// result (task_type is neither `SimpleQA` nor `Unknown`) wins. If no analyzer
/// returns a definitive result, the last result is used.
///
/// When `first_definitive` is false, the first analyzer's result always wins.
pub struct AnalyzerChain {
    analyzers: Vec<(String, Box<dyn RequestAnalyzer>)>,
    /// If true, first non-SimpleQA/Unknown result wins; otherwise first always wins
    pub first_definitive: bool,
}

impl AnalyzerChain {
    /// Create a new analyzer chain.
    pub fn new(analyzers: Vec<(String, Box<dyn RequestAnalyzer>)>, first_definitive: bool) -> Self {
        Self {
            analyzers,
            first_definitive,
        }
    }

    /// Returns true if a result is "definitive" (not SimpleQA and not Unknown).
    fn is_definitive(profile: &RequestProfile) -> bool {
        !matches!(profile.task_type, TaskType::SimpleQA | TaskType::Unknown)
    }
}

impl RequestAnalyzer for AnalyzerChain {
    fn analyze(&self, messages: &[ChatMessage]) -> RequestProfile {
        let ctx = AnalyzeContext::default();
        self.analyze_with_context(messages, &ctx)
    }

    fn analyze_with_context(
        &self,
        messages: &[ChatMessage],
        ctx: &AnalyzeContext,
    ) -> RequestProfile {
        if self.analyzers.is_empty() {
            return RuleBasedAnalyzer::analyze_text(messages);
        }

        if !self.first_definitive {
            // First always wins
            return self.analyzers[0].1.analyze_with_context(messages, ctx);
        }

        let mut last_result = None;
        for (_name, analyzer) in &self.analyzers {
            let result = analyzer.analyze_with_context(messages, ctx);
            if Self::is_definitive(&result) {
                return result;
            }
            last_result = Some(result);
        }

        last_result.unwrap_or_else(|| RuleBasedAnalyzer::analyze_text(messages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: content.into(),
        }
    }

    fn user(content: &str) -> ChatMessage {
        msg("user", content)
    }

    // ── RequestProfile creation & field validation ──

    #[test]
    fn profile_fields_roundtrip() {
        let profile = RequestProfile {
            complexity: 0.7,
            task_type: TaskType::CodeGeneration,
            estimated_tokens: 500,
            requires_reasoning: false,
            requires_vision: false,
            language: Language::English,
            is_multi_turn: false,
            context_window_need: ContextSize::Small,
        };
        assert_eq!(profile.complexity, 0.7);
        assert_eq!(profile.task_type, TaskType::CodeGeneration);
        assert_eq!(profile.estimated_tokens, 500);
        assert!(!profile.requires_reasoning);
        assert!(!profile.requires_vision);
        assert_eq!(profile.language, Language::English);
        assert!(!profile.is_multi_turn);
        assert_eq!(profile.context_window_need, ContextSize::Small);
    }

    #[test]
    fn profile_serialization() {
        let profile = RequestProfile {
            complexity: 0.5,
            task_type: TaskType::SimpleQA,
            estimated_tokens: 100,
            requires_reasoning: false,
            requires_vision: false,
            language: Language::English,
            is_multi_turn: false,
            context_window_need: ContextSize::Small,
        };
        let json = serde_json::to_string(&profile).unwrap();
        let deserialized: RequestProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.task_type, TaskType::SimpleQA);
        assert_eq!(deserialized.complexity, 0.5);
    }

    // ── ContextSize classification ──

    #[test]
    fn context_size_small() {
        // 30 chars → ~10 tokens → Small
        let p = RuleBasedAnalyzer::analyze_text(&[user("hello world, short message")]);
        assert_eq!(p.context_window_need, ContextSize::Small);
    }

    #[test]
    fn context_size_medium() {
        // Need > 4000 tokens → > 12000 chars
        let long_text = "word ".repeat(3000); // 15000 chars → ~5000 tokens
        let p = RuleBasedAnalyzer::analyze_text(&[user(&long_text)]);
        assert_eq!(p.context_window_need, ContextSize::Medium);
    }

    #[test]
    fn context_size_large() {
        // Need > 32000 tokens → > 96000 chars
        let long_text = "word ".repeat(20000); // 100000 chars → ~33333 tokens
        let p = RuleBasedAnalyzer::analyze_text(&[user(&long_text)]);
        assert_eq!(p.context_window_need, ContextSize::Large);
    }

    #[test]
    fn context_size_xlarge() {
        // Need > 128000 tokens → > 384000 chars
        let long_text = "word ".repeat(80000); // 400000 chars → ~133333 tokens
        let p = RuleBasedAnalyzer::analyze_text(&[user(&long_text)]);
        assert_eq!(p.context_window_need, ContextSize::XLarge);
    }

    // ── Language detection ──

    #[test]
    fn detect_english() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("What is the capital of France?")]);
        assert_eq!(p.language, Language::English);
    }

    #[test]
    fn detect_chinese() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("请帮我写一段代码")]);
        assert_eq!(p.language, Language::Chinese);
    }

    #[test]
    fn detect_chinese_mixed() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("Please translate 你好世界 to English")]);
        assert_eq!(p.language, Language::Chinese);
    }

    // ── TaskType detection ──

    #[test]
    fn detect_code_generation() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("Write a function to sort an array")]);
        assert_eq!(p.task_type, TaskType::CodeGeneration);
        assert_eq!(p.complexity, 0.7);
    }

    #[test]
    fn detect_code_via_implement() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("implement a binary search tree")]);
        assert_eq!(p.task_type, TaskType::CodeGeneration);
    }

    #[test]
    fn detect_code_chinese() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("帮我写一段程序")]);
        assert_eq!(p.task_type, TaskType::CodeGeneration);
        assert_eq!(p.language, Language::Chinese);
    }

    #[test]
    fn detect_reasoning() {
        let p =
            RuleBasedAnalyzer::analyze_text(&[user("explain step by step why the sky is blue")]);
        assert_eq!(p.task_type, TaskType::Reasoning);
        assert!(p.requires_reasoning);
        assert_eq!(p.complexity, 0.8);
    }

    #[test]
    fn detect_reasoning_why() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("Why does water boil at 100C?")]);
        assert_eq!(p.task_type, TaskType::Reasoning);
    }

    #[test]
    fn detect_translation() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("translate this to Spanish: hello")]);
        assert_eq!(p.task_type, TaskType::Translation);
        assert_eq!(p.complexity, 0.4);
    }

    #[test]
    fn detect_translation_chinese() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("翻译这段话")]);
        assert_eq!(p.task_type, TaskType::Translation);
    }

    #[test]
    fn detect_summarization() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("summarize this article for me")]);
        assert_eq!(p.task_type, TaskType::Summarization);
    }

    #[test]
    fn detect_creative() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("write a short poem about the moon")]);
        assert_eq!(p.task_type, TaskType::Creative);
        assert_eq!(p.complexity, 0.5);
    }

    #[test]
    fn detect_multimodal() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("describe this image in detail")]);
        assert_eq!(p.task_type, TaskType::MultiModal);
        assert!(p.requires_vision);
        assert_eq!(p.complexity, 0.6);
    }

    #[test]
    fn detect_math() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("calculate the integral of x^2")]);
        assert_eq!(p.task_type, TaskType::Math);
        assert!(p.requires_reasoning);
        assert_eq!(p.complexity, 0.8);
    }

    #[test]
    fn detect_simple_qa() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("What is 2+2?")]);
        assert_eq!(p.task_type, TaskType::SimpleQA);
        assert_eq!(p.complexity, 0.2);
    }

    // ── Token counting heuristic ──

    #[test]
    fn token_estimate_basic() {
        // "hello" = 5 chars → 5/3 = 1 token (max(1))
        let p = RuleBasedAnalyzer::analyze_text(&[user("hello")]);
        assert_eq!(p.estimated_tokens, 1);
    }

    #[test]
    fn token_estimate_longer() {
        // 120 chars → 40 tokens
        let text = "a".repeat(120);
        let p = RuleBasedAnalyzer::analyze_text(&[user(&text)]);
        assert_eq!(p.estimated_tokens, 40);
    }

    #[test]
    fn token_estimate_empty_defaults_to_one() {
        // Empty user message: 0 chars → max(0/3, 1) = 1
        let p = RuleBasedAnalyzer::analyze_text(&[user("")]);
        assert_eq!(p.estimated_tokens, 1);
    }

    // ── Multi-turn detection ──

    #[test]
    fn single_turn() {
        let p = RuleBasedAnalyzer::analyze_text(&[user("hi"), msg("assistant", "hello")]);
        assert!(!p.is_multi_turn);
    }

    #[test]
    fn multi_turn() {
        let msgs = vec![user("hi"), msg("assistant", "hello"), user("how are you?")];
        let p = RuleBasedAnalyzer::analyze_text(&msgs);
        assert!(p.is_multi_turn);
    }

    // ── Edge cases ──

    #[test]
    fn empty_messages() {
        let p = RuleBasedAnalyzer::analyze_text(&[]);
        assert_eq!(p.task_type, TaskType::SimpleQA);
        assert_eq!(p.estimated_tokens, 1);
        assert!(!p.is_multi_turn);
    }

    #[test]
    fn system_messages_ignored_for_task_detection() {
        // Only user messages are scanned for keywords
        let msgs = vec![
            msg("system", "You are helpful. Write code for the user."),
            user("What is the weather today?"),
        ];
        let p = RuleBasedAnalyzer::analyze_text(&msgs);
        // "code" is in system msg but should not trigger CodeGeneration
        assert_eq!(p.task_type, TaskType::SimpleQA);
    }

    #[test]
    fn only_assistant_messages() {
        let msgs = vec![msg("assistant", "I can help you write code")];
        let p = RuleBasedAnalyzer::analyze_text(&msgs);
        assert_eq!(p.task_type, TaskType::SimpleQA);
    }

    // ── RuleBasedAnalyzer trait impl ──

    #[test]
    fn rule_based_analyzer_default() {
        let a = RuleBasedAnalyzer;
        let p = a.analyze(&[user("test")]);
        assert_eq!(p.task_type, TaskType::SimpleQA);
    }

    #[test]
    fn rule_based_analyze_with_context_delegates() {
        let a = RuleBasedAnalyzer::new();
        let ctx = AnalyzeContext::default();
        let p = a.analyze_with_context(&[user("implement a queue")], &ctx);
        assert_eq!(p.task_type, TaskType::CodeGeneration);
    }

    // ── AnalyzeContext ──

    #[test]
    fn analyze_context_default() {
        let ctx = AnalyzeContext::default();
        assert!(ctx.caller_type.is_none());
        assert!(ctx.source.is_none());
        assert!(ctx.tenant_id.is_none());
        assert!(ctx.tags.is_empty());
    }

    // ── AnalyzerChain ──

    /// Analyzer that always returns a specific task type.
    struct FixedAnalyzer(TaskType);
    impl RequestAnalyzer for FixedAnalyzer {
        fn analyze(&self, _messages: &[ChatMessage]) -> RequestProfile {
            RequestProfile {
                complexity: 0.5,
                task_type: self.0,
                estimated_tokens: 100,
                requires_reasoning: false,
                requires_vision: false,
                language: Language::English,
                is_multi_turn: false,
                context_window_need: ContextSize::Small,
            }
        }
    }

    #[test]
    fn chain_first_always_mode() {
        let chain = AnalyzerChain::new(
            vec![
                ("a".into(), Box::new(FixedAnalyzer(TaskType::Creative))),
                ("b".into(), Box::new(FixedAnalyzer(TaskType::Math))),
            ],
            false,
        );
        let p = chain.analyze(&[user("test")]);
        assert_eq!(p.task_type, TaskType::Creative);
    }

    #[test]
    fn chain_first_definitive_skips_simple_qa() {
        let chain = AnalyzerChain::new(
            vec![
                ("a".into(), Box::new(FixedAnalyzer(TaskType::SimpleQA))),
                ("b".into(), Box::new(FixedAnalyzer(TaskType::Math))),
            ],
            true,
        );
        let p = chain.analyze(&[user("test")]);
        assert_eq!(p.task_type, TaskType::Math);
    }

    #[test]
    fn chain_first_definitive_skips_unknown() {
        let chain = AnalyzerChain::new(
            vec![
                ("a".into(), Box::new(FixedAnalyzer(TaskType::Unknown))),
                ("b".into(), Box::new(FixedAnalyzer(TaskType::Translation))),
            ],
            true,
        );
        let p = chain.analyze(&[user("test")]);
        assert_eq!(p.task_type, TaskType::Translation);
    }

    #[test]
    fn chain_first_definitive_uses_first_definitive() {
        let chain = AnalyzerChain::new(
            vec![
                ("a".into(), Box::new(FixedAnalyzer(TaskType::Reasoning))),
                ("b".into(), Box::new(FixedAnalyzer(TaskType::Math))),
            ],
            true,
        );
        let p = chain.analyze(&[user("test")]);
        assert_eq!(p.task_type, TaskType::Reasoning);
    }

    #[test]
    fn chain_empty_falls_back() {
        let chain = AnalyzerChain::new(vec![], true);
        let p = chain.analyze(&[user("implement a function")]);
        assert_eq!(p.task_type, TaskType::CodeGeneration);
    }

    #[test]
    fn chain_all_non_definitive_uses_last() {
        let chain = AnalyzerChain::new(
            vec![
                ("a".into(), Box::new(FixedAnalyzer(TaskType::SimpleQA))),
                ("b".into(), Box::new(FixedAnalyzer(TaskType::Unknown))),
            ],
            true,
        );
        let p = chain.analyze(&[user("test")]);
        // Last result should be used when none are definitive
        assert_eq!(p.task_type, TaskType::Unknown);
    }

    // ── is_definitive ──

    #[test]
    fn is_definitive_check() {
        let definitive = RequestProfile {
            complexity: 0.7,
            task_type: TaskType::CodeGeneration,
            estimated_tokens: 100,
            requires_reasoning: false,
            requires_vision: false,
            language: Language::English,
            is_multi_turn: false,
            context_window_need: ContextSize::Small,
        };
        assert!(AnalyzerChain::is_definitive(&definitive));

        let non_definitive = RequestProfile {
            task_type: TaskType::SimpleQA,
            ..definitive.clone()
        };
        assert!(!AnalyzerChain::is_definitive(&non_definitive));

        let unknown = RequestProfile {
            task_type: TaskType::Unknown,
            ..definitive
        };
        assert!(!AnalyzerChain::is_definitive(&unknown));
    }
}
