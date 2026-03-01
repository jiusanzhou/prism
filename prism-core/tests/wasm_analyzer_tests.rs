//! Integration tests for WasmAnalyzer using a real WASM module.
//!
//! These tests require `--features wasm` to compile.
//! The test fixture `fixtures/prism_wasm_analyzer.wasm` is built from
//! `examples/wasm-plugins/analyzer/` (a simple keyword-based classifier).

#![cfg(feature = "wasm")]

use prism_core::analyzer::{
    AnalyzeContext, ChatMessage, RuleBasedAnalyzer, RequestAnalyzer, WasmAnalyzer,
};
use prism_core::types::TaskType;
use std::collections::HashMap;
use std::path::PathBuf;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("prism_wasm_analyzer.wasm")
}

fn make_wasm_analyzer() -> WasmAnalyzer {
    let fallback = Box::new(RuleBasedAnalyzer);
    WasmAnalyzer::from_file(&fixture_path(), fallback)
        .expect("should load wasm module")
}

fn msg(role: &str, content: &str) -> ChatMessage {
    ChatMessage {
        role: role.to_string(),
        content: content.to_string(),
    }
}

// ──────────────────────────────────────────────
// Basic loading tests
// ──────────────────────────────────────────────

#[test]
fn test_wasm_load_from_file() {
    let analyzer = make_wasm_analyzer();
    // Should be able to analyze without panicking
    let profile = analyzer.analyze(&[msg("user", "hello")]);
    assert!(profile.complexity >= 0.0 && profile.complexity <= 1.0);
}

#[test]
fn test_wasm_load_from_bytes() {
    let bytes = std::fs::read(fixture_path()).expect("should read wasm file");
    let fallback = Box::new(RuleBasedAnalyzer);
    let analyzer = WasmAnalyzer::from_bytes(&bytes, fallback)
        .expect("should load wasm from bytes");
    let profile = analyzer.analyze(&[msg("user", "hello")]);
    assert!(profile.complexity >= 0.0);
}

// ──────────────────────────────────────────────
// Task classification tests
// ──────────────────────────────────────────────

#[test]
fn test_wasm_classifies_code() {
    let analyzer = make_wasm_analyzer();
    let profile = analyzer.analyze(&[
        msg("user", "Please write a Rust function to sort a vector"),
    ]);
    assert_eq!(profile.task_type, TaskType::CodeGeneration);
    assert!(profile.complexity >= 0.5);
    assert!(profile.requires_reasoning);
}

#[test]
fn test_wasm_classifies_reasoning() {
    let analyzer = make_wasm_analyzer();
    let profile = analyzer.analyze(&[
        msg("user", "Explain why the sky is blue and analyze the physics"),
    ]);
    assert_eq!(profile.task_type, TaskType::Reasoning);
    assert!(profile.requires_reasoning);
}

#[test]
fn test_wasm_classifies_simple_qa() {
    let analyzer = make_wasm_analyzer();
    let profile = analyzer.analyze(&[msg("user", "hello")]);
    assert_eq!(profile.task_type, TaskType::SimpleQA);
    assert!(profile.complexity <= 0.3);
    assert!(!profile.requires_reasoning);
}

#[test]
fn test_wasm_classifies_creative_writing() {
    let analyzer = make_wasm_analyzer();
    let profile = analyzer.analyze(&[
        msg("user", "Write a short story about a dragon"),
    ]);
    assert_eq!(profile.task_type, TaskType::Creative);
}

#[test]
fn test_wasm_classifies_translation() {
    let analyzer = make_wasm_analyzer();
    let profile = analyzer.analyze(&[
        msg("user", "Translate this text to English: 你好世界"),
    ]);
    assert_eq!(profile.task_type, TaskType::Translation);
}

#[test]
fn test_wasm_classifies_math() {
    let analyzer = make_wasm_analyzer();
    let profile = analyzer.analyze(&[
        msg("user", "Calculate the integral of x^2 from 0 to 1"),
    ]);
    assert_eq!(profile.task_type, TaskType::Math);
    assert!(profile.requires_reasoning);
}

// ──────────────────────────────────────────────
// Chinese language detection
// ──────────────────────────────────────────────

#[test]
fn test_wasm_detects_chinese() {
    let analyzer = make_wasm_analyzer();
    let profile = analyzer.analyze(&[
        msg("user", "请帮我写一段代码实现快速排序"),
    ]);
    assert_eq!(profile.task_type, TaskType::CodeGeneration);
    // Token estimation should account for CJK
    assert!(profile.estimated_tokens > 0);
}

// ──────────────────────────────────────────────
// Multi-turn detection
// ──────────────────────────────────────────────

#[test]
fn test_wasm_multi_turn() {
    let analyzer = make_wasm_analyzer();
    let profile = analyzer.analyze(&[
        msg("user", "What is Rust?"),
        msg("assistant", "Rust is a systems programming language."),
        msg("user", "Show me some example code"),
    ]);
    assert!(profile.is_multi_turn);
}

#[test]
fn test_wasm_single_turn() {
    let analyzer = make_wasm_analyzer();
    let profile = analyzer.analyze(&[msg("user", "hello")]);
    assert!(!profile.is_multi_turn);
}

// ──────────────────────────────────────────────
// Analyze with context
// ──────────────────────────────────────────────

#[test]
fn test_wasm_analyze_with_context() {
    let analyzer = make_wasm_analyzer();
    let ctx = AnalyzeContext {
        caller_type: Some("user".to_string()),
        source: Some("mobile".to_string()),
        tenant_id: Some("team-a".to_string()),
        tags: HashMap::new(),
    };
    let profile = analyzer.analyze_with_context(
        &[msg("user", "Write a Python function to parse JSON")],
        &ctx,
    );
    assert_eq!(profile.task_type, TaskType::CodeGeneration);
}

// ──────────────────────────────────────────────
// Edge cases
// ──────────────────────────────────────────────

#[test]
fn test_wasm_empty_messages() {
    let analyzer = make_wasm_analyzer();
    let profile = analyzer.analyze(&[]);
    // Should not panic, return a default
    assert_eq!(profile.task_type, TaskType::SimpleQA);
}

#[test]
fn test_wasm_very_long_input() {
    let analyzer = make_wasm_analyzer();
    let long_text = "explain this code ".repeat(500);
    let profile = analyzer.analyze(&[msg("user", &long_text)]);
    // Should handle without crashing
    assert!(profile.complexity > 0.0);
    assert!(profile.estimated_tokens > 0);
}

// ──────────────────────────────────────────────
// Complexity scoring
// ──────────────────────────────────────────────

#[test]
fn test_wasm_complexity_ordering() {
    let analyzer = make_wasm_analyzer();

    let simple = analyzer.analyze(&[msg("user", "hi")]);
    let code = analyzer.analyze(&[
        msg("user", "Write a Rust function implementing quicksort"),
    ]);
    let math = analyzer.analyze(&[
        msg("user", "Calculate the derivative of sin(x^2) and prove it"),
    ]);

    // Math/code should be more complex than simple
    assert!(code.complexity > simple.complexity);
    assert!(math.complexity > simple.complexity);
}
