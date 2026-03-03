use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;



/// GET /api/variants
pub async fn list_variants(State(state): State<AppState>) -> impl IntoResponse {
    match std::fs::read_to_string(&state.config_path)
        .map_err(|e| e.to_string())
        .and_then(|y| prism_router::config::load_config_from_str(&y))
    {
        Ok(config) => {
            let variants: Vec<serde_json::Value> = config.variants.iter().map(|v| {
                serde_json::json!({
                    "id": v.id,
                    "model_id": v.model_id,
                    "provider_id": v.provider_id,
                    "weight": v.weight,
                })
            }).collect();
            (StatusCode::OK, Json(serde_json::json!({"variants": variants}))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response(),
    }
}




