# Proxy Body Forwarding — Pingora 踩坑记录

## 问题

Prism 使用 Pingora 做反向代理，路由层需要读取请求 body 中的 `model` 字段来决定路由。读取 body 后，upstream 收到空 body（`Required request body is missing`）。

## Pingora ProxyHttp 生命周期

```
request_filter → upstream_peer → upstream_request_filter → request_body_filter → [upstream] → response_filter → response_body_filter → logging
```

关键约束：
- `upstream_peer` 需要 model name 做路由决策
- model name 在 request body（JSON `{"model": "xxx", ...}`）里
- body 只能通过 `request_body_filter` 流式传输给 upstream
- `request_body_filter` 在 `upstream_peer` **之后**才被调用

## 尝试过的方案

### ❌ 方案 1：request_filter 中直接 read_request_body

```rust
// request_filter 中
let mut full_body = Vec::new();
loop {
    match session.downstream_session.read_request_body().await {
        Ok(Some(bytes)) => full_body.extend_from_slice(&bytes),
        Ok(None) => break,
        Err(_) => break,
    }
}
```

**失败原因**：`read_request_body()` 消耗了 downstream body stream。之后 `is_body_done()` 返回 true，`DownstreamStateMachine` 初始化为 `ReadingFinished`。Pingora 的 `proxy_handle_downstream` 中 `read_body_or_idle(is_done=true)` 会无限挂起，**`request_body_filter` 永远不会被调用**。

### ❌ 方案 2：request_body_filter 中缓冲（不预读 body）

不在 request_filter 读 body，让 request_body_filter 正常收到 body chunks。

**失败原因**：`upstream_peer` 在 `request_body_filter` 之前被调用，此时没有 model name，无法按模型路由。

### ❌ 方案 3：enable_retry_buffering + read_request_body，但只删 Content-Length

```rust
session.downstream_session.enable_retry_buffering();
// ... read body ...
// upstream_request_filter 中:
upstream_request.remove_header("content-length");
```

**失败原因**：Pingora 的 `BodyWriter` 初始化逻辑（`init_body_writer_comm`）：

```rust
fn init_body_writer_comm(&mut self, headers: &HMap) {
    if is_header_value_chunked_encoding(te_value) {
        self.body_writer.init_chunked();           // ← chunked
    } else if let Some(length) = content_length {
        self.body_writer.init_content_length(length); // ← 固定长度
    } else {
        self.body_writer.init_http10();            // ← 无 body！
    }
}
```

只删 Content-Length 不加 Transfer-Encoding，BodyWriter 回退到 `init_http10()`（HTTP/1.0 close-delimited），**`write_body()` 被静默忽略，body 丢失**。

### ✅ 方案 4：enable_retry_buffering + chunked encoding（最终方案）

```rust
// request_filter 中：
session.downstream_session.enable_retry_buffering();
loop { read_request_body()... } // 提取 model name

// upstream_request_filter 中：
upstream_request.remove_header("content-length");
upstream_request.insert_header("transfer-encoding", "chunked");

// request_body_filter 中：
// Pingora 通过 retry buffer 回放 body，正常做 model rewrite / codec
```

**工作原理**：

1. `enable_retry_buffering()` 让 Pingora 在读 body 时同时保存到 retry buffer
2. `read_request_body()` 消耗 stream 但 retry buffer 保留了完整 body
3. `proxy_handle_downstream` 检测到 retry buffer 存在，通过 `send_body_to_pipe` 发送
4. `send_body_to_pipe` 调用 `request_body_filter`，我们可以做 model rewrite
5. 设置 `Transfer-Encoding: chunked` 确保 BodyWriter 用 chunked 模式写 body

## Pingora retry buffer 流程（源码分析）

```
proxy_1to1()
  └─ enable_retry_buffering()  // line 103, proxy_h1.rs
  └─ proxy_handle_downstream()
       ├─ downstream_state = DownstreamStateMachine::new(is_body_done())
       │   // 如果 body 已消费: ReadingFinished
       ├─ buffer = get_retry_buffer()  // 获取之前保存的 body
       ├─ if buffer.is_some():
       │    send_body_to_pipe(data=buffer, end_of_body=is_done(), ...)
       │    └─ request_body_filter(body=Some(buffer), end_of_stream=true)
       │       // ← 我们在这里做 model rewrite
       └─ while loop: read_body_or_idle(is_done)
            // is_done=true 时永远 idle，不会再读
```

## 已知限制

- **chunked encoding 兼容性**：部分 upstream（如基于 Spring 的 API）不支持 `Transfer-Encoding: chunked`，会返回 `Required request body is missing`。这些需要通过 reqwest-based forwarding 绕过 Pingora connector。
- **body 大小**：retry buffer 有大小限制（`BODY_BUF_LIMIT`），超大 body 会被截断，`retry_buffer_truncated()` 返回 true，`get_retry_buffer()` 返回 None。

## 关键源码位置

| 文件 | 行 | 说明 |
|------|-----|------|
| `pingora-proxy/src/proxy_h1.rs` | 103 | `enable_retry_buffering()` 调用 |
| `pingora-proxy/src/proxy_h1.rs` | 289-295 | retry buffer 检测和发送 |
| `pingora-proxy/src/proxy_h1.rs` | 710-756 | `send_body_to_pipe` → `request_body_filter` |
| `pingora-proxy/src/proxy_common.rs` | 3-50 | `DownstreamStateMachine` 状态机 |
| `pingora-core/.../v1/client.rs` | 621-648 | `init_body_writer_comm` BodyWriter 初始化 |
| `pingora-core/.../v1/server.rs` | 832-845 | retry buffer 实现 |
