# prism-openai-compat

Example WASM proxy plugin for Prism — OpenAI compatibility layer.

## What it does

Normalizes non-standard API requests into the OpenAI format:

| Feature | Input | Output |
|---|---|---|
| **Path rewrite** | `/api/chat` | `/v1/chat/completions` |
| **Header injection** | — | `X-Prism-Converted: true` |
| **Request body** | `{"prompt": "..."}` | `{"messages": [{"role":"user","content":"..."}]}` |
| **Response body** | `{"id": "..."}` | `{"id": "...", "prism_converted": true}` |

## Architecture

This plugin implements the proxy-wasm ABI directly — no SDK dependency. It exports the standard proxy-wasm lifecycle and hook functions, and imports host functions from the `"env"` module.

## Build

```bash
# Prerequisites
rustup target add wasm32-unknown-unknown

# Build
cargo build --target wasm32-unknown-unknown --release
```

Output: `target/wasm32-unknown-unknown/release/prism_openai_compat.wasm`

## Usage

Copy the `.wasm` file to your Prism configuration directory and reference it in your proxy plugin chain configuration.

## Testing

From the workspace root:

```bash
# Build the plugin and run WASM integration tests
make test-wasm
```

## How it works

The plugin uses a fixed memory layout:
- **Static strings** (`:path`, `/v1/chat/completions`, etc.) are stored as Rust `static` byte slices
- **Return value slots** — mutable statics where the host writes pointers and sizes
- **Body write buffer** — 32 KiB static buffer for writing transformed bodies
- **Bump allocator** — `proxy_on_memory_allocate` provides memory for host callbacks

### Exported functions (proxy-wasm ABI)

- `proxy_on_memory_allocate` — memory allocation for host
- `proxy_on_context_create` — context lifecycle
- `proxy_on_configure` — plugin configuration
- `proxy_on_request_headers` — path rewrite + header injection
- `proxy_on_request_body` — prompt→messages body transformation
- `proxy_on_response_headers` — pass-through
- `proxy_on_response_body` — adds `prism_converted: true`

### Imported host functions

All imported from the `"env"` module:

- `proxy_get_header_map_value` — read a header value
- `proxy_replace_header_map_value` — replace a header value
- `proxy_add_header_map_value` — add a new header
- `proxy_get_buffer_bytes` — read request/response body
- `proxy_set_buffer_bytes` — write request/response body
- `proxy_log` — log messages to host
