use std::collections::HashMap;

use prism_core::analyzer::*;
use prism_core::types::TaskType;

fn msg(content: &str) -> Vec<ChatMessage> {
    vec![ChatMessage {
        role: "user".into(),
        content: content.into(),
    }]
}

fn make_config() -> AnalyzerConfig {
    AnalyzerConfig {
        default_rules: vec![
            AnalyzerRule {
                keywords: vec!["SQL".into(), "database".into(), "数据库".into()],
                patterns: vec![],
                task_type: TaskType::CodeGeneration,
                complexity: 0.6,
                require_reasoning: false,
                require_vision: false,
                priority: 0,
            },
            AnalyzerRule {
                keywords: vec!["legal".into(), "contract".into(), "法律".into()],
                patterns: vec![],
                task_type: TaskType::Reasoning,
                complexity: 0.9,
                require_reasoning: true,
                require_vision: false,
                priority: 0,
            },
        ],
        scopes: vec![
            RuleScope {
                name: "mobile_users".into(),
                match_on: RuleScopeMatch {
                    caller_types: None,
                    sources: Some(vec!["mobile".into(), "ios".into(), "android".into()]),
                    tenant_ids: None,
                    tags: None,
                },
                rules: vec![AnalyzerRule {
                    keywords: vec!["help".into(), "帮助".into()],
                    patterns: vec![],
                    task_type: TaskType::SimpleQA,
                    complexity: 0.1,
                    require_reasoning: false,
                    require_vision: false,
                    priority: 0,
                }],
            },
            RuleScope {
                name: "internal_services".into(),
                match_on: RuleScopeMatch {
                    caller_types: Some(vec!["service".into()]),
                    sources: None,
                    tenant_ids: None,
                    tags: Some({
                        let mut m = HashMap::new();
                        m.insert("env".into(), "prod".into());
                        m
                    }),
                },
                rules: vec![AnalyzerRule {
                    keywords: vec!["analyze".into(), "分析".into()],
                    patterns: vec![],
                    task_type: TaskType::Reasoning,
                    complexity: 0.95,
                    require_reasoning: true,
                    require_vision: false,
                    priority: 0,
                }],
            },
            RuleScope {
                name: "tenant_acme".into(),
                match_on: RuleScopeMatch {
                    caller_types: None,
                    sources: None,
                    tenant_ids: Some(vec!["acme-corp".into()]),
                    tags: None,
                },
                rules: vec![AnalyzerRule {
                    keywords: vec!["report".into(), "报告".into()],
                    patterns: vec![],
                    task_type: TaskType::Summarization,
                    complexity: 0.5,
                    require_reasoning: false,
                    require_vision: false,
                    priority: 0,
                }],
            },
        ],
    }
}

// ──────────────────────────────────────────────
// ConfigurableRuleAnalyzer tests
// ──────────────────────────────────────────────

#[test]
fn test_custom_keyword_rule() {
    let analyzer = ConfigurableRuleAnalyzer::new(make_config());
    let profile = analyzer.analyze(&msg("Write a SQL query to get users"));
    assert_eq!(profile.task_type, TaskType::CodeGeneration);
    assert!((profile.complexity - 0.6).abs() < 0.01);
}

#[test]
fn test_custom_keyword_chinese() {
    let analyzer = ConfigurableRuleAnalyzer::new(make_config());
    let profile = analyzer.analyze(&msg("帮我写一个数据库查询"));
    assert_eq!(profile.task_type, TaskType::CodeGeneration);
}

#[test]
fn test_scope_by_source() {
    let analyzer = ConfigurableRuleAnalyzer::new(make_config());
    let ctx = AnalyzeContext {
        source: Some("mobile".into()),
        ..Default::default()
    };
    let profile = analyzer.analyze_with_context(&msg("I need help"), &ctx);
    assert_eq!(profile.task_type, TaskType::SimpleQA);
    assert!((profile.complexity - 0.1).abs() < 0.01);
}

#[test]
fn test_scope_by_caller_type_and_tags() {
    let analyzer = ConfigurableRuleAnalyzer::new(make_config());
    let ctx = AnalyzeContext {
        caller_type: Some("service".into()),
        tags: {
            let mut m = HashMap::new();
            m.insert("env".into(), "prod".into());
            m
        },
        ..Default::default()
    };
    let profile = analyzer.analyze_with_context(&msg("Please analyze this data"), &ctx);
    assert_eq!(profile.task_type, TaskType::Reasoning);
    assert!((profile.complexity - 0.95).abs() < 0.01);
    assert!(profile.requires_reasoning);
}

#[test]
fn test_scope_by_tenant() {
    let analyzer = ConfigurableRuleAnalyzer::new(make_config());
    let ctx = AnalyzeContext {
        tenant_id: Some("acme-corp".into()),
        ..Default::default()
    };
    let profile = analyzer.analyze_with_context(&msg("Generate a report for Q4"), &ctx);
    assert_eq!(profile.task_type, TaskType::Summarization);
    assert!((profile.complexity - 0.5).abs() < 0.01);
}

#[test]
fn test_scope_mismatch_falls_to_default() {
    let analyzer = ConfigurableRuleAnalyzer::new(make_config());
    // Context doesn't match any scope, but text matches default rule "legal"
    let ctx = AnalyzeContext {
        source: Some("unknown_platform".into()),
        ..Default::default()
    };
    let profile = analyzer.analyze_with_context(&msg("Review this legal contract"), &ctx);
    assert_eq!(profile.task_type, TaskType::Reasoning);
    assert!((profile.complexity - 0.9).abs() < 0.01);
}

#[test]
fn test_no_match_falls_back_to_builtin() {
    let analyzer = ConfigurableRuleAnalyzer::new(make_config());
    // "Hello" doesn't match any custom rule → falls back to RuleBasedAnalyzer → SimpleQA
    let profile = analyzer.analyze(&msg("Hello there"));
    assert_eq!(profile.task_type, TaskType::SimpleQA);
}

#[test]
fn test_regex_pattern_rule() {
    let config = AnalyzerConfig {
        default_rules: vec![AnalyzerRule {
            keywords: vec![],
            patterns: vec![r"(?i)SELECT\s+.+\s+FROM".into()],
            task_type: TaskType::CodeGeneration,
            complexity: 0.7,
            require_reasoning: false,
            require_vision: false,
            priority: 0,
        }],
        scopes: vec![],
    };
    let analyzer = ConfigurableRuleAnalyzer::new(config);
    let profile = analyzer.analyze(&msg("SELECT id, name FROM users WHERE active = true"));
    assert_eq!(profile.task_type, TaskType::CodeGeneration);
}

#[test]
fn test_rule_priority() {
    let config = AnalyzerConfig {
        default_rules: vec![
            AnalyzerRule {
                keywords: vec!["code".into()],
                patterns: vec![],
                task_type: TaskType::CodeGeneration,
                complexity: 0.7,
                require_reasoning: false,
                require_vision: false,
                priority: 1, // lower priority
            },
            AnalyzerRule {
                keywords: vec!["code".into()],
                patterns: vec![],
                task_type: TaskType::Reasoning,
                complexity: 0.9,
                require_reasoning: true,
                require_vision: false,
                priority: 10, // higher priority — should win
            },
        ],
        scopes: vec![],
    };
    let analyzer = ConfigurableRuleAnalyzer::new(config);
    let profile = analyzer.analyze(&msg("Review this code"));
    assert_eq!(profile.task_type, TaskType::Reasoning);
    assert!(profile.requires_reasoning);
}

#[test]
fn test_scope_tag_matching_partial_fail() {
    let analyzer = ConfigurableRuleAnalyzer::new(make_config());
    // caller_type matches but tag doesn't → scope should NOT match
    let ctx = AnalyzeContext {
        caller_type: Some("service".into()),
        tags: {
            let mut m = HashMap::new();
            m.insert("env".into(), "staging".into()); // wrong value
            m
        },
        ..Default::default()
    };
    // Should fall through to default rules, not internal_services scope
    let profile = analyzer.analyze_with_context(&msg("analyze this"), &ctx);
    // "分析" is in default rules too? No. "analyze" is only in the service scope.
    // Falls to builtin RuleBasedAnalyzer which matches "分析" but not "analyze" alone.
    // Actually RuleBasedAnalyzer checks "reason" keyword → not matched.
    // So this should be SimpleQA from builtin.
    // Wait — let's check: the builtin has no "analyze" keyword.
    // The default_rules don't have "analyze" either.
    // So it falls back to builtin → SimpleQA.
    assert_eq!(profile.task_type, TaskType::SimpleQA);
}

// ──────────────────────────────────────────────
// AnalyzerChain tests
// ──────────────────────────────────────────────

/// A dummy analyzer that always returns a fixed task type.
struct FixedAnalyzer(TaskType);

impl RequestAnalyzer for FixedAnalyzer {
    fn analyze(&self, _messages: &[ChatMessage]) -> RequestProfile {
        RequestProfile {
            complexity: 0.5,
            task_type: self.0,
            estimated_tokens: 10,
            requires_reasoning: false,
            requires_vision: false,
            language: Language::English,
            is_multi_turn: false,
            context_window_need: ContextSize::Small,
        }
    }
}

#[test]
fn test_chain_first_definitive_wins() {
    let chain = AnalyzerChain::new(
        vec![
            ("simple".into(), Box::new(FixedAnalyzer(TaskType::SimpleQA))),
            (
                "code".into(),
                Box::new(FixedAnalyzer(TaskType::CodeGeneration)),
            ),
            (
                "reason".into(),
                Box::new(FixedAnalyzer(TaskType::Reasoning)),
            ),
        ],
        true, // first_definitive
    );
    let profile = chain.analyze(&msg("anything"));
    // SimpleQA is not definitive, CodeGeneration is → wins
    assert_eq!(profile.task_type, TaskType::CodeGeneration);
}

#[test]
fn test_chain_all_indefinitive_returns_last() {
    let chain = AnalyzerChain::new(
        vec![
            ("a".into(), Box::new(FixedAnalyzer(TaskType::SimpleQA))),
            ("b".into(), Box::new(FixedAnalyzer(TaskType::Unknown))),
            ("c".into(), Box::new(FixedAnalyzer(TaskType::SimpleQA))),
        ],
        true,
    );
    let profile = chain.analyze(&msg("anything"));
    // All indefinitive → last result (SimpleQA from "c")
    assert_eq!(profile.task_type, TaskType::SimpleQA);
}

#[test]
fn test_chain_first_always_wins_when_not_definitive_mode() {
    let chain = AnalyzerChain::new(
        vec![
            ("first".into(), Box::new(FixedAnalyzer(TaskType::SimpleQA))),
            (
                "second".into(),
                Box::new(FixedAnalyzer(TaskType::CodeGeneration)),
            ),
        ],
        false, // first always wins
    );
    let profile = chain.analyze(&msg("anything"));
    assert_eq!(profile.task_type, TaskType::SimpleQA);
}

// ──────────────────────────────────────────────
// WebhookAnalyzer tests (using ureq mock isn't easy,
// so we test the fallback path)
// ──────────────────────────────────────────────

#[test]
fn test_webhook_unreachable_falls_back() {
    let fallback = Box::new(FixedAnalyzer(TaskType::Creative));
    let analyzer = WebhookAnalyzer::new(
        "http://127.0.0.1:1/nonexistent".into(),
        std::time::Duration::from_millis(100),
        fallback,
    );
    // Should fall back because the URL is unreachable
    let profile = analyzer.analyze(&msg("Hello"));
    assert_eq!(profile.task_type, TaskType::Creative);
}
