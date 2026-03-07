import { useEffect, useState } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from "@/components/ui/select"
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, DialogDescription } from "@/components/ui/dialog"
import { Plus, Trash2, Save, Loader2 } from "lucide-react"

// ── Type definitions ──

interface BalancedConfig {
  cost_weight: number
  quality_weight: number
  latency_weight: number
}

interface ModelStrategyOverride {
  model_id: string
  strategy: string
  pool?: string | null
}

interface RetryConfig {
  max_retries: number
  retry_on: string[]
  backoff: string
  retry_scope: string
}

interface CircuitBreakerConfig {
  error_threshold_pct: number
  window_seconds: number
  open_duration_seconds: number
}

interface TimeoutConfig {
  connect_ms: number
  request_ms: number
  ttft_ms: number
}

interface ResilienceConfig {
  retry: RetryConfig
  circuit_breaker: CircuitBreakerConfig
  timeout: TimeoutConfig
}

interface RoutingConfig {
  default_strategy: string
  balanced?: BalancedConfig | null
  l2_selector: string
  l3_selector: string
  l4_selector: string
  model_overrides: ModelStrategyOverride[]
  resilience: ResilienceConfig
}

// ── Defaults ──

const DEFAULT_ROUTING: RoutingConfig = {
  default_strategy: "balanced",
  balanced: { cost_weight: 0.3, quality_weight: 0.5, latency_weight: 0.2 },
  l2_selector: "default",
  l3_selector: "affinity",
  l4_selector: "best",
  model_overrides: [],
  resilience: {
    retry: { max_retries: 2, retry_on: ["RateLimit", "Timeout"], backoff: "Exponential", retry_scope: "CrossProvider" },
    circuit_breaker: { error_threshold_pct: 50, window_seconds: 60, open_duration_seconds: 30 },
    timeout: { connect_ms: 3000, request_ms: 30000, ttft_ms: 10000 },
  },
}

const STRATEGIES = ["balanced", "cost_first", "quality_first", "latency_first"]
const L2_OPTIONS = ["default", "lowest_latency", "lowest_cost", "priority"]
const L3_OPTIONS = ["affinity", "round_robin", "least_load"]
const L4_OPTIONS = ["best", "weighted"]
const RETRY_CONDITIONS = ["RateLimit", "Timeout", "ConnectionError", "HttpStatus"]
const BACKOFF_OPTIONS = ["Fixed", "Exponential", "Jittered"]
const RETRY_SCOPE_OPTIONS = ["SameVariant", "SameProvider", "CrossProvider"]

// ── Component ──

export default function Routing() {
  const [form, setForm] = useState<RoutingConfig>(DEFAULT_ROUTING)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)

  // Override dialog
  const [overrideOpen, setOverrideOpen] = useState(false)
  const [overrideForm, setOverrideForm] = useState<ModelStrategyOverride>({ model_id: "", strategy: "balanced", pool: "" })

  useEffect(() => {
    fetch("/api/routing")
      .then(r => r.json())
      .then(d => {
        if (d.routing) {
          const r = d.routing
          setForm({
            default_strategy: r.default_strategy || "balanced",
            balanced: r.balanced ?? { cost_weight: 0.3, quality_weight: 0.5, latency_weight: 0.2 },
            l2_selector: r.l2_selector || "default",
            l3_selector: r.l3_selector || "affinity",
            l4_selector: r.l4_selector || "best",
            model_overrides: r.model_overrides || [],
            resilience: {
              retry: {
                max_retries: r.resilience?.retry?.max_retries ?? 2,
                retry_on: r.resilience?.retry?.retry_on ?? ["RateLimit", "Timeout"],
                backoff: r.resilience?.retry?.backoff || "Exponential",
                retry_scope: r.resilience?.retry?.retry_scope || "CrossProvider",
              },
              circuit_breaker: {
                error_threshold_pct: r.resilience?.circuit_breaker?.error_threshold_pct ?? 50,
                window_seconds: r.resilience?.circuit_breaker?.window_seconds ?? 60,
                open_duration_seconds: r.resilience?.circuit_breaker?.open_duration_seconds ?? 30,
              },
              timeout: {
                connect_ms: r.resilience?.timeout?.connect_ms ?? 3000,
                request_ms: r.resilience?.timeout?.request_ms ?? 30000,
                ttft_ms: r.resilience?.timeout?.ttft_ms ?? 10000,
              },
            },
          })
        }
        setLoading(false)
      })
      .catch(() => setLoading(false))
  }, [])

  const handleSave = async () => {
    setSaving(true)
    try {
      await fetch("/api/routing", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(form),
      })
      setSaved(true)
      setTimeout(() => setSaved(false), 2000)
    } catch {
      // error handling could be added here
    }
    setSaving(false)
  }

  const toggleRetryOn = (condition: string) => {
    setForm(f => ({
      ...f,
      resilience: {
        ...f.resilience,
        retry: {
          ...f.resilience.retry,
          retry_on: f.resilience.retry.retry_on.includes(condition)
            ? f.resilience.retry.retry_on.filter(c => c !== condition)
            : [...f.resilience.retry.retry_on, condition],
        },
      },
    }))
  }

  const addOverride = () => {
    if (!overrideForm.model_id) return
    setForm(f => ({
      ...f,
      model_overrides: [...f.model_overrides, {
        model_id: overrideForm.model_id,
        strategy: overrideForm.strategy,
        pool: overrideForm.pool || null,
      }],
    }))
    setOverrideForm({ model_id: "", strategy: "balanced", pool: "" })
    setOverrideOpen(false)
  }

  const removeOverride = (index: number) => {
    setForm(f => ({
      ...f,
      model_overrides: f.model_overrides.filter((_, i) => i !== index),
    }))
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center py-12">
        <Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" />
      </div>
    )
  }

  return (
    <div>
      {/* Header */}
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <div>
          <h1 className="text-xl sm:text-2xl font-bold">Routing Configuration</h1>
          <p className="text-sm text-[hsl(var(--muted-foreground))]">Strategy, layer selectors, and resilience settings</p>
        </div>
        <Button size="sm" onClick={handleSave} disabled={saving}>
          <Save className="h-4 w-4 mr-1" /> {saving ? "Saving..." : saved ? "Saved!" : "Save Changes"}
        </Button>
      </div>

      <div className="max-w-3xl space-y-4">
        {/* ── Global Strategy ── */}
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Global Strategy</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div>
              <Label>Default Strategy</Label>
              <Select value={form.default_strategy} onValueChange={v => setForm(f => ({ ...f, default_strategy: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {STRATEGIES.map(s => <SelectItem key={s} value={s}>{s}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>

            {form.default_strategy === "balanced" && (
              <>
                <Separator />
                <div className="text-sm font-medium text-[hsl(var(--muted-foreground))]">Balanced Weights</div>
                <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
                  <div>
                    <Label>Cost Weight</Label>
                    <Input
                      type="number"
                      min={0} max={1} step={0.05}
                      value={form.balanced?.cost_weight ?? 0.3}
                      onChange={e => setForm(f => ({
                        ...f,
                        balanced: { ...f.balanced!, cost_weight: parseFloat(e.target.value) || 0 },
                      }))}
                    />
                  </div>
                  <div>
                    <Label>Quality Weight</Label>
                    <Input
                      type="number"
                      min={0} max={1} step={0.05}
                      value={form.balanced?.quality_weight ?? 0.5}
                      onChange={e => setForm(f => ({
                        ...f,
                        balanced: { ...f.balanced!, quality_weight: parseFloat(e.target.value) || 0 },
                      }))}
                    />
                  </div>
                  <div>
                    <Label>Latency Weight</Label>
                    <Input
                      type="number"
                      min={0} max={1} step={0.05}
                      value={form.balanced?.latency_weight ?? 0.2}
                      onChange={e => setForm(f => ({
                        ...f,
                        balanced: { ...f.balanced!, latency_weight: parseFloat(e.target.value) || 0 },
                      }))}
                    />
                  </div>
                </div>
              </>
            )}
          </CardContent>
        </Card>

        {/* ── Layer Configuration ── */}
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Layer Configuration</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div>
              <Label>L2 Provider Selection</Label>
              <Select value={form.l2_selector} onValueChange={v => setForm(f => ({ ...f, l2_selector: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {L2_OPTIONS.map(s => <SelectItem key={s} value={s}>{s}</SelectItem>)}
                </SelectContent>
              </Select>
              <p className="text-xs text-[hsl(var(--muted-foreground))] mt-1">How providers are ranked and selected</p>
            </div>
            <div>
              <Label>L3 Account Selection</Label>
              <Select value={form.l3_selector} onValueChange={v => setForm(f => ({ ...f, l3_selector: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {L3_OPTIONS.map(s => <SelectItem key={s} value={s}>{s}</SelectItem>)}
                </SelectContent>
              </Select>
              <p className="text-xs text-[hsl(var(--muted-foreground))] mt-1">How accounts within a provider are selected</p>
            </div>
            <div>
              <Label>L4 Variant Selection</Label>
              <Select value={form.l4_selector} onValueChange={v => setForm(f => ({ ...f, l4_selector: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {L4_OPTIONS.map(s => <SelectItem key={s} value={s}>{s}</SelectItem>)}
                </SelectContent>
              </Select>
              <p className="text-xs text-[hsl(var(--muted-foreground))] mt-1">How variants within an account are selected</p>
            </div>
          </CardContent>
        </Card>

        {/* ── Per-Model Strategy Overrides ── */}
        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <CardTitle className="text-base">Per-Model Strategy Overrides</CardTitle>
              <Button variant="ghost" size="sm" onClick={() => { setOverrideForm({ model_id: "", strategy: "balanced", pool: "" }); setOverrideOpen(true) }}>
                <Plus className="h-3.5 w-3.5 mr-1" /> Add Override
              </Button>
            </div>
          </CardHeader>
          <CardContent className="p-0">
            {form.model_overrides.length === 0 ? (
              <div className="text-center py-8 text-sm text-[hsl(var(--muted-foreground))]">
                No per-model overrides. All models use the global strategy.
              </div>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-t border-[hsl(var(--border))]">
                      <th className="text-left px-4 py-2 font-medium text-[hsl(var(--muted-foreground))]">Model ID</th>
                      <th className="text-left px-4 py-2 font-medium text-[hsl(var(--muted-foreground))]">Strategy</th>
                      <th className="text-left px-4 py-2 font-medium text-[hsl(var(--muted-foreground))]">Pool</th>
                      <th className="text-right px-4 py-2 font-medium text-[hsl(var(--muted-foreground))]">Actions</th>
                    </tr>
                  </thead>
                  <tbody>
                    {form.model_overrides.map((o, i) => (
                      <tr key={i} className="border-b border-[hsl(var(--border))] last:border-0">
                        <td className="px-4 py-2 font-mono text-xs">{o.model_id}</td>
                        <td className="px-4 py-2">
                          <Badge variant="secondary">{o.strategy}</Badge>
                        </td>
                        <td className="px-4 py-2 text-xs">{o.pool || <span className="text-[hsl(var(--muted-foreground))] italic">default</span>}</td>
                        <td className="px-4 py-2 text-right">
                          <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => removeOverride(i)}>
                            <Trash2 className="h-3.5 w-3.5 text-red-400" />
                          </Button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </CardContent>
        </Card>

        {/* ── Resilience Settings ── */}
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Resilience Settings</CardTitle>
          </CardHeader>
          <CardContent className="space-y-6">
            {/* Retry */}
            <div>
              <div className="text-sm font-medium mb-3">Retry</div>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                <div>
                  <Label>Max Retries</Label>
                  <Input
                    type="number"
                    min={0} max={10}
                    value={form.resilience.retry.max_retries}
                    onChange={e => setForm(f => ({
                      ...f,
                      resilience: {
                        ...f.resilience,
                        retry: { ...f.resilience.retry, max_retries: parseInt(e.target.value) || 0 },
                      },
                    }))}
                  />
                </div>
                <div>
                  <Label>Backoff</Label>
                  <Select
                    value={form.resilience.retry.backoff}
                    onValueChange={v => setForm(f => ({
                      ...f,
                      resilience: { ...f.resilience, retry: { ...f.resilience.retry, backoff: v } },
                    }))}
                  >
                    <SelectTrigger><SelectValue /></SelectTrigger>
                    <SelectContent>
                      {BACKOFF_OPTIONS.map(b => <SelectItem key={b} value={b}>{b}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <div>
                  <Label>Retry Scope</Label>
                  <Select
                    value={form.resilience.retry.retry_scope}
                    onValueChange={v => setForm(f => ({
                      ...f,
                      resilience: { ...f.resilience, retry: { ...f.resilience.retry, retry_scope: v } },
                    }))}
                  >
                    <SelectTrigger><SelectValue /></SelectTrigger>
                    <SelectContent>
                      {RETRY_SCOPE_OPTIONS.map(s => <SelectItem key={s} value={s}>{s}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
              </div>
              <div className="mt-3">
                <Label>Retry On</Label>
                <div className="flex flex-wrap gap-2 mt-1.5">
                  {RETRY_CONDITIONS.map(c => (
                    <button
                      key={c}
                      type="button"
                      onClick={() => toggleRetryOn(c)}
                      className={`text-xs px-3 py-1.5 rounded-md cursor-pointer transition-colors border ${
                        form.resilience.retry.retry_on.includes(c)
                          ? "bg-blue-600 text-white border-blue-600"
                          : "bg-[hsl(var(--secondary))] text-[hsl(var(--secondary-foreground))] border-[hsl(var(--border))] hover:bg-[hsl(var(--accent))]"
                      }`}
                    >
                      {c}
                    </button>
                  ))}
                </div>
              </div>
            </div>

            <Separator />

            {/* Circuit Breaker */}
            <div>
              <div className="text-sm font-medium mb-3">Circuit Breaker</div>
              <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
                <div>
                  <Label>Error Threshold %</Label>
                  <Input
                    type="number"
                    min={0} max={100} step={1}
                    value={form.resilience.circuit_breaker.error_threshold_pct}
                    onChange={e => setForm(f => ({
                      ...f,
                      resilience: {
                        ...f.resilience,
                        circuit_breaker: {
                          ...f.resilience.circuit_breaker,
                          error_threshold_pct: parseFloat(e.target.value) || 0,
                        },
                      },
                    }))}
                  />
                </div>
                <div>
                  <Label>Window (seconds)</Label>
                  <Input
                    type="number"
                    min={1}
                    value={form.resilience.circuit_breaker.window_seconds}
                    onChange={e => setForm(f => ({
                      ...f,
                      resilience: {
                        ...f.resilience,
                        circuit_breaker: {
                          ...f.resilience.circuit_breaker,
                          window_seconds: parseInt(e.target.value) || 0,
                        },
                      },
                    }))}
                  />
                </div>
                <div>
                  <Label>Open Duration (seconds)</Label>
                  <Input
                    type="number"
                    min={1}
                    value={form.resilience.circuit_breaker.open_duration_seconds}
                    onChange={e => setForm(f => ({
                      ...f,
                      resilience: {
                        ...f.resilience,
                        circuit_breaker: {
                          ...f.resilience.circuit_breaker,
                          open_duration_seconds: parseInt(e.target.value) || 0,
                        },
                      },
                    }))}
                  />
                </div>
              </div>
            </div>

            <Separator />

            {/* Timeout */}
            <div>
              <div className="text-sm font-medium mb-3">Timeout</div>
              <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
                <div>
                  <Label>Connect (ms)</Label>
                  <Input
                    type="number"
                    min={100}
                    value={form.resilience.timeout.connect_ms}
                    onChange={e => setForm(f => ({
                      ...f,
                      resilience: {
                        ...f.resilience,
                        timeout: { ...f.resilience.timeout, connect_ms: parseInt(e.target.value) || 0 },
                      },
                    }))}
                  />
                </div>
                <div>
                  <Label>Request (ms)</Label>
                  <Input
                    type="number"
                    min={1000}
                    value={form.resilience.timeout.request_ms}
                    onChange={e => setForm(f => ({
                      ...f,
                      resilience: {
                        ...f.resilience,
                        timeout: { ...f.resilience.timeout, request_ms: parseInt(e.target.value) || 0 },
                      },
                    }))}
                  />
                </div>
                <div>
                  <Label>TTFT (ms)</Label>
                  <Input
                    type="number"
                    min={1000}
                    value={form.resilience.timeout.ttft_ms}
                    onChange={e => setForm(f => ({
                      ...f,
                      resilience: {
                        ...f.resilience,
                        timeout: { ...f.resilience.timeout, ttft_ms: parseInt(e.target.value) || 0 },
                      },
                    }))}
                  />
                  <p className="text-xs text-[hsl(var(--muted-foreground))] mt-1">Time to first token</p>
                </div>
              </div>
            </div>
          </CardContent>
        </Card>
      </div>

      {/* ── Add Override Dialog ── */}
      <Dialog open={overrideOpen} onOpenChange={setOverrideOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Add Model Override</DialogTitle>
            <DialogDescription>Override the routing strategy for a specific model</DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div>
              <Label>Model ID</Label>
              <Input
                value={overrideForm.model_id}
                onChange={e => setOverrideForm(f => ({ ...f, model_id: e.target.value }))}
                placeholder="gpt-4o"
              />
            </div>
            <div>
              <Label>Strategy</Label>
              <Select value={overrideForm.strategy} onValueChange={v => setOverrideForm(f => ({ ...f, strategy: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {STRATEGIES.map(s => <SelectItem key={s} value={s}>{s}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div>
              <Label>Pool (optional)</Label>
              <Input
                value={overrideForm.pool || ""}
                onChange={e => setOverrideForm(f => ({ ...f, pool: e.target.value }))}
                placeholder="Leave empty for default pool"
              />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setOverrideOpen(false)}>Cancel</Button>
            <Button onClick={addOverride} disabled={!overrideForm.model_id}>Add</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
