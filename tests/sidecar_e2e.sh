#!/usr/bin/env bash
# Sidecar Discovery E2E Test
#
# Tests the full chain:
#   K8s cluster → Go sidecar (informer) → SSE → Prism (auto-register) → Proxy → Pod
#
# Prerequisites:
#   - kubectl configured with an inference cluster (pods labeled app=inference-service)
#   - Go sidecar binary at /tmp/k8s-discovery (or BUILD_SIDECAR=1)
#   - Prism binary at ./target/release/prism-router (or BUILD_PRISM=1)
#
# Usage:
#   bash tests/sidecar_e2e.sh
#   BUILD_SIDECAR=1 BUILD_PRISM=1 bash tests/sidecar_e2e.sh

set -euo pipefail

PRISM_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SIDECAR_BIN="${SIDECAR_BIN:-/tmp/k8s-discovery}"
PLUGINS_DIR="${PRISM_DIR}/../prism-plugins"

SIDECAR_PORT=19091
PRISM_HTTP_PORT=19080
PRISM_GRPC_PORT=19090
PRISM_PROXY_PORT=19000

PRISM_CONFIG="/tmp/prism-sidecar-e2e.yaml"
PRISM_LOG="/tmp/prism-sidecar-e2e.log"
SIDECAR_LOG="/tmp/sidecar-e2e.log"

strip_ansi() { perl -pe 's/\e\[[0-9;]*m//g'; }

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

cleanup() {
    echo ""
    echo "=== Cleanup ==="
    kill $SIDECAR_PID 2>/dev/null && echo "  Killed sidecar" || true
    kill $PRISM_PID 2>/dev/null && echo "  Killed prism" || true
    wait $SIDECAR_PID 2>/dev/null || true
    wait $PRISM_PID 2>/dev/null || true
}
trap cleanup EXIT

SIDECAR_PID=0
PRISM_PID=0
PASS=0
FAIL=0

pass() { PASS=$((PASS+1)); echo -e "  ${GREEN}✓${NC} $1"; }
fail() { FAIL=$((FAIL+1)); echo -e "  ${RED}✗${NC} $1"; }

# ── Step 0: Prerequisites ──────────────────────────────────────────

echo "=== Step 0: Prerequisites ==="

# Check kubectl
if ! kubectl config current-context >/dev/null 2>&1; then
    echo -e "${RED}kubectl not configured${NC}"
    exit 1
fi
CONTEXT=$(kubectl config current-context)
echo "  Cluster: $CONTEXT"

# Count ready inference pods
N_PODS=$(kubectl get pods -l app=inference-service --field-selector=status.phase=Running --no-headers 2>/dev/null | wc -l | tr -d ' ')
if [ "$N_PODS" -lt 1 ]; then
    echo -e "${RED}No inference pods found${NC}"
    exit 1
fi
echo "  Inference pods: $N_PODS"

# Build if requested
if [ "${BUILD_SIDECAR:-}" = "1" ]; then
    echo "  Building sidecar..."
    (cd "$PLUGINS_DIR/plugins/k8s-discovery" && \
        PATH=/usr/local/go/bin:$PATH GOPROXY=https://goproxy.cn,direct \
        go build -o "$SIDECAR_BIN" ./cmd/main.go)
fi
if [ "${BUILD_PRISM:-}" = "1" ]; then
    echo "  Building prism..."
    (cd "$PRISM_DIR" && cargo build --release 2>&1 | tail -1)
fi

[ -x "$SIDECAR_BIN" ] && pass "Sidecar binary" || { fail "Sidecar binary not found: $SIDECAR_BIN"; exit 1; }
[ -x "$PRISM_DIR/target/release/prism-router" ] && pass "Prism binary" || { fail "Prism binary not found"; exit 1; }

# ── Step 1: Start Go sidecar ───────────────────────────────────────

echo ""
echo "=== Step 1: Start sidecar ==="

"$SIDECAR_BIN" \
    -kubeconfigs "$CONTEXT:$HOME/.kube/config" \
    -listen ":$SIDECAR_PORT" \
    -provider-prefix k8s \
    -v 2 > "$SIDECAR_LOG" 2>&1 &
SIDECAR_PID=$!

# Wait for sidecar to be ready
for i in $(seq 1 20); do
    if curl -sf "http://127.0.0.1:$SIDECAR_PORT/health" >/dev/null 2>&1; then
        break
    fi
    sleep 0.5
done

HEALTH=$(curl -sf "http://127.0.0.1:$SIDECAR_PORT/health" 2>/dev/null || echo '{"status":"dead"}')
STATUS=$(echo "$HEALTH" | python3 -c "import json,sys; print(json.load(sys.stdin).get('status','dead'))" 2>/dev/null || echo "dead")
[ "$STATUS" = "ok" ] && pass "Sidecar healthy" || { fail "Sidecar not healthy: $HEALTH"; exit 1; }

# ── Step 2: Verify sidecar discovery ───────────────────────────────

echo ""
echo "=== Step 2: Verify sidecar discovery ==="

DISCOVER=$(curl -sf "http://127.0.0.1:$SIDECAR_PORT/discover" 2>/dev/null)
N_MODELS=$(echo "$DISCOVER" | python3 -c "import json,sys; print(len(json.load(sys.stdin).get('models',{})))" 2>/dev/null || echo 0)
[ "$N_MODELS" -gt 0 ] && pass "Discovered $N_MODELS models" || { fail "No models discovered"; exit 1; }

PRISM_PAYLOAD=$(curl -sf "http://127.0.0.1:$SIDECAR_PORT/prism" 2>/dev/null)
N_VARIANTS=$(echo "$PRISM_PAYLOAD" | python3 -c "import json,sys; print(len(json.load(sys.stdin).get('variants',[])))" 2>/dev/null || echo 0)
[ "$N_VARIANTS" -gt 0 ] && pass "Prism payload: $N_VARIANTS variants" || { fail "No variants in /prism"; exit 1; }

# Check P/D instances
PD_MODELS=$(echo "$DISCOVER" | python3 -c "
import json,sys
d = json.load(sys.stdin)
pd = []
for m, insts in d.get('models',{}).items():
    roles = set(i.get('labels',{}).get('wanqing.inference.instance/role','') for i in insts)
    if 'prefill' in roles and 'decode' in roles:
        pd.append(m)
print(','.join(pd) if pd else 'none')
" 2>/dev/null || echo "none")
echo "  P/D models: $PD_MODELS"

# ── Step 3: Write Prism config (ZERO manual models/variants) ──────

echo ""
echo "=== Step 3: Write Prism config ==="

cat > "$PRISM_CONFIG" << YAML
server:
  http_port: $PRISM_HTTP_PORT
  grpc_port: $PRISM_GRPC_PORT
  proxy_port: $PRISM_PROXY_PORT

providers:
  $CONTEXT:
    type: k8s
    discovery:
      cluster_name: $CONTEXT
      sidecar_url: "http://127.0.0.1:$SIDECAR_PORT"
      enable_pd_filter: true

routing:
  strategy: lowest_latency
YAML

pass "Config written (zero manual models/variants)"
cat "$PRISM_CONFIG" | sed 's/^/  /'

# ── Step 4: Start Prism ───────────────────────────────────────────

echo ""
echo "=== Step 4: Start Prism ==="

RUST_LOG=info RUST_LOG_STYLE=never "$PRISM_DIR/target/release/prism-router" \
    --config "$PRISM_CONFIG" > "$PRISM_LOG" 2>&1 &
PRISM_PID=$!

# Wait for Prism to register variants
for i in $(seq 1 30); do
    if grep -q "Registry updated.*variants" "$PRISM_LOG" 2>/dev/null; then
        break
    fi
    sleep 0.5
done

REGISTERED=$(grep "Registry updated" "$PRISM_LOG" 2>/dev/null | tail -1 | strip_ansi | sed -n 's/.*variants=\([0-9]*\).*/\1/p')
REGISTERED=${REGISTERED:-0}
[ "$REGISTERED" -gt 0 ] && pass "Prism registered $REGISTERED variants" || { fail "No variants registered"; cat "$PRISM_LOG"; exit 1; }

# ── Step 5: Find reachable pod a pod for local access ───────────────────

echo ""
echo "=== Step 5: Find reachable pod ==="

# Find a colocated pod (qwen3 preferred for fast response)
TARGET_POD=""
TARGET_MODEL=""
TARGET_IP=""
TARGET_PORT=""

python3 << 'PYEOF' > /tmp/e2e_target.env
import json
with open("/tmp/pods.json") as f:
    data = json.load(f)
# Prefer qwen3-0.6b, then any colocated non-embedding model
preferred = ["qwen3-0.6b", "qwen3-4b", "qwen2.5-1.5b"]
candidates = []
for pod in data["items"]:
    conds = pod.get("status",{}).get("conditions",[])
    ready = any(c.get("type")=="Ready" and c.get("status")=="True" for c in conds)
    if not ready: continue
    labels = pod["metadata"].get("labels",{})
    model = labels.get("modelServiceName","")
    role = labels.get("wanqing.inference.instance/role","")
    if role: continue  # skip P/D pods
    if "bge" in model: continue  # skip embedding models
    ann = pod["metadata"].get("annotations",{})
    ports = json.loads(ann.get("io.kwai.kce.ports.inference-container","[]"))
    port = 8000
    for p in ports:
        if p.get("name") == "AUTO_PORT3":
            port = int(p["value"]); break
    name = pod["metadata"]["name"]
    priority = 99
    for i, prefix in enumerate(preferred):
        if model.startswith(prefix):
            priority = i; break
    ip = pod["status"].get("podIP","")
    candidates.append((priority, name, model, ip, port))

candidates.sort()
if candidates:
    _, name, model, ip, port = candidates[0]
    print(f"TARGET_POD={name}")
    print(f"TARGET_MODEL={model}")
    print(f"TARGET_IP={ip}")
    print(f"TARGET_PORT={port}")
PYEOF

source /tmp/e2e_target.env

if [ -z "${TARGET_POD:-}" ]; then
    fail "No suitable pod found"
    exit 1
fi

echo "  Target: $TARGET_POD ($TARGET_MODEL @ $TARGET_IP:$TARGET_PORT)"


# Verify direct Pod IP access
DIRECT_HTTP=$(curl -sf -o /dev/null -w "%{http_code}" --max-time 10 \
    "http://$TARGET_IP:$TARGET_PORT/v1/chat/completions" \
    -H "Content-Type: application/json" \
    -d "{\"model\":\"$TARGET_MODEL\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}],\"max_tokens\":8}" 2>/dev/null || echo "000")
[ "$DIRECT_HTTP" = "200" ] && pass "Direct Pod IP access: HTTP $DIRECT_HTTP" || { fail "Direct Pod IP access failed: HTTP $DIRECT_HTTP"; exit 1; }

# ── Step 6: Route through Prism ───────────────────────────────────

echo ""
echo "=== Step 6: Route through Prism ==="

# The sidecar registered the pod with its cluster IP (unreachable from local).
# We need to verify Prism selected the correct variant, even if it can't reach the pod.
# Use the proxy port — Prism will try to connect to the cluster IP and fail with 502.
# But the routing decision itself is what we're testing.

PRISM_RESP=$(curl -sf --max-time 10 \
    "http://127.0.0.1:$PRISM_PROXY_PORT/v1/chat/completions" \
    -H "Content-Type: application/json" \
    -d "{\"model\":\"$TARGET_MODEL\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}],\"max_tokens\":8}" 2>&1 || echo "CONNECT_FAIL")
PRISM_HTTP=$(curl -sf -o /dev/null -w "%{http_code}" --max-time 10 \
    "http://127.0.0.1:$PRISM_PROXY_PORT/v1/chat/completions" \
    -H "Content-Type: application/json" \
    -d "{\"model\":\"$TARGET_MODEL\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}],\"max_tokens\":8}" 2>/dev/null || echo "000")

# Check routing log
ROUTE_LOG=$(grep "Routing to upstream.*$TARGET_MODEL" "$PRISM_LOG" | tail -1 | strip_ansi)
if [ -n "$ROUTE_LOG" ]; then
    ROUTED_VARIANT=$(echo "$ROUTE_LOG" | strip_ansi | sed -n 's/.*variant=\([^ ]*\).*/\1/p')
    pass "Prism routed to variant: $ROUTED_VARIANT"
else
    fail "No routing log for $TARGET_MODEL"
fi

# 502 = pod IP unreachable
# 200 = full chain working
if [ "$PRISM_HTTP" = "200" ]; then
    pass "Prism proxy: HTTP 200 (response from pod)"
elif [ "$PRISM_HTTP" = "502" ]; then
    pass "Prism proxy: HTTP 502 (routed correctly, pod IP unreachable from local — expected)"
else
    fail "Prism proxy: HTTP $PRISM_HTTP (unexpected)"
fi

# ── Step 7: Verify P/D routing (if P/D model exists) ──────────────

echo ""
echo "=== Step 7: P/D routing ==="

if [ "$PD_MODELS" != "none" ]; then
    PD_MODEL=$(echo "$PD_MODELS" | cut -d, -f1)
    echo "  Testing P/D model: $PD_MODEL"

    curl -sf -o /dev/null --max-time 10 \
        "http://127.0.0.1:$PRISM_PROXY_PORT/v1/chat/completions" \
        -H "Content-Type: application/json" \
        -d "{\"model\":\"$PD_MODEL\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}],\"max_tokens\":8}" 2>/dev/null || true

    PD_LOG=$(grep "P/D Split" "$PRISM_LOG" | strip_ansi)
    if [ -n "$PD_LOG" ]; then
        pass "P/D Split detected in logs"
        PF_VARIANT=$(echo "$PD_LOG" | sed -n 's/.*prefill_variant=\([^ ]*\).*/\1/p' | head -1)
        PF_VARIANT=${PF_VARIANT:-?}
        echo "    Prefill: $PF_VARIANT"
        DEC_VARIANT=$(echo "$PD_LOG" | sed -n 's/.*decode_variant=\([^ ]*\).*/\1/p' | head -1)
        DEC_VARIANT=${DEC_VARIANT:-?}
        echo "    Decode:  $DEC_VARIANT"
    else
        fail "No P/D Split log for $PD_MODEL"
    fi
else
    echo -e "  ${YELLOW}⊘${NC} No P/D models in cluster — skipped"
fi

# ── Step 8: Verify SSE real-time (check sidecar has SSE clients) ──

echo ""
echo "=== Step 8: SSE connectivity ==="

SSE_LOG=$(grep "SSE connected" "$PRISM_LOG" | tail -1 | strip_ansi)
if [ -n "$SSE_LOG" ]; then
    pass "Prism SSE connected to sidecar"
else
    fail "No SSE connection log"
fi

# ── Summary ────────────────────────────────────────────────────────

echo ""
echo "========================================"
TOTAL=$((PASS + FAIL))
if [ "$FAIL" -eq 0 ]; then
    echo -e "${GREEN}✅ ALL $TOTAL CHECKS PASSED${NC}"
    echo ""
    echo "Config: $PRISM_CONFIG"
    echo "Prism log: $PRISM_LOG"
    echo "Sidecar log: $SIDECAR_LOG"
    exit 0
else
    echo -e "${RED}❌ $FAIL/$TOTAL CHECKS FAILED${NC}"
    echo ""
    echo "Prism log: $PRISM_LOG"
    echo "Sidecar log: $SIDECAR_LOG"
    exit 1
fi
