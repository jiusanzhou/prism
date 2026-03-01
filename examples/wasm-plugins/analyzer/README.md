# prism-wasm-analyzer

Example WASM analyzer plugin for Prism — keyword-based request classifier.

## What it does

Analyzes incoming LLM requests and classifies them by task type, complexity, and language. This enables Prism's routing engine to select the optimal model (e.g., route simple Q&A to cheaper models, complex code to GPT-4o).

### Classification

| Task Type | Trigger Keywords | Base Complexity |
|---|---|---|
| **CodeGeneration** | code, function, implement, rust, python, 写代码, 编程... | 0.6 |
| **Reasoning** | why, explain, analyze, 为什么, 推理... | 0.7 |
| **Math** | calculate, integral, equation, 数学, 概率... | 0.75 |
| **Creative** | story, poem, essay, 写故事, 创作... | 0.5 |
| **Translation** | translate, 翻译, 英译中... | 0.4 |
| **SimpleQA** | (no keyword match) | 0.2 |

### Output Fields

| Field | Type | Description |
|---|---|---|
| `complexity` | f32 | 0.0–1.0, factoring in task type + text length + multi-turn |
| `task_type` | string | One of the task types above |
| `estimated_tokens` | u32 | Rough input+output token estimate (CJK-aware) |
| `requires_reasoning` | bool | True for Code/Reasoning/Math |
| `requires_vision` | bool | Always false (no vision detection) |
| `language` | string | "Chinese" if CJK detected, else "English" |
| `is_multi_turn` | bool | True if > 2 messages |
| `context_window_need` | string | Small / Medium / Large / XL based on token estimate |

## ABI

This plugin implements the Prism WASM Analyzer ABI:

| Export | Signature | Description |
|---|---|---|
| `memory` | — | Linear memory (automatic with cdylib) |
| `alloc` | `(size: i32) -> i32` | Allocate bytes, returns pointer |
| `dealloc` | `(ptr: i32, size: i32)` | No-op (bump allocator) |
| `analyze` | `(ptr: i32, len: i32) -> i32` | Analyze JSON input, returns pointer to length-prefixed JSON |

### Input Format

```json
{
  "messages": [
    {"role": "user", "content": "Write a quicksort in Rust"}
  ],
  "context": {
    "caller_type": "user",
    "source": "api",
    "tenant_id": "team-a"
  }
}
```

### Output Format

4-byte little-endian length prefix + JSON body:

```
[u32 LE length][JSON bytes...]
```

```json
{
  "complexity": 0.6,
  "task_type": "CodeGeneration",
  "estimated_tokens": 450,
  "requires_reasoning": true,
  "requires_vision": false,
  "language": "English",
  "is_multi_turn": false,
  "context_window_need": "Small"
}
```

## Build

```bash
# Prerequisites
rustup target add wasm32-unknown-unknown

# Build
cargo build --target wasm32-unknown-unknown --release
```

Output: `target/wasm32-unknown-unknown/release/prism_wasm_analyzer.wasm`

## Testing

The pre-built `.wasm` fixture is at `prism-core/tests/fixtures/prism_wasm_analyzer.wasm`. Integration tests live in `prism-core/tests/wasm_analyzer_tests.rs`:

```bash
# From workspace root
cargo test --features wasm -p prism-core -- wasm_analyzer
```

Tests cover: loading from file/bytes, task classification (6 types), Chinese detection, multi-turn, complexity ordering, edge cases (empty/long input), and context passthrough.
