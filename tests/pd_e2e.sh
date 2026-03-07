#!/bin/bash
set -e

DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$DIR"

cleanup() {
    kill $PID_P $PID_D $PID_SSE $PID_PRISM 2>/dev/null || true
}
trap cleanup EXIT

echo "=== Building ==="
cargo build --release -p prism-router 2>&1 | tail -1
cd examples/mock-llm-server && cargo build --release 2>&1 | tail -1 && cd ../..

MOCK="./examples/mock-llm-server/target/release/mock-llm-server"
PRISM="./target/release/prism-router"

echo "=== Starting P mock :18081, D mock :18082 ==="
$MOCK --port 18081 --host 127.0.0.1 &
PID_P=$!
$MOCK --port 18082 --host 127.0.0.1 &
PID_D=$!
sleep 1

echo "=== Starting mock SSE sidecar :19091 ==="
python3 << 'PYEOF' &
import http.server, json, time
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/events":
            self.send_response(200)
            self.send_header("Content-Type","text/event-stream")
            self.send_header("Cache-Control","no-cache")
            self.end_headers()
            snap = {"type":"full_sync","timestamp":int(time.time()*1000),"snapshot":{"test-model":[
                {"address":"127.0.0.1:18081","protocol":"http","pod_name":"prefill-0","labels":{"modelServiceName":"test-model","wanqing.inference.instance/role":"prefill","wanqing.inference.instance.pd/group":"g1","apps.kwai.io/duplicateset-instance-id":"0","cluster":"local"}},
                {"address":"127.0.0.1:18082","protocol":"http","pod_name":"decode-0","labels":{"modelServiceName":"test-model","wanqing.inference.instance/role":"decode","wanqing.inference.instance.pd/group":"g1","apps.kwai.io/duplicateset-instance-id":"0","cluster":"local"}}
            ]}}
            self.wfile.write(("data: "+json.dumps(snap)+"\n\n").encode())
            self.wfile.flush()
            while True:
                time.sleep(30)
                try: self.wfile.write(b": keepalive\n\n"); self.wfile.flush()
                except: break
        elif self.path == "/health":
            self.send_response(200); self.end_headers(); self.wfile.write(b"ok")
        else:
            self.send_response(404); self.end_headers()
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1",19091),H).serve_forever()
PYEOF
PID_SSE=$!
sleep 1

echo "=== Writing Prism config ==="
cat > /tmp/prism-pd-test.yaml << 'YAML'
server:
  http_port: 19080
  grpc_port: 19090
  proxy_port: 19000

providers:
  k8s-local:
    type: openai
    base_url: "http://127.0.0.1:18082"
    protocols: [openai]

models:
  - id: test-model
    display_name: test-model
    quality_tier: B

variants:
  - id: test-model-prefill-0
    model_id: test-model
    provider_id: k8s-local
    weight: 1.0
    base_url: "http://127.0.0.1:18081"
    metadata:
      "wanqing.inference.instance/role": "prefill"
      "wanqing.inference.instance.pd/group": "g1"
      "apps.kwai.io/duplicateset-instance-id": "0"
  - id: test-model-decode-0
    model_id: test-model
    provider_id: k8s-local
    weight: 1.0
    base_url: "http://127.0.0.1:18082"
    metadata:
      "wanqing.inference.instance/role": "decode"
      "wanqing.inference.instance.pd/group": "g1"
      "apps.kwai.io/duplicateset-instance-id": "0"

pools:
  - id: pool-test-model
    name: test-model
    variants: [test-model-prefill-0, test-model-decode-0]

routing:
  strategy: lowest_latency
YAML

echo "=== Starting Prism ==="
RUST_LOG=info $PRISM --config /tmp/prism-pd-test.yaml > /tmp/prism-pd.log 2>&1 &
PID_PRISM=$!
sleep 3

echo "=== Sending request ==="
RESP=$(curl -s -w "\n%{http_code}" -X POST http://127.0.0.1:19000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"test-model","messages":[{"role":"user","content":"Hello"}]}')

CODE=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')

echo "HTTP: $CODE"
echo "Body: $(echo $BODY | head -c 200)"

echo ""
echo "=== P/D logs ==="
grep -i "P/D\|prefill\|decode\|pd_decision\|Split\|fire" /tmp/prism-pd.log 2>/dev/null | tail -10 || echo "(no PD logs)"

if [ "$CODE" = "200" ]; then
    echo ""
    echo "✅ P/D E2E PASSED"
else
    echo ""
    echo "❌ FAILED (HTTP $CODE)"
    tail -30 /tmp/prism-pd.log
    exit 1
fi
