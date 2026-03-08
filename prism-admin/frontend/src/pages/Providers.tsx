import { useEffect, useState, useRef, useCallback } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, DialogDescription } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Badge } from "@/components/ui/badge"
import { Switch } from "@/components/ui/switch"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Separator } from "@/components/ui/separator"
import { Trash2, Pencil, Plus, ArrowLeft, X, ChevronDown, Eye, EyeOff, Loader2, Key } from "lucide-react"
import { ConfirmDialog } from "@/components/ui/confirm-dialog"

const KNOWN_PROTOCOLS = ["openai", "anthropic", "azure", "gemini", "bedrock", "minimax", "cohere", "mistral", "deepseek"]

interface ProviderDiscovery {
  sidecar_url?: string; cluster_name?: string; namespace?: string
  label_selector?: string; enable_pd_filter?: boolean; poll_interval_secs?: number
}

interface Provider {
  id: string; base_url: string; protocols: string[]; api_key?: string; timeout_ms?: number
  circuit_breaker_threshold: number; circuit_breaker_recovery_secs: number; headers?: Record<string, string>
  provider_type?: string; discovery?: ProviderDiscovery
}

interface Account {
  id: string; provider_id: string; api_key?: string | null; base_url?: string | null; headers?: Record<string, string>
  project_id?: string | null; billing_tag?: string | null; weight: number; enabled: boolean
  rate_limit?: { rpm?: number | null; tpm?: number | null; concurrent?: number | null } | null
  metadata?: Record<string, string>
}

interface ProviderForm {
  id: string; base_url: string; protocols: string[]; api_key: string; timeout_ms: string
  circuit_breaker_threshold: string; circuit_breaker_recovery_secs: string
  headers: { key: string; value: string }[]
  provider_type: "api" | "sidecar"
  discovery: {
    sidecar_url: string; cluster_name: string; namespace: string
    label_selector: string; enable_pd_filter: boolean; poll_interval_secs: string
  }
}

const emptyDiscoveryForm = {
  sidecar_url: "", cluster_name: "", namespace: "",
  label_selector: "", enable_pd_filter: false, poll_interval_secs: "",
}

const emptyProviderForm: ProviderForm = {
  id: "", base_url: "", protocols: ["openai"], api_key: "", timeout_ms: "",
  circuit_breaker_threshold: "5", circuit_breaker_recovery_secs: "30", headers: [],
  provider_type: "api", discovery: { ...emptyDiscoveryForm },
}

interface AccountForm {
  id: string; provider_id: string; api_key: string; base_url: string
  headers: { key: string; value: string }[]; project_id: string; billing_tag: string
  weight: string; enabled: boolean; rpm: string; tpm: string; concurrent: string
  metadata: { key: string; value: string }[]
}

const emptyAccountForm = (pid: string): AccountForm => ({
  id: "", provider_id: pid, api_key: "", base_url: "", headers: [], project_id: "",
  billing_tag: "", weight: "1.0", enabled: true, rpm: "", tpm: "", concurrent: "", metadata: [],
})

// ── ProtocolsSelect ──
function ProtocolsSelect({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const [open, setOpen] = useState(false)
  const [customInput, setCustomInput] = useState("")
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const h = (e: MouseEvent) => { if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false) }
    document.addEventListener("mousedown", h); return () => document.removeEventListener("mousedown", h)
  }, [open])

  const toggle = (p: string) => onChange(value.includes(p) ? value.filter(x => x !== p) : [...value, p])
  const addCustom = () => { const t = customInput.trim().toLowerCase(); if (t && !value.includes(t)) onChange([...value, t]); setCustomInput("") }
  const available = KNOWN_PROTOCOLS.filter(p => !value.includes(p))

  return (
    <div ref={ref} className="relative">
      <div className="flex flex-wrap items-center gap-1.5 min-h-[36px] w-full rounded-md border border-[hsl(var(--input))] bg-transparent px-3 py-1.5 text-sm cursor-pointer" onClick={() => setOpen(!open)}>
        {value.length === 0 && <span className="text-[hsl(var(--muted-foreground))]">Select protocols...</span>}
        {value.map(p => <Badge key={p} variant="secondary" className="gap-1 pr-1">{p}<button type="button" className="ml-0.5 rounded-full hover:bg-[hsl(var(--muted-foreground)/0.2)] p-0.5" onClick={e => { e.stopPropagation(); toggle(p) }}><X className="h-3 w-3" /></button></Badge>)}
        <ChevronDown className="h-4 w-4 ml-auto shrink-0 text-[hsl(var(--muted-foreground))]" />
      </div>
      {open && (
        <div className="absolute z-50 mt-1 w-full rounded-md border border-[hsl(var(--border))] bg-[hsl(var(--popover))] text-[hsl(var(--popover-foreground))] shadow-md">
          <div className="max-h-48 overflow-y-auto p-1">
            {available.map(p => <button key={p} type="button" className="w-full text-left px-3 py-1.5 text-sm rounded hover:bg-[hsl(var(--accent))]" onClick={() => toggle(p)}>{p}</button>)}
            {available.length === 0 && <div className="px-3 py-1.5 text-sm text-[hsl(var(--muted-foreground))]">All protocols selected</div>}
          </div>
          <div className="border-t border-[hsl(var(--border))] p-2 flex gap-2">
            <Input placeholder="Custom..." value={customInput} onChange={e => setCustomInput(e.target.value)} onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); addCustom() } }} className="h-8 text-sm" onClick={e => e.stopPropagation()} />
            <Button size="sm" variant="outline" className="h-8 shrink-0" onClick={addCustom} disabled={!customInput.trim()}>Add</Button>
          </div>
        </div>
      )}
    </div>
  )
}

// ── Provider Type Selector ──
function ProviderTypeSelect({ value, onChange }: { value: "api" | "sidecar"; onChange: (v: "api" | "sidecar") => void }) {
  return (
    <div>
      <Label>Type</Label>
      <Select value={value} onValueChange={v => onChange(v as "api" | "sidecar")}>
        <SelectTrigger><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="api">API Provider</SelectItem>
          <SelectItem value="sidecar">Sidecar Discovery</SelectItem>
        </SelectContent>
      </Select>
    </div>
  )
}

// ── K8s Discovery Fields ──
function DiscoveryFields({ discovery, onChange }: { discovery: ProviderForm["discovery"]; onChange: (d: ProviderForm["discovery"]) => void }) {
  const upd = <K extends keyof ProviderForm["discovery"]>(k: K, v: ProviderForm["discovery"][K]) => onChange({ ...discovery, [k]: v })
  return (
    <div className="space-y-4">
      <Separator />
      <p className="text-xs font-medium text-[hsl(var(--muted-foreground))] uppercase tracking-wider">Discovery Configuration</p>
      <div><Label>Sidecar URL <span className="text-red-400">*</span></Label><Input value={discovery.sidecar_url} onChange={e => upd("sidecar_url", e.target.value)} placeholder="http://sidecar:9091" /></div>
      <div className="grid grid-cols-2 gap-3">
        <div><Label>Cluster Name</Label><Input value={discovery.cluster_name} onChange={e => upd("cluster_name", e.target.value)} placeholder="ai-infra-staging" /></div>
        <div><Label>Namespace</Label><Input value={discovery.namespace} onChange={e => upd("namespace", e.target.value)} placeholder="default" /></div>
      </div>
      <div><Label>Label Selector</Label><Input value={discovery.label_selector} onChange={e => upd("label_selector", e.target.value)} placeholder="app=inference-service" /></div>
      <div className="grid grid-cols-2 gap-3">
        <div><Label>Poll Interval (secs)</Label><Input type="number" value={discovery.poll_interval_secs} onChange={e => upd("poll_interval_secs", e.target.value)} placeholder="30" /></div>
        <div className="flex items-end pb-2"><div className="flex items-center gap-2"><Switch checked={discovery.enable_pd_filter} onCheckedChange={v => upd("enable_pd_filter", v)} /><Label>P/D Filter</Label></div></div>
      </div>
    </div>
  )
}

// ════════════════════════════════════════
// Provider Detail View
// ════════════════════════════════════════
function ProviderDetail({ providerId, onBack, onUpdated }: { providerId: string; onBack: () => void; onUpdated: () => void }) {
  const [provider, setProvider] = useState<Provider | null>(null)
  const [accounts, setAccounts] = useState<Account[]>([])
  const [loading, setLoading] = useState(true)
  const [activeTab, setActiveTab] = useState<"overview" | "accounts">("overview")

  const [providerDialogOpen, setProviderDialogOpen] = useState(false)
  const [providerForm, setProviderForm] = useState<ProviderForm>(emptyProviderForm)
  const [providerSaving, setProviderSaving] = useState(false)

  const [accountDialogOpen, setAccountDialogOpen] = useState(false)
  const [editingAccount, setEditingAccount] = useState<string | null>(null)
  const [accountForm, setAccountForm] = useState<AccountForm>(emptyAccountForm(providerId))
  const [accountSaving, setAccountSaving] = useState(false)
  const [showKey, setShowKey] = useState(false)

  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)
  const [togglingId, setTogglingId] = useState<string | null>(null)

  const loadDetail = useCallback(async () => {
    setLoading(true)
    try {
      const [pRes, aRes] = await Promise.all([
        fetch(`/api/providers/${providerId}`).then(r => r.json()),
        fetch("/api/accounts").then(r => r.json()),
      ])
      setProvider(pRes)
      setAccounts((aRes.accounts || []).filter((a: Account) => a.provider_id === providerId))
    } catch { /* */ }
    setLoading(false)
  }, [providerId])

  useEffect(() => { loadDetail() }, [loadDetail])

  const enabledCount = accounts.filter(a => a.enabled).length

  // Provider edit
  const openProviderEdit = () => {
    if (!provider) return
    setProviderForm(providerToForm(provider))
    setProviderDialogOpen(true)
  }

  const saveProvider = async () => {
    setProviderSaving(true)
    const body = buildProviderBody(providerForm)
    await fetch(`/api/providers/${providerId}`, {
      method: "PUT", headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    })
    setProviderSaving(false); setProviderDialogOpen(false); loadDetail(); onUpdated()
  }

  // Account toggle
  const toggleAccount = async (acct: Account) => {
    setTogglingId(acct.id)
    setAccounts(prev => prev.map(a => a.id === acct.id ? { ...a, enabled: !a.enabled } : a))
    await fetch(`/api/accounts/${acct.id}`, {
      method: "PUT", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ ...acct, enabled: !acct.enabled }),
    })
    setTogglingId(null)
  }

  // Account CRUD
  const openNewAccount = () => { setAccountForm(emptyAccountForm(providerId)); setEditingAccount(null); setShowKey(false); setAccountDialogOpen(true) }

  const openEditAccount = async (id: string) => {
    const data: Account = await fetch(`/api/accounts/${id}`).then(r => r.json())
    setAccountForm({
      id: data.id, provider_id: data.provider_id, api_key: data.api_key || "", base_url: data.base_url || "",
      headers: Object.entries(data.headers || {}).map(([key, value]) => ({ key, value })),
      project_id: data.project_id || "", billing_tag: data.billing_tag || "",
      weight: (data.weight ?? 1.0).toString(), enabled: data.enabled,
      rpm: data.rate_limit?.rpm?.toString() || "", tpm: data.rate_limit?.tpm?.toString() || "",
      concurrent: data.rate_limit?.concurrent?.toString() || "",
      metadata: Object.entries(data.metadata || {}).map(([key, value]) => ({ key, value })),
    })
    setEditingAccount(id); setShowKey(false); setAccountDialogOpen(true)
  }

  const saveAccount = async () => {
    setAccountSaving(true)
    const headers: Record<string, string> = {}
    accountForm.headers.forEach(h => { if (h.key) headers[h.key] = h.value })
    const metadata: Record<string, string> = {}
    accountForm.metadata.forEach(m => { if (m.key) metadata[m.key] = m.value })
    const hasRL = accountForm.rpm || accountForm.tpm || accountForm.concurrent
    const body = {
      id: editingAccount || accountForm.id, provider_id: providerId,
      api_key: accountForm.api_key || null, base_url: accountForm.base_url || null, headers, project_id: accountForm.project_id || null,
      billing_tag: accountForm.billing_tag || null, weight: parseFloat(accountForm.weight) || 1.0,
      enabled: accountForm.enabled,
      rate_limit: hasRL ? { rpm: accountForm.rpm ? parseInt(accountForm.rpm) : null, tpm: accountForm.tpm ? parseInt(accountForm.tpm) : null, concurrent: accountForm.concurrent ? parseInt(accountForm.concurrent) : null } : null,
      metadata,
    }
    await fetch(editingAccount ? `/api/accounts/${editingAccount}` : "/api/accounts", {
      method: editingAccount ? "PUT" : "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
    })
    setAccountSaving(false); setAccountDialogOpen(false); loadDetail(); onUpdated()
  }

  const deleteAccount = async (id: string) => { await fetch(`/api/accounts/${id}`, { method: "DELETE" }); loadDetail(); onUpdated() }

  // Form helpers
  const addHeader = () => setAccountForm(f => ({ ...f, headers: [...f.headers, { key: "", value: "" }] }))
  const removeHeader = (i: number) => setAccountForm(f => ({ ...f, headers: f.headers.filter((_, idx) => idx !== i) }))
  const updateHeader = (i: number, field: "key" | "value", val: string) => setAccountForm(f => ({ ...f, headers: f.headers.map((h, idx) => idx === i ? { ...h, [field]: val } : h) }))
  const addMeta = () => setAccountForm(f => ({ ...f, metadata: [...f.metadata, { key: "", value: "" }] }))
  const removeMeta = (i: number) => setAccountForm(f => ({ ...f, metadata: f.metadata.filter((_, idx) => idx !== i) }))
  const updateMeta = (i: number, field: "key" | "value", val: string) => setAccountForm(f => ({ ...f, metadata: f.metadata.map((m, idx) => idx === i ? { ...m, [field]: val } : m) }))
  const addProviderHeader = () => setProviderForm(f => ({ ...f, headers: [...f.headers, { key: "", value: "" }] }))
  const removeProviderHeader = (i: number) => setProviderForm(f => ({ ...f, headers: f.headers.filter((_, idx) => idx !== i) }))
  const updateProviderHeader = (i: number, field: "key" | "value", val: string) => setProviderForm(f => ({ ...f, headers: f.headers.map((h, idx) => idx === i ? { ...h, [field]: val } : h) }))

  if (loading) return <div className="flex items-center justify-center py-12"><Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" /></div>
  if (!provider) return <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">Provider not found</div>

  const isSidecar = provider.provider_type === "sidecar"

  return (
    <div>
      {/* Header */}
      <div className="flex items-center gap-3 mb-6">
        <Button variant="ghost" size="icon" onClick={onBack}><ArrowLeft className="h-4 w-4" /></Button>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <h1 className="text-xl sm:text-2xl font-bold">{provider.id}</h1>
            {isSidecar && <Badge className="bg-purple-600/80 text-purple-100 hover:bg-purple-600/80">Sidecar</Badge>}
            {provider.protocols.map(p => <Badge key={p} variant="secondary">{p}</Badge>)}
          </div>
          <p className="text-sm text-[hsl(var(--muted-foreground))] truncate">{isSidecar ? provider.discovery?.sidecar_url || "K8s discovery" : provider.base_url}</p>
        </div>
        <Button variant="outline" size="sm" onClick={openProviderEdit}><Pencil className="h-3.5 w-3.5 mr-1.5" /> Edit</Button>
      </div>

      {/* Tabs */}
      <div className="flex gap-1 mb-4 border-b border-[hsl(var(--border))]">
        {(isSidecar ? [["overview", "Overview"]] as const : [["overview", "Overview"], ["accounts", `Accounts (${accounts.length})`]] as const).map(([key, label]) => (
          <button key={key} onClick={() => setActiveTab(key)}
            className={`px-4 py-2 text-sm font-medium border-b-2 transition-colors -mb-px ${activeTab === key ? "border-[hsl(var(--foreground))] text-[hsl(var(--foreground))]" : "border-transparent text-[hsl(var(--muted-foreground))] hover:text-[hsl(var(--foreground))]"}`}>{label}</button>
        ))}
      </div>

      {/* Overview */}
      {activeTab === "overview" && (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <Card>
            <CardHeader className="pb-3"><CardTitle className="text-sm">{isSidecar ? "Discovery Configuration" : "Configuration"}</CardTitle></CardHeader>
            <CardContent className="space-y-3 text-sm">
              {isSidecar ? (<>
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Sidecar URL</span><span className="font-mono text-xs truncate max-w-[200px]">{provider.discovery?.sidecar_url || "—"}</span></div>
                <Separator />
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Cluster</span><span>{provider.discovery?.cluster_name || "—"}</span></div>
                <Separator />
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Namespace</span><span>{provider.discovery?.namespace || "all"}</span></div>
                <Separator />
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Label Selector</span><span className="font-mono text-xs truncate max-w-[200px]">{provider.discovery?.label_selector || "default"}</span></div>
                <Separator />
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">P/D Filter</span><Badge variant={provider.discovery?.enable_pd_filter ? "default" : "secondary"}>{provider.discovery?.enable_pd_filter ? "Enabled" : "Disabled"}</Badge></div>
                <Separator />
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Poll Interval</span><span>{provider.discovery?.poll_interval_secs || 30}s</span></div>
              </>) : (<>
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Base URL</span><span className="font-mono text-xs truncate max-w-[200px]">{provider.base_url || "—"}</span></div>
                <Separator />
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Protocols</span><div className="flex gap-1">{provider.protocols.map(p => <Badge key={p} variant="outline" className="text-[10px]">{p}</Badge>)}</div></div>
                <Separator />
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Timeout</span><span>{provider.timeout_ms ? `${provider.timeout_ms}ms` : "default"}</span></div>
                <Separator />
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Circuit Breaker</span><span>{provider.circuit_breaker_threshold} / {provider.circuit_breaker_recovery_secs}s</span></div>
                {provider.headers && Object.keys(provider.headers).length > 0 && (<><Separator /><div><span className="text-[hsl(var(--muted-foreground))]">Headers</span><div className="mt-1 space-y-1">{Object.entries(provider.headers).map(([k, v]) => <div key={k} className="font-mono text-xs"><span className="text-[hsl(var(--muted-foreground))]">{k}:</span> {v}</div>)}</div></div></>)}
              </>)}
            </CardContent>
          </Card>
          {isSidecar ? (
            <Card>
              <CardHeader className="pb-3"><CardTitle className="text-sm">Sidecar Discovery</CardTitle></CardHeader>
              <CardContent className="text-sm text-[hsl(var(--muted-foreground))]">
                <p>Instances are auto-discovered from the Kubernetes cluster via the sidecar. Accounts are not applicable for K8s providers.</p>
                <Separator className="my-3" />
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Circuit Breaker</span><span>{provider.circuit_breaker_threshold} / {provider.circuit_breaker_recovery_secs}s</span></div>
              </CardContent>
            </Card>
          ) : (
            <Card>
              <CardHeader className="pb-3"><CardTitle className="text-sm">Accounts Summary</CardTitle></CardHeader>
              <CardContent className="space-y-3 text-sm">
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Total</span><span className="font-medium">{accounts.length}</span></div>
                <Separator />
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Enabled</span><Badge variant="secondary" className="bg-green-900/50 text-green-400">{enabledCount}</Badge></div>
                <Separator />
                <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Disabled</span><Badge variant="secondary">{accounts.length - enabledCount}</Badge></div>
                <Separator />
                <Button variant="outline" size="sm" className="w-full" onClick={() => setActiveTab("accounts")}><Key className="h-3.5 w-3.5 mr-1.5" /> Manage Accounts</Button>
              </CardContent>
            </Card>
          )}
        </div>
      )}

      {/* Accounts Tab */}
      {activeTab === "accounts" && !isSidecar && (
        <div>
          <div className="flex items-center justify-between mb-4">
            <div><h2 className="text-lg font-semibold">Accounts</h2><p className="text-sm text-[hsl(var(--muted-foreground))]">API accounts and credentials for this provider</p></div>
            <Button size="sm" onClick={openNewAccount}><Plus className="h-4 w-4 mr-1" /> New Account</Button>
          </div>
          <Card>
            <CardContent className="p-0">
              {accounts.length === 0 ? (
                <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">No accounts yet</div>
              ) : (
                <div className="overflow-x-auto">
                  <table className="w-full text-sm">
                    <thead><tr className="border-b border-[hsl(var(--border))]">
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Account ID</th>
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">API Key</th>
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden md:table-cell">Base URL</th>
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Enabled</th>
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Weight</th>
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Rate Limits</th>
                      <th className="text-right px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Actions</th>
                    </tr></thead>
                    <tbody>
                      {accounts.map(acct => (
                        <tr key={acct.id} className="border-b border-[hsl(var(--border))] last:border-0 hover:bg-[hsl(var(--accent))]/50 transition-colors">
                          <td className="px-4 py-3"><div className="font-medium">{acct.id}</div>{acct.billing_tag && <div className="text-xs text-[hsl(var(--muted-foreground))]">billing: {acct.billing_tag}</div>}</td>
                          <td className="px-4 py-3 font-mono text-xs text-[hsl(var(--muted-foreground))]">{acct.api_key ? `...${acct.api_key.slice(-4)}` : "—"}</td>
                          <td className="px-4 py-3 hidden md:table-cell"><span className="text-xs font-mono text-[hsl(var(--muted-foreground))] truncate max-w-[150px] block">{acct.base_url || "—"}</span></td>
                          <td className="px-4 py-3"><Switch checked={acct.enabled} disabled={togglingId === acct.id} onCheckedChange={() => toggleAccount(acct)} /></td>
                          <td className="px-4 py-3 font-mono text-xs">{acct.weight}</td>
                          <td className="px-4 py-3 text-xs text-[hsl(var(--muted-foreground))] hidden sm:table-cell">{acct.rate_limit ? [acct.rate_limit.rpm != null && `${acct.rate_limit.rpm} rpm`, acct.rate_limit.tpm != null && `${acct.rate_limit.tpm} tpm`, acct.rate_limit.concurrent != null && `${acct.rate_limit.concurrent} conc`].filter(Boolean).join(", ") || "—" : "—"}</td>
                          <td className="px-4 py-3"><div className="flex justify-end gap-1">
                            <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => openEditAccount(acct.id)}><Pencil className="h-3.5 w-3.5" /></Button>
                            <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => setDeleteTarget(acct.id)}><Trash2 className="h-3.5 w-3.5 text-red-400" /></Button>
                          </div></td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </CardContent>
          </Card>
        </div>
      )}

      {/* Provider Edit Dialog */}
      <Dialog open={providerDialogOpen} onOpenChange={setProviderDialogOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader><DialogTitle>Edit Provider: {providerId}</DialogTitle><DialogDescription>Update provider configuration</DialogDescription></DialogHeader>
          <div className="space-y-4">
            <ProviderTypeSelect value={providerForm.provider_type} onChange={provider_type => setProviderForm(f => ({ ...f, provider_type }))} />
            {providerForm.provider_type === "api" && (<>
              <div><Label>Base URL</Label><Input value={providerForm.base_url} onChange={e => setProviderForm(f => ({ ...f, base_url: e.target.value }))} placeholder="https://api.openai.com/v1" /></div>
              <div><Label>Protocols</Label><ProtocolsSelect value={providerForm.protocols} onChange={protocols => setProviderForm(f => ({ ...f, protocols }))} /></div>
              <div><Label>API Key</Label><Input type="password" value={providerForm.api_key} onChange={e => setProviderForm(f => ({ ...f, api_key: e.target.value }))} placeholder="sk-..." /></div>
              <div className="grid grid-cols-2 gap-3">
                <div><Label>Timeout (ms)</Label><Input type="number" value={providerForm.timeout_ms} onChange={e => setProviderForm(f => ({ ...f, timeout_ms: e.target.value }))} placeholder="30000" /></div>
                <div><Label>CB Threshold</Label><Input type="number" value={providerForm.circuit_breaker_threshold} onChange={e => setProviderForm(f => ({ ...f, circuit_breaker_threshold: e.target.value }))} /></div>
              </div>
              <div><Label>CB Recovery (secs)</Label><Input type="number" value={providerForm.circuit_breaker_recovery_secs} onChange={e => setProviderForm(f => ({ ...f, circuit_breaker_recovery_secs: e.target.value }))} /></div>
              <div>
                <div className="flex items-center justify-between mb-2"><Label>Headers</Label><Button variant="ghost" size="sm" onClick={addProviderHeader}><Plus className="h-3 w-3 mr-1" /> Add</Button></div>
                {providerForm.headers.map((h, i) => (<div key={i} className="flex gap-2 mb-2"><Input placeholder="Key" value={h.key} onChange={e => updateProviderHeader(i, "key", e.target.value)} className="flex-1" /><Input placeholder="Value" value={h.value} onChange={e => updateProviderHeader(i, "value", e.target.value)} className="flex-1" /><Button variant="ghost" size="icon" onClick={() => removeProviderHeader(i)}><X className="h-4 w-4" /></Button></div>))}
              </div>
            </>)}
            {providerForm.provider_type === "sidecar" && (
              <DiscoveryFields discovery={providerForm.discovery} onChange={discovery => setProviderForm(f => ({ ...f, discovery }))} />
            )}
          </div>
          <DialogFooter><Button variant="outline" onClick={() => setProviderDialogOpen(false)}>Cancel</Button><Button onClick={saveProvider} disabled={providerSaving || (providerForm.provider_type === "sidecar" && !providerForm.discovery.sidecar_url)}>{providerSaving ? "Saving..." : "Save"}</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Account Edit Dialog */}
      <Dialog open={accountDialogOpen} onOpenChange={setAccountDialogOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader><DialogTitle>{editingAccount ? `Edit Account: ${editingAccount}` : "New Account"}</DialogTitle><DialogDescription>{editingAccount ? "Update account" : `Add account for ${providerId}`}</DialogDescription></DialogHeader>
          <div className="space-y-4">
            {!editingAccount && (<div><Label>Account ID</Label><Input value={accountForm.id} onChange={e => setAccountForm(f => ({ ...f, id: e.target.value }))} placeholder="acct-openai-1" /><p className="text-xs text-[hsl(var(--muted-foreground))] mt-1">Cannot be changed later</p></div>)}
            <div><Label>API Key</Label><div className="flex gap-2"><Input type={showKey ? "text" : "password"} value={accountForm.api_key} onChange={e => setAccountForm(f => ({ ...f, api_key: e.target.value }))} placeholder="sk-..." className="flex-1" /><Button variant="ghost" size="icon" type="button" onClick={() => setShowKey(v => !v)}>{showKey ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}</Button></div><p className="text-xs text-[hsl(var(--muted-foreground))] mt-1">Leave empty to use provider default</p></div>
            <div className="grid grid-cols-2 gap-3"><div><Label>Project ID</Label><Input value={accountForm.project_id} onChange={e => setAccountForm(f => ({ ...f, project_id: e.target.value }))} placeholder="Optional" /></div><div><Label>Billing Tag</Label><Input value={accountForm.billing_tag} onChange={e => setAccountForm(f => ({ ...f, billing_tag: e.target.value }))} placeholder="Optional" /></div></div>
            <div className="grid grid-cols-2 gap-3"><div><Label>Weight</Label><Input type="number" step="0.1" value={accountForm.weight} onChange={e => setAccountForm(f => ({ ...f, weight: e.target.value }))} /></div><div className="flex items-end pb-2"><div className="flex items-center gap-2"><Switch checked={accountForm.enabled} onCheckedChange={v => setAccountForm(f => ({ ...f, enabled: v }))} /><Label>Enabled</Label></div></div></div>
            <div><Label className="mb-2 block">Rate Limits</Label><div className="grid grid-cols-3 gap-3"><div><Label className="text-xs text-[hsl(var(--muted-foreground))]">RPM</Label><Input type="number" value={accountForm.rpm} onChange={e => setAccountForm(f => ({ ...f, rpm: e.target.value }))} placeholder="—" /></div><div><Label className="text-xs text-[hsl(var(--muted-foreground))]">TPM</Label><Input type="number" value={accountForm.tpm} onChange={e => setAccountForm(f => ({ ...f, tpm: e.target.value }))} placeholder="—" /></div><div><Label className="text-xs text-[hsl(var(--muted-foreground))]">Concurrent</Label><Input type="number" value={accountForm.concurrent} onChange={e => setAccountForm(f => ({ ...f, concurrent: e.target.value }))} placeholder="—" /></div></div></div>
            <div><div className="flex items-center justify-between mb-2"><Label>Headers</Label><Button variant="ghost" size="sm" onClick={addHeader}><Plus className="h-3 w-3 mr-1" /> Add</Button></div>{accountForm.headers.map((h, i) => (<div key={i} className="flex gap-2 mb-2"><Input placeholder="Key" value={h.key} onChange={e => updateHeader(i, "key", e.target.value)} className="flex-1" /><Input placeholder="Value" value={h.value} onChange={e => updateHeader(i, "value", e.target.value)} className="flex-1" /><Button variant="ghost" size="icon" onClick={() => removeHeader(i)}><X className="h-4 w-4" /></Button></div>))}</div>
            <div><div className="flex items-center justify-between mb-2"><Label>Metadata</Label><Button variant="ghost" size="sm" onClick={addMeta}><Plus className="h-3 w-3 mr-1" /> Add</Button></div>{accountForm.metadata.map((m, i) => (<div key={i} className="flex gap-2 mb-2"><Input placeholder="Key" value={m.key} onChange={e => updateMeta(i, "key", e.target.value)} className="flex-1" /><Input placeholder="Value" value={m.value} onChange={e => updateMeta(i, "value", e.target.value)} className="flex-1" /><Button variant="ghost" size="icon" onClick={() => removeMeta(i)}><X className="h-4 w-4" /></Button></div>))}</div>
          </div>
          <DialogFooter><Button variant="outline" onClick={() => setAccountDialogOpen(false)}>Cancel</Button><Button onClick={saveAccount} disabled={accountSaving}>{accountSaving ? "Saving..." : "Save"}</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog open={!!deleteTarget} onOpenChange={open => { if (!open) setDeleteTarget(null) }}
        title="Delete Account" description={`Delete account "${deleteTarget}"? This cannot be undone.`}
        onConfirm={() => { if (deleteTarget) deleteAccount(deleteTarget) }} />
    </div>
  )
}

// ── Build provider JSON body from form ──
function buildProviderBody(form: ProviderForm) {
  if (form.provider_type === "sidecar") {
    const d = form.discovery
    return {
      type: "sidecar",
      discovery: {
        sidecar_url: d.sidecar_url || undefined,
        cluster_name: d.cluster_name || undefined,
        namespace: d.namespace || undefined,
        label_selector: d.label_selector || undefined,
        enable_pd_filter: d.enable_pd_filter,
        poll_interval_secs: d.poll_interval_secs ? parseInt(d.poll_interval_secs) : undefined,
      },
      circuit_breaker_threshold: parseInt(form.circuit_breaker_threshold) || 5,
      circuit_breaker_recovery_secs: parseInt(form.circuit_breaker_recovery_secs) || 30,
    }
  }
  const headers: Record<string, string> = {}
  form.headers.forEach(h => { if (h.key) headers[h.key] = h.value })
  return {
    base_url: form.base_url, protocols: form.protocols, api_key: form.api_key || null,
    timeout_ms: form.timeout_ms ? parseInt(form.timeout_ms) : null,
    circuit_breaker_threshold: parseInt(form.circuit_breaker_threshold) || 5,
    circuit_breaker_recovery_secs: parseInt(form.circuit_breaker_recovery_secs) || 30, headers,
  }
}

// ── Map Provider to ProviderForm ──
function providerToForm(p: Provider): ProviderForm {
  const isSidecar = p.provider_type === "sidecar"
  return {
    id: p.id, base_url: p.base_url || "", protocols: p.protocols || [],
    api_key: p.api_key || "", timeout_ms: p.timeout_ms?.toString() || "",
    circuit_breaker_threshold: (p.circuit_breaker_threshold ?? 5).toString(),
    circuit_breaker_recovery_secs: (p.circuit_breaker_recovery_secs ?? 30).toString(),
    headers: Object.entries(p.headers || {}).map(([key, value]) => ({ key, value })),
    provider_type: isSidecar ? "sidecar" : "api",
    discovery: isSidecar && p.discovery ? {
      sidecar_url: p.discovery.sidecar_url || "",
      cluster_name: p.discovery.cluster_name || "",
      namespace: p.discovery.namespace || "",
      label_selector: p.discovery.label_selector || "",
      enable_pd_filter: p.discovery.enable_pd_filter || false,
      poll_interval_secs: p.discovery.poll_interval_secs?.toString() || "",
    } : { ...emptyDiscoveryForm },
  }
}

// ════════════════════════════════════════
// Main Providers Page (Table)
// ════════════════════════════════════════
export default function Providers() {
  const [providers, setProviders] = useState<Provider[]>([])
  const [accounts, setAccounts] = useState<Account[]>([])
  const [loading, setLoading] = useState(true)
  const [selectedProvider, setSelectedProvider] = useState<string | null>(null)
  const [search, setSearch] = useState("")
  const [newDialogOpen, setNewDialogOpen] = useState(false)
  const [newForm, setNewForm] = useState<ProviderForm>(emptyProviderForm)
  const [newSaving, setNewSaving] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [pRes, aRes] = await Promise.all([fetch("/api/providers").then(r => r.json()), fetch("/api/accounts").then(r => r.json())])
      setProviders(pRes.providers || []); setAccounts(aRes.accounts || [])
    } catch { /* */ }
    setLoading(false)
  }, [])

  useEffect(() => { load() }, [load])

  const handleNewProvider = async () => {
    setNewSaving(true)
    const body = buildProviderBody(newForm)
    await fetch(`/api/providers/${newForm.id}`, {
      method: "PUT", headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    })
    setNewSaving(false); setNewDialogOpen(false); setNewForm(emptyProviderForm); load()
  }

  const handleDelete = async (id: string) => { await fetch(`/api/providers/${id}`, { method: "DELETE" }); load() }

  if (selectedProvider) return <ProviderDetail providerId={selectedProvider} onBack={() => { setSelectedProvider(null); load() }} onUpdated={load} />

  const filtered = search ? providers.filter(p => p.id.toLowerCase().includes(search.toLowerCase()) || p.protocols.some(pr => pr.toLowerCase().includes(search.toLowerCase())) || (p.provider_type === "sidecar" && "sidecar".includes(search.toLowerCase()))) : providers

  if (loading) return <div className="flex items-center justify-center py-12"><Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" /></div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <div><h1 className="text-xl sm:text-2xl font-bold">Providers</h1><p className="text-sm text-[hsl(var(--muted-foreground))]">Manage LLM providers and their accounts</p></div>
        <Button size="sm" onClick={() => { setNewForm(emptyProviderForm); setNewDialogOpen(true) }}><Plus className="h-4 w-4 mr-1" /> New Provider</Button>
      </div>
      <div className="mb-4"><Input placeholder="Search providers..." value={search} onChange={e => setSearch(e.target.value)} className="max-w-sm" /></div>
      <Card>
        <CardContent className="p-0">
          {filtered.length === 0 ? (
            <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">{search ? "No providers match" : "No providers configured"}</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead><tr className="border-b border-[hsl(var(--border))]">
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Provider</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Endpoint</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Protocols</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Accounts</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden md:table-cell">Circuit Breaker</th>
                  <th className="text-right px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Actions</th>
                </tr></thead>
                <tbody>
                  {filtered.map(p => {
                    const pa = accounts.filter(a => a.provider_id === p.id)
                    const en = pa.filter(a => a.enabled).length
                    const pIsSidecar = p.provider_type === "sidecar"
                    return (
                      <tr key={p.id} className="border-b border-[hsl(var(--border))] last:border-0 hover:bg-[hsl(var(--accent))]/50 cursor-pointer transition-colors" onClick={() => setSelectedProvider(p.id)}>
                        <td className="px-4 py-3 font-medium"><div className="flex items-center gap-2">{p.id}{pIsSidecar && <Badge className="bg-purple-600/80 text-purple-100 hover:bg-purple-600/80 text-[10px]">Sidecar</Badge>}</div></td>
                        <td className="px-4 py-3 hidden sm:table-cell"><span className="text-xs text-[hsl(var(--muted-foreground))] font-mono truncate max-w-[200px] block">{pIsSidecar ? (p.discovery?.sidecar_url || "—") : (p.base_url || "—")}</span></td>
                        <td className="px-4 py-3"><div className="flex gap-1 flex-wrap">{pIsSidecar ? <span className="text-xs text-[hsl(var(--muted-foreground))] italic">discovery</span> : p.protocols.map(pr => <Badge key={pr} variant="outline" className="text-[10px]">{pr}</Badge>)}</div></td>
                        <td className="px-4 py-3">{pIsSidecar ? <span className="text-xs text-[hsl(var(--muted-foreground))] italic">n/a</span> : pa.length > 0 ? <span className="font-mono text-xs">{en}/{pa.length}</span> : <span className="text-xs text-[hsl(var(--muted-foreground))] italic">none</span>}</td>
                        <td className="px-4 py-3 hidden md:table-cell"><span className="text-xs text-[hsl(var(--muted-foreground))]">{p.circuit_breaker_threshold} / {p.circuit_breaker_recovery_secs}s</span></td>
                        <td className="px-4 py-3" onClick={e => e.stopPropagation()}><div className="flex justify-end gap-1">
                          <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => setSelectedProvider(p.id)}><Pencil className="h-3.5 w-3.5" /></Button>
                          <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => setDeleteTarget(p.id)}><Trash2 className="h-3.5 w-3.5 text-red-400" /></Button>
                        </div></td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>

      {/* New Provider Dialog */}
      <Dialog open={newDialogOpen} onOpenChange={setNewDialogOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader><DialogTitle>New Provider</DialogTitle><DialogDescription>Add a new LLM provider</DialogDescription></DialogHeader>
          <div className="space-y-4">
            <div><Label>ID</Label><Input value={newForm.id} onChange={e => setNewForm(f => ({ ...f, id: e.target.value }))} placeholder="openai" /><p className="text-xs text-[hsl(var(--muted-foreground))] mt-1">Cannot be changed later</p></div>
            <ProviderTypeSelect value={newForm.provider_type} onChange={provider_type => setNewForm(f => ({ ...f, provider_type, ...(provider_type === "sidecar" ? { protocols: [], base_url: "", api_key: "" } : {}) }))} />
            {newForm.provider_type === "api" && (<>
              <div><Label>Base URL</Label><Input value={newForm.base_url} onChange={e => setNewForm(f => ({ ...f, base_url: e.target.value }))} placeholder="https://api.openai.com/v1" /></div>
              <div><Label>Protocols</Label><ProtocolsSelect value={newForm.protocols} onChange={protocols => setNewForm(f => ({ ...f, protocols }))} /></div>
              <div><Label>API Key</Label><Input type="password" value={newForm.api_key} onChange={e => setNewForm(f => ({ ...f, api_key: e.target.value }))} placeholder="sk-..." /></div>
              <div className="grid grid-cols-2 gap-3"><div><Label>Timeout (ms)</Label><Input type="number" value={newForm.timeout_ms} onChange={e => setNewForm(f => ({ ...f, timeout_ms: e.target.value }))} placeholder="30000" /></div><div><Label>CB Threshold</Label><Input type="number" value={newForm.circuit_breaker_threshold} onChange={e => setNewForm(f => ({ ...f, circuit_breaker_threshold: e.target.value }))} /></div></div>
              <div><Label>CB Recovery (secs)</Label><Input type="number" value={newForm.circuit_breaker_recovery_secs} onChange={e => setNewForm(f => ({ ...f, circuit_breaker_recovery_secs: e.target.value }))} /></div>
            </>)}
            {newForm.provider_type === "sidecar" && (
              <DiscoveryFields discovery={newForm.discovery} onChange={discovery => setNewForm(f => ({ ...f, discovery }))} />
            )}
          </div>
          <DialogFooter><Button variant="outline" onClick={() => setNewDialogOpen(false)}>Cancel</Button><Button onClick={handleNewProvider} disabled={newSaving || !newForm.id || (newForm.provider_type === "sidecar" && !newForm.discovery.sidecar_url)}>{newSaving ? "Creating..." : "Create"}</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog open={!!deleteTarget} onOpenChange={open => { if (!open) setDeleteTarget(null) }}
        title="Delete Provider" description={`Delete provider "${deleteTarget}"? This cannot be undone.`}
        onConfirm={() => { if (deleteTarget) handleDelete(deleteTarget) }} />
    </div>
  )
}
