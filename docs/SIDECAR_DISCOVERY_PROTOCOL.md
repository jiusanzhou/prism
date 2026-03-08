# Prism Sidecar Discovery Protocol

Prism supports dynamic service discovery via external sidecar processes.
Any sidecar implementing this protocol can register models, variants, and pools
into Prism's routing registry automatically.

## Overview

```
┌─────────┐   SSE /events    ┌─────────────┐   K8s API / Consul / DB
│  Prism   │◄────────────────│   Sidecar    │◄────────────────────────
│  Router  │   HTTP /prism   │  (Go/Python/ │   (any data source)
│          │────────────────►│   any lang)  │
└─────────┘                  └─────────────┘
```

Prism connects to the sidecar, not the other way around. The sidecar is the
authoritative source of truth for the instances it manages.

## Endpoints

A conforming sidecar MUST implement these HTTP endpoints:

### `GET /events` — Server-Sent Events (real-time)

Primary discovery channel. Prism connects on startup and maintains a persistent
SSE connection. The sidecar pushes events as instance state changes.

**Response**: `Content-Type: text/event-stream`

#### Event Types

**`full_sync`** — Complete state snapshot.

Sent on initial connection and whenever the sidecar wants to reset state.
If `models` is empty, Prism ignores the event (treats as "not ready yet").

```
event: full_sync
data: {"type":"full_sync","models":{"model-a":[<instance>,...],"model-b":[...]}}
```

**`instance_add`** — A new instance became available.

```
event: instance_add
data: {"type":"instance_add","model":"model-a","instance":{"address":"10.0.1.5:8000","pod_name":"pod-xyz","labels":{...}}}
```

**`instance_del`** — An instance was removed (pod terminating, health check failed, etc).

```
event: instance_del
data: {"type":"instance_del","model":"model-a","instance":{"address":"10.0.1.5:8000","pod_name":"pod-xyz"}}
```

**`model_remove`** — All instances of a model are gone.

```
event: model_remove
data: {"type":"model_remove","model":"model-a"}
```

#### Keepalive

The sidecar SHOULD send SSE comment lines (`: keepalive\n\n`) every 15 seconds
to prevent proxy/load-balancer timeouts.

### `GET /prism` — Full State (HTTP poll)

Returns the complete discovery state in Prism-native format. Used for:
- Initial sync when SSE connects (SSE initial full_sync may be empty)
- Fallback when SSE is unavailable
- Incremental event handling (Prism re-fetches full state on `instance_add`/`instance_del`)

**Response**: `Content-Type: application/json`

```json
{
  "providers": [
    {"id": "k8s-staging", "base_url": ""}
  ],
  "models": [
    {"id": "qwen3-0.6b", "display_name": "Qwen3 0.6B"}
  ],
  "variants": [
    {
      "id": "qwen3-0.6b-pod-abc",
      "model_id": "qwen3-0.6b",
      "provider_id": "k8s-staging",
      "weight": 1.0,
      "base_url": "http://10.0.1.5:8000",
      "metadata": {
        "wanqing.inference.instance/role": "prefill",
        "wanqing.inference.instance.pd/group": "group-1"
      }
    }
  ],
  "pools": [
    {"id": "pool-qwen3-0.6b", "name": "qwen3-0.6b", "variants": ["qwen3-0.6b-pod-abc"]}
  ]
}
```

### `GET /discover` — Raw Instance Data (optional)

Human-friendly endpoint showing raw discovered instances grouped by model.
Not consumed by Prism directly — useful for debugging and monitoring.

```json
{
  "models": {
    "qwen3-0.6b": [
      {
        "address": "10.0.1.5:8000",
        "protocol": "http",
        "pod_name": "pod-abc",
        "labels": {"modelServiceName": "qwen3-0.6b", ...},
        "dc": "us-east"
      }
    ]
  },
  "updated_at": 1772890680000
}
```

### `GET /health` — Health Check (optional)

```json
{"status": "ok", "updated_at": 1772890680000}
```

`status` is `"stale"` if the sidecar hasn't received updates in >60 seconds.

## Instance Schema

Each instance in SSE events and `/discover` responses:

| Field      | Type              | Required | Description |
|------------|-------------------|----------|-------------|
| `address`  | `string`          | Yes      | `ip:port` of the inference endpoint |
| `protocol` | `string`          | No       | `"http"` (default) or `"https"` |
| `pod_name` | `string`          | No       | Kubernetes pod name (used in variant ID) |
| `labels`   | `map<string,string>` | No    | Arbitrary labels (see below) |
| `dc`       | `string`          | No       | Data center identifier |

### Well-Known Labels

These labels are recognized by Prism for special routing behavior:

| Label | Description | Values |
|-------|-------------|--------|
| `wanqing.inference.instance/role` | P/D disaggregation role | `prefill`, `decode` |
| `wanqing.inference.instance.pd/group` | P/D group identifier | Any string |
| `apps.kwai.io/duplicateset-instance-id` | Instance replica ID | `"0"`, `"1"`, ... |
| `modelServiceName` | Model name | Any string |
| `cluster` | Source cluster name | Any string |

When `role` is `prefill` and `decode` instances share the same `pd/group`,
Prism performs P/D split routing: fire-and-forget to prefill, upstream proxy to decode.

## Prism Configuration

```yaml
providers:
  my-k8s-cluster:
    type: sidecar
    discovery:
      sidecar_url: "http://localhost:9091"    # Required
      cluster_name: "staging"                  # Optional, for logging
      enable_pd_filter: true                   # Optional, sidecar-side P/D filtering
      poll_interval_secs: 30                   # Optional, fallback poll interval
```

Multiple providers can point to different sidecars:

```yaml
providers:
  staging:
    type: sidecar
    discovery:
      sidecar_url: "http://sidecar-staging:9091"
  production:
    type: sidecar
    discovery:
      sidecar_url: "http://sidecar-prod:9091"
  cloud-apis:
    type: openai
    base_url: "https://api.openai.com/v1"
    api_key: ${OPENAI_API_KEY}
```

## Connection Lifecycle

1. Prism starts, scans providers for `type: sidecar` with `discovery` config
2. For each: spawns async task → connects SSE `/events`
3. On SSE connect: immediately fetches `/prism` for initial state
4. SSE events update registry in real-time
5. On SSE disconnect: retry with 5s backoff, HTTP poll as fallback
6. Empty `full_sync` events are ignored (sidecar may not be ready)

## Implementing a Sidecar

A minimal sidecar needs:
1. A data source (K8s informer, Consul watch, database poll, etc.)
2. In-memory model→instances map
3. HTTP server with `/events` (SSE) and `/prism` (JSON)
4. Push SSE events on state changes

See `prism-plugins/plugins/k8s-discovery/` for the reference Go implementation
using Kubernetes Pod informers.

### Example: Minimal Python Sidecar

```python
from http.server import HTTPServer, BaseHTTPRequestHandler
import json, threading, time

instances = {}  # model_name → [{"address": "ip:port", ...}]

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/prism":
            payload = build_prism_payload(instances)
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(payload).encode())
        elif self.path == "/events":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            # Send initial full_sync
            data = json.dumps({"type": "full_sync", "models": instances})
            self.wfile.write(f"event: full_sync\ndata: {data}\n\n".encode())
            self.wfile.flush()
            # Keep alive
            while True:
                time.sleep(15)
                self.wfile.write(b": keepalive\n\n")
                self.wfile.flush()
```
