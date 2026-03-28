use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::OnceLock;

use crate::analyzer::ChatMessage;
use crate::feedback::OutcomeReport;
use crate::registry::{InMemoryRegistry, ModelRegistryTrait};
use crate::router::{PrismRouter, RouteMetadata, RouteRequest, Router};
use crate::selector::{create_l2_selector, create_l3_selector, create_l4_selector, SelectorConfig};
use crate::strategy::ModelSelectionStrategy;
use crate::types::*;

// ──────────────────────────────────────────────
// Global state
// ──────────────────────────────────────────────

static ROUTER: OnceLock<PrismRouter> = OnceLock::new();

/// Wrapper so we can store a `*const c_char` in `OnceLock` (which requires `Send + Sync`).
///
/// SAFETY: The wrapped pointer is produced by `CString::into_raw` and is never freed
/// (intentional leak for process-lifetime storage). The pointed-to data is immutable
/// after creation, so sharing across threads is safe.
struct StaticCStr(*const c_char);
unsafe impl Send for StaticCStr {}
unsafe impl Sync for StaticCStr {}

static VERSION: OnceLock<StaticCStr> = OnceLock::new();

// ──────────────────────────────────────────────
// FFI structs (C ABI)
// ──────────────────────────────────────────────

/// Request passed over FFI boundary.
#[repr(C)]
pub struct PrismRouteRequest {
    pub messages_json: *const c_char,
    pub strategy_json: *const c_char,
    pub metadata_json: *const c_char,
    pub analyze_context_json: *const c_char,
    pub required_protocol: *const c_char,
}

/// Result returned over FFI boundary.
#[repr(C)]
pub struct PrismRouteResult {
    pub model_id: *const c_char,
    pub provider: *const c_char,
    pub variant_id: *const c_char,
    pub account_id: *const c_char,
    pub fallbacks_json: *const c_char,
    pub reason_json: *const c_char,
    pub decision_time_us: u64,
    pub error: *const c_char,
}

// ──────────────────────────────────────────────
// Config deserialization types
// ──────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct InitConfig {
    #[serde(default)]
    providers: Vec<Provider>,
    #[serde(default)]
    endpoints: Vec<Endpoint>,
    #[serde(default)]
    accounts: Vec<Account>,
    #[serde(default)]
    models: Vec<Model>,
    #[serde(default)]
    variants: Vec<ModelVariant>,
    #[serde(default)]
    pools: Vec<ModelPool>,
    default_strategy: Option<ModelSelectionStrategy>,
    selector: Option<SelectorConfig>,
}

#[derive(serde::Deserialize)]
struct StatsUpdate {
    variant_id: String,
    #[serde(flatten)]
    performance: ModelPerformance,
}

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

/// Convert a `*const c_char` to `&str`. Returns `None` if null or invalid UTF-8.
///
/// # Safety
/// The pointer must be null or point to a valid, NUL-terminated C string.
unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    CStr::from_ptr(ptr).to_str().ok()
}

/// Allocate a C string on the heap. Returns null on failure.
fn to_c_string(s: &str) -> *const c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw() as *const c_char,
        Err(_) => std::ptr::null(),
    }
}

/// Build an error-only `PrismRouteResult`.
fn error_result(msg: &str) -> *mut PrismRouteResult {
    let result = Box::new(PrismRouteResult {
        model_id: std::ptr::null(),
        provider: std::ptr::null(),
        variant_id: std::ptr::null(),
        account_id: std::ptr::null(),
        fallbacks_json: std::ptr::null(),
        reason_json: std::ptr::null(),
        decision_time_us: 0,
        error: to_c_string(msg),
    });
    Box::into_raw(result)
}

// ──────────────────────────────────────────────
// FFI entry points
// ──────────────────────────────────────────────

/// Initialise the routing engine from a JSON configuration blob.
///
/// Returns 0 on success, -1 if the config JSON is invalid, -2 if already initialised.
///
/// # Safety
///
/// `config_json` must be null or a valid pointer to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn prism_init(config_json: *const c_char) -> i32 {
    let json_str = match unsafe { cstr_to_str(config_json) } {
        Some(s) => s,
        None => return -1,
    };

    let config: InitConfig = match serde_json::from_str(json_str) {
        Ok(c) => c,
        Err(_) => return -1,
    };

    // Build registry
    let mut registry = InMemoryRegistry::new();

    for p in config.providers {
        registry.add_provider(p);
    }
    for e in config.endpoints {
        registry.add_endpoint(e);
    }
    for a in config.accounts {
        registry.add_account(a);
    }
    for m in config.models {
        registry.add_model(m);
    }
    for v in config.variants {
        registry.add_variant(v);
    }
    for p in config.pools {
        registry.add_pool(p);
    }

    // Build router
    let mut router = PrismRouter::new(registry);
    if let Some(strategy) = config.default_strategy {
        router = router.with_strategy(strategy);
    }

    // Apply selector configuration
    if let Some(ref sel_cfg) = config.selector {
        router = router
            .with_l2(create_l2_selector(&sel_cfg.l2))
            .with_l3(create_l3_selector(&sel_cfg.l3))
            .with_l4(create_l4_selector(&sel_cfg.l4));
    }

    // Store globally (OnceLock — first call wins)
    match ROUTER.set(router) {
        Ok(()) => 0,
        Err(_) => -2, // already initialised
    }
}

/// Perform a routing decision.
///
/// Caller **must** free the result with `prism_free_result`.
///
/// # Safety
///
/// `request` must be null or a valid pointer to a `PrismRouteRequest` struct
/// whose string fields are null or valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn prism_route(request: *const PrismRouteRequest) -> *mut PrismRouteResult {
    // Get the global router
    let router = match ROUTER.get() {
        Some(r) => r,
        None => return error_result("router not initialised: call prism_init first"),
    };

    if request.is_null() {
        return error_result("null request pointer");
    }

    let req = unsafe { &*request };

    // Parse messages (required)
    let messages_str = match unsafe { cstr_to_str(req.messages_json) } {
        Some(s) if !s.is_empty() => s,
        _ => return error_result("messages_json is null or empty"),
    };
    let messages: Vec<ChatMessage> = match serde_json::from_str(messages_str) {
        Ok(m) => m,
        Err(e) => return error_result(&format!("failed to parse messages_json: {e}")),
    };

    // Parse strategy (optional — null/empty uses default)
    let strategy: Option<ModelSelectionStrategy> = unsafe { cstr_to_str(req.strategy_json) }
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str(s).ok());

    // Parse metadata (optional — null/empty uses default)
    let metadata: RouteMetadata = unsafe { cstr_to_str(req.metadata_json) }
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    // Parse analyze context (optional — null/empty means None)
    let analyze_context: Option<crate::analyzer::AnalyzeContext> =
        unsafe { cstr_to_str(req.analyze_context_json) }
            .filter(|s| !s.is_empty())
            .and_then(|s| serde_json::from_str(s).ok());

    // Parse required protocol (optional — null/empty means None)
    let required_protocol: Option<String> = unsafe { cstr_to_str(req.required_protocol) }
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let route_req = RouteRequest {
        messages,
        strategy,
        metadata,
        analyze_context,
        requested_model: None,
        inflight_snapshot: None,
        prefix_cache_index: None,
        required_protocol,
        required_capability: None,
        content_hint: None,
    };

    // Execute routing
    match router.route(&route_req) {
        Ok(decision) => {
            let fallbacks_json = serde_json::to_string(&decision.fallback_chain)
                .unwrap_or_else(|_| "[]".to_string());
            let reason_json =
                serde_json::to_string(&decision.reason).unwrap_or_else(|_| "{}".to_string());

            let result = Box::new(PrismRouteResult {
                model_id: to_c_string(&decision.selected_model),
                provider: to_c_string(&decision.selected_provider),
                variant_id: to_c_string(&decision.selected_variant),
                account_id: to_c_string(&decision.selected_account),
                fallbacks_json: to_c_string(&fallbacks_json),
                reason_json: to_c_string(&reason_json),
                decision_time_us: decision.decision_time_us,
                error: std::ptr::null(),
            });
            Box::into_raw(result)
        }
        Err(e) => error_result(&e.to_string()),
    }
}

/// Update real-time model performance statistics.
///
/// Accepts JSON in two formats:
/// - Array: `[{"variant_id": "xxx", "current_qps": ..., ...}, ...]`
/// - Single object: `{"variant_id": "xxx", "current_qps": ..., ...}`
///
/// Returns 0 on success, -1 on parse error, -2 if router not initialised.
///
/// # Safety
///
/// `stats_json` must be null or a valid pointer to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn prism_update_model_stats(stats_json: *const c_char) -> i32 {
    let router = match ROUTER.get() {
        Some(r) => r,
        None => return -2,
    };

    let json_str = match unsafe { cstr_to_str(stats_json) } {
        Some(s) => s,
        None => return -1,
    };

    // Try array first, then single object (backward compatible).
    let updates: Vec<StatsUpdate> = match serde_json::from_str(json_str) {
        Ok(arr) => arr,
        Err(_) => match serde_json::from_str::<StatsUpdate>(json_str) {
            Ok(single) => vec![single],
            Err(_) => return -1,
        },
    };

    for update in updates {
        router
            .registry
            .update_performance(&update.variant_id, update.performance);
    }

    0
}

/// Free a result previously returned by `prism_route`.
///
/// # Safety
///
/// `result` must be null or a valid pointer previously returned by `prism_route`.
#[no_mangle]
pub unsafe extern "C" fn prism_free_result(result: *mut PrismRouteResult) {
    if result.is_null() {
        return;
    }
    unsafe {
        let r = Box::from_raw(result);
        free_c_string(r.model_id);
        free_c_string(r.provider);
        free_c_string(r.variant_id);
        free_c_string(r.account_id);
        free_c_string(r.fallbacks_json);
        free_c_string(r.reason_json);
        free_c_string(r.error);
    }
}

/// Return the crate version as a static C string.
///
/// The returned pointer is valid for the lifetime of the process and must **not** be freed.
#[no_mangle]
pub extern "C" fn prism_version() -> *const c_char {
    VERSION
        .get_or_init(|| {
            let version = env!("CARGO_PKG_VERSION");
            StaticCStr(
                CString::new(version)
                    .expect("version string contains no NUL bytes")
                    .into_raw() as *const c_char,
            )
        })
        .0
}

/// Return a JSON health-check payload.
///
/// Caller **must** free the returned string with `prism_free_string`.
///
/// If the router is not initialised, returns `{"status": "not_initialised"}`.
#[no_mangle]
pub extern "C" fn prism_health() -> *const c_char {
    match ROUTER.get() {
        Some(router) => {
            let registered_models = router.registry.models.len();
            let active_variants = router
                .registry
                .variants
                .values()
                .filter(|v| v.status == VariantStatus::Active)
                .count();

            let json = serde_json::json!({
                "status": "healthy",
                "registered_models": registered_models,
                "active_variants": active_variants,
            });

            to_c_string(&json.to_string())
        }
        None => to_c_string(r#"{"status":"not_initialised"}"#),
    }
}

/// Free a C string returned by `prism_health` or similar heap-allocated strings.
///
/// Do **not** use this on `prism_version` (that pointer is static / leaked).
#[no_mangle]
pub extern "C" fn prism_free_string(ptr: *const c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr as *mut c_char);
        }
    }
}

/// Record an outcome report for the feedback loop.
///
/// Expects JSON matching `OutcomeReport`. Returns 0 on success, -1 on parse error,
/// -2 if the router is not initialised.
///
/// # Safety
///
/// `outcome_json` must be null or a valid pointer to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn prism_record_outcome(outcome_json: *const c_char) -> i32 {
    let router = match ROUTER.get() {
        Some(r) => r,
        None => return -2,
    };

    let json_str = match unsafe { cstr_to_str(outcome_json) } {
        Some(s) => s,
        None => return -1,
    };

    let report: OutcomeReport = match serde_json::from_str(json_str) {
        Ok(r) => r,
        Err(_) => return -1,
    };

    router.record_outcome(&report);
    0
}

// ──────────────────────────────────────────────
// Internal helpers
// ──────────────────────────────────────────────

unsafe fn free_c_string(ptr: *const c_char) {
    if !ptr.is_null() {
        let _ = CString::from_raw(ptr as *mut c_char);
    }
}

// ──────────────────────────────────────────────
// FFI unit tests
// ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_prism_version() {
        let version = prism_version();
        assert!(!version.is_null());
        let s = unsafe { CStr::from_ptr(version) }.to_str().unwrap();
        assert_eq!(s, env!("CARGO_PKG_VERSION"));
        // Do NOT free — it's a leaked static pointer
    }

    #[test]
    fn test_prism_health_returns_json() {
        let health = prism_health();
        assert!(!health.is_null());
        let s = unsafe { CStr::from_ptr(health) }.to_str().unwrap();
        assert!(s.contains("status"));
        prism_free_string(health);
    }

    #[test]
    fn test_prism_init_null_ptr() {
        let result = unsafe { prism_init(std::ptr::null()) };
        assert_eq!(result, -1);
    }

    #[test]
    fn test_prism_init_invalid_json() {
        let bad = CString::new("not json").unwrap();
        let result = unsafe { prism_init(bad.as_ptr()) };
        assert_eq!(result, -1);
    }

    #[test]
    fn test_prism_route_null_request() {
        let result = unsafe { prism_route(std::ptr::null()) };
        assert!(!result.is_null());
        let r = unsafe { &*result };
        assert!(!r.error.is_null());
        unsafe { prism_free_result(result) };
    }

    #[test]
    fn test_prism_free_result_null() {
        // Must not crash
        unsafe { prism_free_result(std::ptr::null_mut()) };
    }

    #[test]
    fn test_prism_free_string_null() {
        // Must not crash
        prism_free_string(std::ptr::null());
    }

    #[test]
    fn test_prism_update_model_stats_null() {
        let result = unsafe { prism_update_model_stats(std::ptr::null()) };
        assert!(result < 0);
    }

    #[test]
    fn test_prism_update_model_stats_invalid_json() {
        let bad = CString::new("not json").unwrap();
        let result = unsafe { prism_update_model_stats(bad.as_ptr()) };
        assert!(result < 0);
    }
}
