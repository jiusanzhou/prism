//! A minimal WASM analyzer module for Prism.
//!
//! This module implements the Prism WASM analyzer contract:
//! - Export `memory`    (automatic with cdylib)
//! - Export `alloc(size) -> ptr`
//! - Export `dealloc(ptr, size)` (no-op for bump allocator)
//! - Export `analyze(ptr, len) -> result_ptr`
//!
//! Input JSON: `{ "messages": [{"role":"...", "content":"..."}], "context": {...} }`
//! Output: length-prefixed (4-byte LE u32) JSON matching `RequestProfile`.
//!
//! This analyzer implements a simple keyword-based classifier as a demonstration.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;

// ── Simple bump allocator ──

thread_local! {
    static HEAP: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(65536));
}

#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    HEAP.with(|heap| {
        let mut heap = heap.borrow_mut();
        let ptr = heap.len();
        heap.resize(ptr + size as usize, 0);
        ptr as i32
    })
}

#[no_mangle]
pub extern "C" fn dealloc(_ptr: i32, _size: i32) {
    // bump allocator — no-op
}

// ── Types (mirrors prism-core analyzer types) ──

#[derive(Deserialize)]
struct AnalyzeInput {
    messages: Vec<ChatMessage>,
    #[serde(default)]
    context: AnalyzeContext,
}

#[derive(Deserialize)]
struct ChatMessage {
    #[allow(dead_code)]
    role: String,
    content: String,
}

#[derive(Deserialize, Default)]
struct AnalyzeContext {
    #[allow(dead_code)]
    caller_type: Option<String>,
    #[allow(dead_code)]
    source: Option<String>,
    #[allow(dead_code)]
    tenant_id: Option<String>,
}

#[derive(Serialize)]
struct RequestProfile {
    complexity: f32,
    task_type: String,
    estimated_tokens: u32,
    requires_reasoning: bool,
    requires_vision: bool,
    language: String,
    is_multi_turn: bool,
    context_window_need: String,
}

// ── Keyword sets ──

const CODE_KEYWORDS: &[&str] = &[
    "code", "function", "implement", "bug", "error", "compile",
    "rust", "python", "javascript", "golang", "java", "debug",
    "api", "class", "struct", "enum", "trait", "interface",
    "algorithm", "leetcode", "refactor", "def ", "fn ", "func ",
    "print(", "console.log", "import ", "package ", "代码", "编程",
    "函数", "报错", "编译", "排序", "数组", "链表", "递归",
    "写代码", "写一段代码", "写一个",
];

const REASONING_KEYWORDS: &[&str] = &[
    "why", "explain", "analyze", "compare", "evaluate", "reason",
    "think", "consider", "因为", "为什么", "解释", "分析",
    "推理", "逻辑", "证明", "论证",
];

const CREATIVE_KEYWORDS: &[&str] = &[
    "write a story", "write a poem", "write an essay",
    "story", "poem", "essay", "creative", "imagine",
    "fiction", "narrative", "compose", "draft",
    "写故事", "写诗", "写文章", "写小说", "小说", "创作",
];

const TRANSLATION_KEYWORDS: &[&str] = &[
    "translate", "translation", "翻译", "英译中", "中译英",
    "in english", "in chinese", "in japanese",
];

const MATH_KEYWORDS: &[&str] = &[
    "calculate", "math", "equation", "integral", "derivative",
    "probability", "statistics", "数学", "计算", "方程",
    "概率", "微积分", "求解",
];

// ── Analysis logic ──

fn classify(input: &AnalyzeInput) -> RequestProfile {
    // Combine all message content
    let text: String = input
        .messages
        .iter()
        .map(|m| m.content.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");

    let text_len = text.len();
    let is_multi_turn = input.messages.len() > 2;

    // Count keyword matches
    let code_score = count_matches(&text, CODE_KEYWORDS);
    let reasoning_score = count_matches(&text, REASONING_KEYWORDS);
    let creative_score = count_matches(&text, CREATIVE_KEYWORDS);
    let translation_score = count_matches(&text, TRANSLATION_KEYWORDS);
    let math_score = count_matches(&text, MATH_KEYWORDS);

    // Determine task type by highest score
    let scores = [
        (code_score, "CodeGeneration"),
        (reasoning_score, "Reasoning"),
        (creative_score, "Creative"),
        (translation_score, "Translation"),
        (math_score, "Math"),
    ];

    let (best_score, task_type) = scores
        .iter()
        .max_by_key(|(s, _)| *s)
        .map(|(s, t)| (*s, *t))
        .unwrap_or((0, "SimpleQA"));

    let task_type = if best_score == 0 { "SimpleQA" } else { task_type };

    // Compute complexity based on task type and text length
    let base_complexity: f32 = match task_type {
        "CodeGeneration" => 0.6,
        "Reasoning" => 0.7,
        "Math" => 0.75,
        "Creative" => 0.5,
        "Translation" => 0.4,
        _ => 0.2,
    };

    // Length factor: longer content → higher complexity
    let length_factor: f32 = if text_len > 2000 {
        0.2
    } else if text_len > 500 {
        0.1
    } else {
        0.0
    };

    // Multi-turn bonus
    let multi_turn_bonus: f32 = if is_multi_turn { 0.05 } else { 0.0 };

    let complexity = (base_complexity + length_factor + multi_turn_bonus).min(1.0);

    // Estimate tokens (rough: 1 token ≈ 4 chars for English, 2 chars for CJK)
    let has_cjk = text.chars().any(|c| c >= '\u{4e00}' && c <= '\u{9fff}');
    let char_per_token: usize = if has_cjk { 2 } else { 4 };
    let input_tokens = text_len / char_per_token;
    let output_estimate = match task_type {
        "CodeGeneration" => input_tokens * 3,
        "CreativeWriting" => input_tokens * 4,
        "Translation" => input_tokens * 2,
        _ => input_tokens * 2,
    };
    let estimated_tokens = (input_tokens + output_estimate).min(100_000) as u32;

    // Requires reasoning?
    let requires_reasoning = match task_type {
        "Reasoning" | "Math" | "CodeGeneration" => true,
        _ => false,
    };

    // Context window need
    let context_window_need = if estimated_tokens > 32_000 {
        "XL"
    } else if estimated_tokens > 8_000 {
        "Large"
    } else if estimated_tokens > 2_000 {
        "Medium"
    } else {
        "Small"
    };

    // Detect language
    let language = if has_cjk { "Chinese" } else { "English" };

    RequestProfile {
        complexity,
        task_type: task_type.to_string(),
        estimated_tokens,
        requires_reasoning,
        requires_vision: false,
        language: language.to_string(),
        is_multi_turn,
        context_window_need: context_window_need.to_string(),
    }
}

fn count_matches(text: &str, keywords: &[&str]) -> usize {
    keywords.iter().filter(|kw| text.contains(**kw)).count()
}

// ── WASM entry point ──

/// Analyze input at `ptr`/`len`, return pointer to length-prefixed JSON result.
///
/// Result layout: [len: u32 LE] [json_bytes...]
#[no_mangle]
pub extern "C" fn analyze(ptr: i32, len: i32) -> i32 {
    let input_bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) };

    let result = match serde_json::from_slice::<AnalyzeInput>(input_bytes) {
        Ok(input) => classify(&input),
        Err(_) => {
            // Fallback: return a default SimpleQA profile
            RequestProfile {
                complexity: 0.2,
                task_type: "SimpleQA".to_string(),
                estimated_tokens: 100,
                requires_reasoning: false,
                requires_vision: false,
                language: "English".to_string(),
                is_multi_turn: false,
                context_window_need: "Small".to_string(),
            }
        }
    };

    let json = serde_json::to_vec(&result).unwrap_or_default();
    let total_len = 4 + json.len();

    // Allocate output buffer
    let out_ptr = alloc(total_len as i32);

    unsafe {
        let out = std::slice::from_raw_parts_mut(out_ptr as *mut u8, total_len);
        // Write length prefix (LE u32)
        let len_bytes = (json.len() as u32).to_le_bytes();
        out[0..4].copy_from_slice(&len_bytes);
        // Write JSON
        out[4..4 + json.len()].copy_from_slice(&json);
    }

    out_ptr
}
