# Prism 分布式部署方案

## 现状分析

Prism 当前是**单实例架构**，所有运行时状态存储在进程内存中：

| 组件 | 当前实现 | 问题 |
|---|---|---|
| 限流 | `DashMap` 滑动窗口 | 多实例各自计数，实际限额 = 配置 × N |
| 指标/反馈 | `PerformanceDataSource` (HashMap) | 实例间指标隔离，路由决策不一致 |
| 配置热更新 | `notify` 文件监听 + ArcSwap | 只监听本地文件，无跨实例同步 |
| 注册表 | `InMemoryRegistry` | 每实例独立加载，无变更广播 |

**好消息**：核心 trait 都已抽象（`DataSource`、`ConfigSource`、`Discovery`），只需写新的实现即可。

---

## 业界参考

| 网关 | 限流 Backend | 指标存储 | 配置同步 |
|---|---|---|---|
| **Kong** | local / cluster(PG) / redis | PG / Prometheus | PG + 集群广播 |
| **APISIX** | local / redis / redis-cluster | etcd + Prometheus | etcd watch |
| **Envoy** | 外部 gRPC 服务 (redis/memcache) | xDS + Prometheus | xDS control plane |
| **Higress** | 本地内存 | Nacos / K8s | Nacos / K8s ConfigMap |
| **Traefik** | 本地内存 | 内存 + Prometheus | 文件 / K8s / Consul |

**共识**：限流 backend 抽象为可插拔策略，用户按规模选择。小规模 local 零依赖，大规模 redis 精确计数。

---

## 设计原则

1. **Backend 可插拔** — trait 抽象，内置 `local` + `redis`，可扩展 `memcached` / `external-grpc`
2. **Feature Flag 隔离** — `cargo build` 纯单机，`--features redis` 启用 Redis 支持
3. **优雅降级** — Redis 断连自动回退 local，不影响可用性
4. **零 Breaking Change** — 不设 `policy` 字段时行为与当前完全一致
5. **按需启用** — 限流、指标、配置同步三个维度独立配置

---

## 目标架构

```
                    ┌─────────────────┐
                    │   Load Balancer  │
                    │  (Nginx/HAProxy) │
                    └────────┬────────┘
                             │
              ┌──────────────┼──────────────┐
              ▼              ▼              ▼
        ┌──────────┐   ┌──────────┐   ┌──────────┐
        │ Prism-1  │   │ Prism-2  │   │ Prism-3  │
        │          │   │          │   │          │
        │ Router   │   │ Router   │   │ Router   │
        │ Proxy    │   │ Proxy    │   │ Proxy    │
        │ Metrics  │   │ Metrics  │   │ Metrics  │
        └────┬─────┘   └────┬─────┘   └────┬─────┘
             │              │              │
             └──────────────┼──────────────┘
                            ▼
                    ┌───────────────┐
                    │  Redis (可选)  │
                    │  Sentinel /   │
                    │  Cluster      │
                    └───────────────┘
```

---

## 分阶段实施

### Phase 1: 限流 Backend 抽象 + Redis 实现（最高优先级）

**问题**：多实例部署时限流形同虚设。

#### 1.1 RateLimitBackend trait

```rust
// prism-router/src/ratelimit/backend.rs

/// 限流计数后端 — 可插拔实现
pub trait RateLimitBackend: Send + Sync {
    /// 检查并递增计数器
    /// 成功返回 Ok(())，超限返回 Err(retry_after_secs)
    fn check_and_increment(
        &self,
        key: &str,
        window_secs: u64,
        max_requests: u64,
        max_tokens: Option<u64>,
    ) -> Result<(), u64>;

    /// 记录 token 用量（响应阶段回写）
    fn record_tokens(&self, key: &str, tokens: u64);

    /// 清理过期条目
    fn cleanup(&self, max_window_secs: u64);

    /// Backend 名诊断）
    fn name(&self) -> &str;

    /// 健康检查
    fn is_healthy(&self) -> bool { true }
}
```

#### 1.2 LocalBackend（重构现有实现）

将现有 `SlidingWindowCounter` 包装为 `LocalBackend`，实现 `RateLimitBackend` trait。
逻辑不变，纯重构。

```rust
// prism-router/src/ratelimit/local.rs

pub struct LocalBackend {
    windows: DashMap<String, (u64, u64, u64)>,
}

impl RateLimitBackend for LocalBackend {
    fn name(&self) -> &str { "local" }
    // ... 现有 SlidingWindowCounter 逻辑搬过来
}
```

#### 1.3 RedisBackend（新增）

```rust
// prism-router/src/ratelimit/redis_backend.rs

#[cfg(feature = "redis")]
pub struct RedisBackend {
    client: redis::aio::ConnectionManager,
    script: redis::Script,
    /// 本地 fallback（Redis 不可用时使用）
    fallback: LocalBackend,
    /// 降级策略
    degradation: DegradationPolicy,
}

#[derive(Debug, Clone, Copy)]
pub enum DegradationPolicy {
    /// Redis 挂了放行所有请求
    Allow,
    /// Redis 挂了拒绝所有请求
    Deny,
    /// Redis 挂了回退本地计数（默认）
    Local,
}
```

#### 1.4 Lua 脚本（Redis 原子滑动窗口）

```lua
-- lua/sliding_window.lua
-- KEYS[1] = rate limit key
-- ARGV[1] = window_secs
-- ARGV[2] = max_requests
-- ARGV[3] = now (unix secs)
-- Returns: [allowed (0/1), current_count, retry_after_secs]

local key = KEYS[1]
local window = tonumber(ARGV[1])
local limit = tonumber(ARGV[2])
local now = tonumber(ARGV[3])

local window_start = now - window
redis.call('ZREMRANGEBYSCORE', key, '-inf', window_start)

local count = redis.call('ZCARD', key)
if count >= limit then
    local oldest = redis.call('ZRANGEBYSCORE', key, '-inf', '+inf', 'LIMIT', 0, 1)
    local retry = 0
    if #oldest > 0 then
        local oldest_score = tonumber(redis.call('ZSCORE', key, oldest[1]))
        retry = math.max(0, window - (now - oldest_score))
    end
    return {0, count, retry}
end

redis.call('ZADD', key, now, now .. ':' .. math.random(1000000))
redis.call('EXPIRE', key, window + 1)
return {1, count + 1, 0}
```

#### 1.5 Token 计数 Lua 脚本

```lua
-- lua/record_tokens.lua
-- KEYS[1] = token counter key (rate limit key + ":tokens")
-- ARGV[1] = tokens to add
-- ARGV[2] = window_secs (for TTL)

local key = KEYS[1]
local tokens = tonumber(ARGV[1])
local window = tonumber(ARGV[2])

local current = redis.call('INCRBY', key, tokens)
-- 设置 TTL（仅在首次创建时）
if current == tokens then
    redis.call('EXPIRE', key, window + 1)
end
return current
```

#### 1.6 RateLimiter 重构

```rust
// prism-router/src/ratelimit/mod.rs

pub struct RateLimiter {
    rules: Vec<RateLimitRule>,
    backend: Arc<dyn RateLimitBackend>,
}

impl RateLimiter {
    /// 根据配置创建对应 backend
    pub fn new(rules: Vec<RateLimitRule>, policy: &str, redis_opts: Option<&RedisConfig>) -> Self {
        let backend: Arc<dyn RateLimitBackend> = match policy {
            #[cfg(feature = "redis")]
            "redis" => {
                let opts = redis_opts.expect("redis config required when policy=redis");
                Arc::new(RedisBackend::new(opts).unwrap_or_else(|e| {
                    warn!("Redis backend init failed: {}, falling back to local", e);
                    Arc::new(LocalBackend::new())
                }))
            }
            _ => Arc::new(LocalBackend::new()),
        };
        Self { rules, backend }
    }

    pub fn check(&self, headers: &[(String, String)]) -> Result<(), RateLimitDenied> {
        for rule in &self.rules {
            if !rule.enabled { continue; }
            let key = Self::build_key(rule, headers);
            let (eff_max, eff_tokens) = Self::resolve_limits(rule, headers);

            if let Err(retry_after) = self.backend.check_and_increment(
                &key, rule.window_secs, eff_max, eff_tokens,
            ) {
                return Err(RateLimitDenied { /* ... */ });
            }
        }
        Ok(())
    }
}
```

#### 1.7 配置

```yaml
# prism.yaml — 限流配置
rate_limits:
  - name: per-model
    key_headers: ["x-model"]
    window_secs: 60
    max_requests: 100
    max_tokens: 50000

# 全局限流后端设置
rate_limit_policy:
  # local | redis (默认 local，向后兼容)
  backend: redis
  redis:
    url: "redis://redis:6379"
    # Redis Sentinel 支持
    # sentinel_urls: ["redis://s1:26379", "redis://s2:26379"]
    # sentinel_master: "mymaster"
    # Redis Cluster 支持
    # cluster_urls: ["redis://c1:6379", "redis://c2:6379"]
    password: ""
    database: 0
    # 连接池
    pool_size: 10
    connect_timeout_ms: 1000
    # key 前缀（多集群共用同一 Redis 时隔离）
    key_prefix: "prism"
  # Redis 不可用时的降级策略
  allow_degradation: true       # true = 回退 local, false = 拒绝请求
  degradation_policy: local     # allow | deny | local
```

#### 1.8 文件结构

```
prism-router/src/ratelimit/
├── mod.rs              # RateLimiter（重构，注入 backend）
├── backend.rs          # RateLimitBackend trait
├── local.rs            # LocalBackend（从现有代码重构）
├── redis_backend.rs    # RedisBackend（#[cfg(feature = "redis")]）
└── lua/
    ├── sliding_window.lua
    └── record_tokens.lua
```

#### 1.9 测试策略

- `LocalBackend` 测试：搬迁现有全部测试（零功能回归）
- `RedisBackend` 测试：集成测试，需要 Redis 实例（`#[cfg(test)]` 用 testcontainers 或 mock）
- `RateLimiter` 测试：mock backend，验证策略路由逻辑
- CI：默认跑 local 测试，`--features redis` 跑 Redis 集成测试（需要 Redis service）

---

### Phase 2: 分布式指标聚合

**问题**：各实例指标隔离，路由插件（InferenceLoad、PrefixAffinity）看到的是局部视图。

**方案**：实现 `RedisDataSource`，实现 `DataSource` trait。

#### 2.1 数据模型

```
Redis Hash: prism:perf:{variant_id}
  field: {instance_id}:p50_latency_ms    → "120"
  field: {instance_id}:p95_latency_ms    → "250"
  field: {instance_id}:current_qps       → "15.5"
  field: {instance_id}:error_rate        → "0.02"
  field: {instance_id}:queue_depth       → "3"
  field: {instance_id}:request_count     → "1542"
  field: {instance_id}:last_updated      → "1709420000"
  TTL: 15s (3 × flush interval)
```

#### 2.2 聚合策略

| 指标 | 聚合方式 | 理由 |
|---|---|---|
| QPS | Σ(各实例) | 全局总吞吐 |
| 延迟 P50/P95/P99 | request_count 加权平均 | 近似全局分位数 |
| Error Rate | request_count 加权平均 | 全局错误率 |
| Queue Depth | Σ(各实例) | 全局排队深度 |
| Throughput TPS | Σ(各实例) | 全局 token 吞吐 |

#### 2.3 RedisDataSource

```rust
#[cfg(feature = "redis")]
pub struct RedisDataSource {
    client: redis::aio::ConnectionManager,
    instance_id: String,
    flush_interval: Duration,      // 默认 5s
    /// 本地写入 buffer（异步 flush 到 Redis）
    local: PerformanceDataSource,
    /// 读缓存（避免每次路由都查 Redis）
    read_cache: Arc<RwLock<HashMap<String, (ModelPerformance, Instant)>>>,
    cache_ttl: Duration,           // 默认 2s
}

impl DataSource for RedisDataSource {
    fn name(&self) -> &str { "redis:performance" }
    fn tier(&self) -> DataTier { DataTier::NearRealtime }
    fn supported_metrics(&self) -> Vec<MetricKind> { vec![MetricKind::Performance] }

    fn get(&self, kind: MetricKind, key: &str) -> Option<MetricValue> {
        // 1. 先查读缓存
        // 2. 缓存 miss → 从 Redis HGETALL → 聚合所有 instance → 写入缓存
        // 3. Redis 不可用 → 回退 self.local.get()
    }

    fn update(&self, kind: MetricKind, key: &str, value: MetricValue) {
        // 写入本地 buffer，background task 定期 flush
        self.local.update(kind, key, value);
    }
}
```

#### 2.4 Background Flush Task

```rust
impl RedisDataSource {
    pub fn spawn_flush_task(&self) -> tokio::task::JoinHandle<()> {
        let client = self.client.clone();
        let instance_id = self.instance_id.clone();
        let local = self.local.clone(); // 需要 clone 支持
        let interval = self.flush_interval;

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                // 1. 从 local buffer 读取所有指标
                // 2. Pipeline HSET 到 Redis（batch）
                // 3. 设置 TTL
                // Redis 失败只打 warn 日志，不 panic
            }
        })
    }
}
```

#### 2.5 配置

```yaml
# prism.yaml
metrics:
  # local | redis (默认 local)
  backend: redis
  redis:
    url: "redis://redis:6379"
    key_prefix: "prism"
  flush_interval_secs: 5
  cache_ttl_secs: 2
```

---

### Phase 3: 配置同步

**问题**：各实例只能监听本地文件，配置变更需要逐台操作。

**方案**：Redis Pub/Sub 广播 reload 通知。

#### 3.1 工作流

```
/admin/reload (任意实例)
  │
  ├── 1. 重新加载本地配置文件
  ├── 2. build_registry → swap_router
  ├── 3. PUBLISH prism:config:reload { generation, source_instance }
  │
  ▼
Redis Pub/Sub Channel: prism:config:reload
  │
  ├──▶ Instance-2: 收到 → 检查 generation → 加载本地配置 → swap
  └──▶ Instance-3: 收到 → 检查 generation → 加载本地配置 → swap
```

**前提**：所有实例挂载同一配置文件（NFS / ConfigMap / S3 sync）。Redis 只传通知，不传配置内容。

#### 3.2 实现

```rust
pub struct RedisConfigSync {
    client: redis::aio::ConnectionManager,
    channel: String,     // "prism:config:reload"
    instance_id: String,
}

impl RedisConfigSync {
    /// 订阅配置变更通知
    pub async fn subscribe(&self, state: AppState) {
        let mut pubsub = self.client.get_async_pubsub().await.unwrap();
        pubsub.subscribe(&self.channel).await.unwrap();

        while let Some(msg) = pubsub.on_message().next().await {
            let payload: String = msg.get_payload().unwrap();
            // 跳过自己发的通知
            if payload.contains(&self.instance_id) { continue; }

            info!("Config reload notification from peer, reloading...");
            if let Ok(config) = load_config(&state.config_path) {
                let registry = build_registry(&config);
                state.swap_router(PrismRouter::new(registry));
            }
        }
    }

    /// 广播配置变更通知
    pub async fn notify_reload(&self, generation: u64) {
        let payload = format!("{{\"gen\":{},\"src\":\"{}\"}}", generation, self.instance_id);
        let _ = redis::cmd("PUBLISH")
            .arg(&self.channel)
            .arg(&payload)
            .query_async::<i64>(&mut self.client.clone())
            .await;
    }
}
```

#### 3.3 配置

```yaml
# prism.yaml
config_sync:
  # none | redis (默认 none，保持现有本地 file watch 行为)
  backend: redis
  redis:
    url: "redis://redis:6379"
    channel: "prism:config:reload"
```

---

### Phase 4: PrefixAffinity 分布式一致性（可选）

**问题**：PrefixAffinity 插件的前缀→模型亲和映射是实例局部的。

**方案**：Redis Hash 存储全局亲和映射。

```
prism:affinity:{prefix_hash} → variant_id
TTL: 30min (可配置)
```

- 写时：学习到新亲和关系后写入 Redis
- 读时：先查 Redis，miss 再本地路由
- 优先级最低——亲和不一致只是"路由质量略降"，不影响正确性

---

## Feature Flag 策略

```toml
# prism-router/Cargo.toml
[features]
default = []
wasm = ["dep:wasmtime"]
redis = ["dep:redis"]

[dependencies]
redis = { version = "0.27", features = [
    "tokio-comp",
    "connection-manager",
    "cluster-async",       # Redis Cluster
    "sentinel",            # Redis Sentinel
], optional = true }
```

编译矩阵：

| 命令 | 产物 |
|---|---|
| `cargo build` | 纯单机，与当前一致 |
| `cargo build --features redis` | 支持 Redis backend |
| `cargo build --features redis,wasm` | 完整功能 |

---

## 全局配置汇总

```yaml
# prism.yaml — 完整分布式配置示例
server:
  instance_id: "prism-1"   # 不设则自动生成 UUID
  listen: "0.0.0.0:8080"
  http_port: 8080
  grpc_port: 9090
  proxy_port: 8000

# 共享 Redis 连接（各子系统可复用或独立配置）
redis:
  url: "redis://redis:6379"
  password: ""
  database: 0
  pool_size: 10
  connect_timeout_ms: 1000
  key_prefix: "prism"

# 限流
rate_limit_policy:
  backend: redis           # local | redis
  degradation_policy: local  # allow | deny | local
  # 可选独立 redis 配置，不设则用全局 redis
  # redis: { url: "redis://other:6379" }

# 指标
metrics:
  backend: redis           # local | redis
  flush_interval_secs: 5
  cache_ttl_secs: 2

# 配置同步
config_sync:
  backend: redis           # none | redis
  channel: "prism:config:reload"

# ... 现有配置（providers, models, variants, routing 等）
```

**不设 `redis` 和各 `backend` 字段时，所有行为与当前完全一致。**

---

## 降级矩阵

| 场景 | 限流 | 指标 | 配置同步 |
|---|---|---|---|
| 全部 local（默认） | 进程内 DashMap | 进程内 HashMap | 本地文件监听 |
| Redis 正常 | 全局精确计数 | 跨实例聚合视图 | Pub/Sub 实时同步 |
| Redis 断连 | 自动回退 local | 自动回退 local | 只监听本地文件 |
| Redis 恢复 | 自动重连，计数重置 | 自动恢复聚合 | 自动重新订阅 |
| Redis 慢（>100ms） | 异步降级到 local | 读缓存兜底 | 无影响（Pub/Sub 异步） |

**核心原则：Redis 是增强层，不是必要层。**

---

## 部署示例

### Docker Compose（最小生产）

```yaml
services:
  redis:
    image: redis:7-alpine
    command: redis-server --appendonly yes --maxmemory 256mb --maxmemory-policy allkeys-lru
    volumes:
      - redis-data:/data
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 5s

  prism-1:
    image: prism:latest
    environment:
      PRISM_INSTANCE_ID: prism-1
      PRISM_REDIS_URL: redis://redis:6379
      PRISM_RATE_LIMIT_BACKEND: redis
      PRISM_METRICS_BACKEND: redis
      PRISM_CONFIG_SYNC_BACKEND: redis
    volumes:
      - ./configs:/etc/prism
    ports:
      - "8001:8000"
    depends_on:
      redis:
        condition: service_healthy

  prism-2:
    image: prism:latest
    environment:
      PRISM_INSTANCE_ID: prism-2
      PRISM_REDIS_URL: redis://redis:6379
      PRISM_RATE_LIMIT_BACKEND: redis
      PRISM_METRICS_BACKEND: redis
      PRISM_CONFIG_SYNC_BACKEND: redis
    volumes:
      - ./configs:/etc/prism
    ports:
      - "8002:8000"
    depends_on:
      redis:
        condition: service_healthy

  nginx:
    image: nginx:alpine
    volumes:
      - ./nginx.conf:/etc/nginx/nginx.conf
    ports:
      - "8000:8000"
    depends_on:
      - prism-1
      - prism-2

volumes:
  redis-data:
```

### Kubernetes

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: prism
spec:
  replicas: 3
  template:
    spec:
      containers:
        - name: prism
          image: prism:latest
          env:
            - name: PRISM_INSTANCE_ID
              valueFrom:
                fieldRef:
                  fieldPath: metadata.name
            - name: PRISM_REDIS_URL
              valueFrom:
                secretKeyRef:
                  name: prism-secrets
                  key: redis-url
            - name: PRISM_RATE_LIMIT_BACKEND
              value: redis
            - name: PRISM_METRICS_BACKEND
              value: redis
            - name: PRISM_CONFIG_SYNC_BACKEND
              value: redis
          volumeMounts:
            - name: config
              mountPath: /etc/prism
          resources:
            requests:
              cpu: 200m
              memory: 128Mi
            limits:
              cpu: "1"
              memory: 512Mi
      volumes:
        - name: config
          configMap:
            name: prism-config
---
apiVersion: v1
kind: Service
metadata:
  name: prism
spec:
  type: ClusterIP
  ports:
    - name: proxy
      port: 8000
    - name: http
      port: 8080
    - name: grpc
      port: 9090
  selector:
    app: prism
```

---

## 实施优先级 & 工作量

| Phase | 内容 | 优先级 | 工作量 | 依赖 |
|---|---|---|---|---|
| **P1** | 限流 Backend 抽象 + Redis 实现 | 🔴 必须 | 3-4 天 | redis crate |
| **P2** | 分布式指标聚合 | 🟡 重要 | 3-4 天 | P1（共享连接池） |
| **P3** | 配置 Pub/Sub 同步 | 🟢 有用 | 1-2 天 | P1 |
| **P4** | Affinity 一致性 | ⚪ 可选 | 1 天 | P2 |

**总计**：约 8-11 天，可按 Phase 独立交付。每个 Phase 完成即可独立上线。

---

## 轻量替代方案（无 Redis）

如果部署规模小（2-3 实例），可以不引入 Redis：

1. **限流**：LB 层粗粒度限流 + Prism 限流设为 `总限额 / N`
2. **指标**：各实例独立观察，路由效果略降但可接受
3. **配置**：K8s ConfigMap + rolling restart / 各实例独立 file watch

零外部依赖，适合 PoC 和小规模部署。

---

## 后续扩展（不在 MVP 范围）

- **Memcached Backend**：实现 `RateLimitBackend` for memcached（更高吞吐，但无 Lua 脚本）
- **External gRPC Backend**：Envoy 风格，限流逻辑完全外置（最灵活，但需要独立部署限流服务）
- **Prometheus 指标导出**：`/metrics` 端点，接入现有监控体系
- **etcd ConfigSource**：替代 Redis Pub/Sub，强一致性配置同步
