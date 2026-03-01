//! Mock LLM API Server
//!
//! Simulates OpenAI-compatible API endpoints for testing and benchmarking Prism.
//!
//! Supports:
//! - POST /v1/chat/completions (streaming + non-streaming)
//! - POST /v1/completions
//! - POST /v1/embeddings
//! - GET  /v1/models
//! - GET  /health

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::time::Duration;
use tracing::{info, warn};

// ════════════════════════════════════════════════
// CLI
// ════════════════════════════════════════════════

/// Mock LLM API Server — simulates OpenAI-compatible endpoints.
#[derive(Parser, Debug, Clone)]
#[command(name = "mock-llm-server", version, about)]
struct Cli {
    /// Port to listen on
    #[arg(short, long, default_value_t = 18080)]
    port: u16,

    /// Simulated response delay in milliseconds
    #[arg(long, default_value_t = 0)]
    delay_ms: u64,

    /// Error rate (0.0 to 1.0) — probability of returning 500
    #[arg(long, default_value_t = 0.0)]
    error_rate: f64,

    /// Listen address
    #[arg(long, default_value = "0.0.0.0")]
    host: String,
}

// ════════════════════════════════════════════════
// Shared state
// ════════════════════════════════════════════════

#[derive(Clone)]
struct AppConfig {
    delay: Duration,
    error_rate: f64,
}

// ════════════════════════════════════════════════
// Request / Response types
// ════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
struct ChatCompletionRequest {
    model: Option<String>,
    #[serde(default)]
    messages: Vec<ChatMessage>,
    #[serde(default)]
    stream: bool,
    #[allow(dead_code)]
    #[serde(default)]
    temperature: Option<f64>,
    #[allow(dead_code)]
    #[serde(default)]
    max_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct CompletionRequest {
    model: Option<String>,
    #[allow(dead_code)]
    prompt: Option<String>,
    #[serde(default)]
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct EmbeddingRequest {
    model: Option<String>,
    #[allow(dead_code)]
    input: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatCompletionResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<ChatChoice>,
    usage: Usage,
}

#[derive(Debug, Serialize)]
struct ChatChoice {
    index: u32,
    message: ChatMessage,
    finish_reason: String,
}

#[derive(Debug, Serialize)]
struct ChatCompletionChunk {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<ChatChunkChoice>,
}

#[derive(Debug, Serialize)]
struct ChatChunkChoice {
    index: u32,
    delta: serde_json::Value,
    finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct CompletionResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<CompletionChoice>,
    usage: Usage,
}

#[derive(Debug, Serialize)]
struct CompletionChoice {
    index: u32,
    text: String,
    finish_reason: String,
}

#[derive(Debug, Serialize)]
struct EmbeddingResponse {
    object: String,
    data: Vec<EmbeddingData>,
    model: String,
    usage: EmbeddingUsage,
}

#[derive(Debug, Serialize)]
struct EmbeddingData {
    object: String,
    embedding: Vec<f64>,
    index: u32,
}

#[derive(Debug, Serialize)]
struct EmbeddingUsage {
    prompt_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Serialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Serialize)]
struct ModelsResponse {
    object: String,
    data: Vec<ModelInfo>,
}

#[derive(Debug, Serialize)]
struct ModelInfo {
    id: String,
    object: String,
    created: u64,
    owned_by: String,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: String,
    server: String,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    message: String,
    r#type: String,
    code: String,
}

// ════════════════════════════════════════════════
// Helpers
// ════════════════════════════════════════════════

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn mock_id(prefix: &str) -> String {
    format!("{}-mock-{}", prefix, uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("0"))
}

fn should_error(error_rate: f64) -> bool {
    if error_rate <= 0.0 {
        return false;
    }
    rand::thread_rng().gen::<f64>() < error_rate
}

fn error_500() -> impl IntoResponse {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorBody {
            error: ErrorDetail {
                message: "Mock server simulated error".to_string(),
                r#type: "server_error".to_string(),
                code: "internal_error".to_string(),
            },
        }),
    )
}

/// The mock response content, split into tokens for streaming.
const MOCK_RESPONSE_TOKENS: &[&str] = &[
    "Hello", "!", " I", " am", " a", " mock", " LLM", " response", ".",
];

// ════════════════════════════════════════════════
// Handlers
// ════════════════════════════════════════════════

/// POST /v1/chat/completions
async fn chat_completions(
    State(config): State<Arc<AppConfig>>,
    _headers: HeaderMap,
    Json(body): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    // Simulate delay
    if config.delay > Duration::ZERO {
        tokio::time::sleep(config.delay).await;
    }

    // Simulate errors
    if should_error(config.error_rate) {
        return error_500().into_response();
    }

    let model = body.model.unwrap_or_else(|| "unknown".to_string());
    let id = mock_id("chatcmpl");
    let created = now_epoch();

    if body.stream {
        return stream_chat_response(id, model, created, config.delay).into_response();
    }

    let content = format!(
        "Hello! I am a mock LLM response. Model: {}, messages: {}",
        model,
        body.messages.len()
    );

    let response = ChatCompletionResponse {
        id,
        object: "chat.completion".to_string(),
        created,
        model,
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content,
            },
            finish_reason: "stop".to_string(),
        }],
        usage: Usage {
            prompt_tokens: 10,
            completion_tokens: 8,
            total_tokens: 18,
        },
    };

    Json(response).into_response()
}

/// SSE streaming for chat completions
fn stream_chat_response(
    id: String,
    model: String,
    created: u64,
    token_delay: Duration,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        // First chunk: role
        let chunk = ChatCompletionChunk {
            id: id.clone(),
            object: "chat.completion.chunk".to_string(),
            created,
            model: model.clone(),
            choices: vec![ChatChunkChoice {
                index: 0,
                delta: serde_json::json!({"role": "assistant"}),
                finish_reason: None,
            }],
        };
        yield Ok(Event::default().data(serde_json::to_string(&chunk).unwrap()));

        // Content chunks — one per token
        for token in MOCK_RESPONSE_TOKENS {
            if token_delay > Duration::ZERO {
                tokio::time::sleep(token_delay / 2).await;
            }
            let chunk = ChatCompletionChunk {
                id: id.clone(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.clone(),
                choices: vec![ChatChunkChoice {
                    index: 0,
                    delta: serde_json::json!({"content": token}),
                    finish_reason: None,
                }],
            };
            yield Ok(Event::default().data(serde_json::to_string(&chunk).unwrap()));
        }

        // Final chunk: finish_reason
        let chunk = ChatCompletionChunk {
            id: id.clone(),
            object: "chat.completion.chunk".to_string(),
            created,
            model: model.clone(),
            choices: vec![ChatChunkChoice {
                index: 0,
                delta: serde_json::json!({}),
                finish_reason: Some("stop".to_string()),
            }],
        };
        yield Ok(Event::default().data(serde_json::to_string(&chunk).unwrap()));

        // [DONE]
        yield Ok(Event::default().data("[DONE]"));
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// POST /v1/completions
async fn completions(
    State(config): State<Arc<AppConfig>>,
    Json(body): Json<CompletionRequest>,
) -> impl IntoResponse {
    if config.delay > Duration::ZERO {
        tokio::time::sleep(config.delay).await;
    }

    if should_error(config.error_rate) {
        return error_500().into_response();
    }

    let model = body.model.unwrap_or_else(|| "unknown".to_string());

    if body.stream {
        // For simplicity, return non-streaming for completions even when stream=true
        // (the full streaming implementation is in chat completions)
        warn!("Streaming not fully implemented for /v1/completions, returning non-streaming");
    }

    let response = CompletionResponse {
        id: mock_id("cmpl"),
        object: "text_completion".to_string(),
        created: now_epoch(),
        model,
        choices: vec![CompletionChoice {
            index: 0,
            text: "Hello! I am a mock LLM response.".to_string(),
            finish_reason: "stop".to_string(),
        }],
        usage: Usage {
            prompt_tokens: 5,
            completion_tokens: 8,
            total_tokens: 13,
        },
    };

    Json(response).into_response()
}

/// POST /v1/embeddings
async fn embeddings(
    State(config): State<Arc<AppConfig>>,
    Json(body): Json<EmbeddingRequest>,
) -> impl IntoResponse {
    if config.delay > Duration::ZERO {
        tokio::time::sleep(config.delay).await;
    }

    if should_error(config.error_rate) {
        return error_500().into_response();
    }

    let model = body.model.unwrap_or_else(|| "text-embedding-ada-002".to_string());

    // Return a fixed 1536-dim embedding (all zeros for simplicity)
    let embedding: Vec<f64> = vec![0.0; 1536];

    let response = EmbeddingResponse {
        object: "list".to_string(),
        data: vec![EmbeddingData {
            object: "embedding".to_string(),
            embedding,
            index: 0,
        }],
        model,
        usage: EmbeddingUsage {
            prompt_tokens: 5,
            total_tokens: 5,
        },
    };

    Json(response).into_response()
}

/// GET /v1/models
async fn models() -> Json<ModelsResponse> {
    let created = now_epoch();
    Json(ModelsResponse {
        object: "list".to_string(),
        data: vec![
            ModelInfo {
                id: "gpt-4o".to_string(),
                object: "model".to_string(),
                created,
                owned_by: "mock".to_string(),
            },
            ModelInfo {
                id: "gpt-4o-mini".to_string(),
                object: "model".to_string(),
                created,
                owned_by: "mock".to_string(),
            },
            ModelInfo {
                id: "gpt-3.5-turbo".to_string(),
                object: "model".to_string(),
                created,
                owned_by: "mock".to_string(),
            },
            ModelInfo {
                id: "text-embedding-ada-002".to_string(),
                object: "model".to_string(),
                created,
                owned_by: "mock".to_string(),
            },
        ],
    })
}

/// GET /health
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        server: "mock-llm-server".to_string(),
    })
}

// ════════════════════════════════════════════════
// Main
// ════════════════════════════════════════════════

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mock_llm_server=info".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    let config = Arc::new(AppConfig {
        delay: Duration::from_millis(cli.delay_ms),
        error_rate: cli.error_rate.clamp(0.0, 1.0),
    });

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/completions", post(completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/models", get(models))
        .route("/health", get(health))
        .with_state(config);

    let addr = format!("{}:{}", cli.host, cli.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap_or_else(|e| {
        eprintln!("Failed to bind to {}: {}", addr, e);
        std::process::exit(1);
    });

    info!(
        "🚀 Mock LLM Server listening on {} (delay={}ms, error_rate={:.1}%)",
        addr, cli.delay_ms, cli.error_rate * 100.0
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            info!("Shutting down...");
        })
        .await
        .unwrap();
}
