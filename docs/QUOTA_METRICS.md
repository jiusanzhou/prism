# 可扩展指标限流设计

## 现状

当前 `RateLimitBackend` trait 方法签名写死了两个指标：

```rust
fn check_and_increment(&self, key: &str, window_secs: u64, max_requests: u64, max_tokens: Option<u64>)
fn record_tokens(&self, key: &str, tokens: u64)
```

无法扩展到 input_tokens / output_tokens / cost / concurrency 等维度。

---

## 设计

### 核心类型

```rust
/// 限流指标类型
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaMetric {
    /// 请求数 — 请求阶段自动 +1
    Requests,
    /// 总 token 数 (input + output)
    TotalTokens,
    /// 输入 token 数
    InputTokens,
    /// 输出 token 数
    OutputTokens,
    /// 费用（单位: 0.01 分，整数运算避免浮点）
    CostMicros,
    /// 并发请求数（特殊: gauge 而非 counter）
    Concurrency,
    /// 自定义指标
    Custom(String),
}

/// 单个配额条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quota {
    /// 指标类型
    pub metric: QuotaMetric,
    /// 上限值
    pub limit: u64,
}

/// 配额超限结果
#[derive(Debug)]
pub struct QuotaDenied {
    /// 哪个指标被触发
    pub metric: QuotaMetric,
    /// 当前值
    pub current: u64,
    /// 限额
    pub limit: u64,
    /// 重试等待秒数
    pub retry_after_secs: u64,
}
```

### Backend trait 改造

```rust
pub trait RateLimitBackend: Send + Sync {
    /// 检查所有配额，对 Requests 类型自动 +1
    /// 其他类型只做 check（值由 record_metric 写入）
    fn check_and_increment(
        &self,
        key: &str,
        window_secs: u64,
        quotas: &[Quota],
    ) -> Result<(), QuotaDenied>;

    /// 记录指标值（响应阶段回写）
    fn record_metric(&self, key: &str, metric: &QuotaMetric, value: u64);

    /// 清理过期条目
    fn cleanup(&self, max_window_secs: u64);

    fn name(&self) -> &str;
    fn is_healthy(&self) -> bool { true }
}
```

### 配置

```yaml
rate_limits:
  - name: per-model
    key_headers: ["x-model"]
    window_secs: 60
    quotas:
      - metric: requests
        limit: 100
      - metric: total_tokens
        limit: 50000
      - metric: input_tokens
        limit: 30000
    overrides:
      - match_headers: { "x-model": "gpt-4" }
        quotas:
          - metric: requests
            limit: 50           # gpt-4 请求数更严
          - metric: total_tokens
            limit: 100000       # 但 token 更宽松
```

### 向后兼容

旧配置格式仍然支持，内部转换为新格式：

```yaml
# 旧格式（继续工作）
rate_limits:
  - name: global
    window_secs: 60
    max_requests: 100
    max_tokens: 50000

# 内部等价于
rate_limits:
  - name: global
    window_secs: 60
    quotas:
      - metric: requests
        limit: 100
      - metric: total_tokens
        limit: 50000
```

### LocalBackend 存储

```rust
// 现在: DashMap<String, (window_start, request_count, token_count)>
// 改后: DashMap<String, WindowState>

struct WindowState {
    window_start: u64,
    /// metric_name → current_value
    counters: HashMap<QuotaMetric, u64>,
}
```

### RedisBackend 存储

```
# 现在
ZSET  prism:rl:{key}              → 请求计数
KEY   prism:rl:{key}:tokens       → token 计数

# 改后：每个指标独立的 key
KEY   prism:rl:{key}:requests     → 请求计数 (ZSET 滑动窗口)
KEY   prism:rl:{key}:total_tokens → token 计数 (INCRBY + EXPIRE)
KEY   prism:rl:{key}:input_tokens → input token 计数
KEY   prism:rl:{key}:cost_micros  → 费用计数
```

Requests 用 ZSET 滑动窗口（精确），其他指标用简单的 INCRBY + EXPIRE（固定窗口，足够准确）。

### Proxy 调用侧

```rust
// 请求阶段 — check_and_increment（自动 +1 requests）
limiter.check(headers)?;

// 响应阶段 — 回写实际指标
limiter.record_metric(rule_name, headers, QuotaMetric::TotalTokens, usage.total_tokens);
limiter.record_metric(rule_name, headers, QuotaMetric::InputTokens, usage.prompt_tokens);
limiter.record_metric(rule_name, headers, QuotaMetric::OutputTokens, usage.completion_tokens);
```

### RateLimitDenied 改造

```rust
pub struct RateLimitDenied {
    pub rule_name: String,
    pub status_code: u16,
    pub message: String,
    pub retry_after_secs: Option<u64>,
    /// 新增：哪个指标触发的
    pub denied_metric: QuotaMetric,
    pub current_value: u64,
    pub limit_value: u64,
}
```

响应体可以包含更详细的信息：

```json
{
    "error": {
        "type": "rate_limit_exceeded",
        "rule": "per-model",
        "metric": "total_tokens",
        "current": 49832,
        "limit": 50000,
        "retry_after": 45
    }
}
```

---

## 改动范围

| 文件 | 改动 |
|---|---|
| `ratelimit/mod.rs` | QuotaMetric / Quota / QuotaDenied 类型，RateLimitRule 加 quotas 字段，兼容旧 max_requests/max_tokens |
| `ratelimit/backend.rs` | trait 方法签名改为 quotas + record_metric |
| `ratelimit/local.rs` | WindowState 改为 HashMap<QuotaMetric, u64> |
| `ratelimit/redis_backend.rs` | 每指标独立 key，Requests 保留 ZSET，其他用 INCRBY |
| `proxy.rs` | 响应阶段回写 record_metric（替代 record_tokens） |
| `config/types.rs` | RateLimitConfig 加 quotas 字段 |
| 测试 | 全部更新 + 新增多指标测试 |

---

## 不在范围

- **Concurrency（并发数）**：需要 gauge 语义（+1/-1），与 counter 窗口模型不同，后续单独做
- **自动 token 提取**：从上游 OpenAI 响应自动解析 usage，属于 proxy 层功能，不在限流模块
