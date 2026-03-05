import { useEffect, useState, useCallback } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, DialogDescription } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
import { Plus, Trash2, Pencil, ArrowLeft, Loader2, Layers } from "lucide-react"
import { ConfirmDialog } from "@/components/ui/confirm-dialog"

interface Capabilities {
  max_context_window: number
  max_output_tokens: number
  supports_streaming: boolean
  supports_function_calling: boolean
  supports_vision: boolean
  supports_json_mode: boolean
  supports_reasoning: boolean
}

interface Pricing {
  input_per_1m_tokens: number
  output_per_1m_tokens: number
  currency?: string
}

interface Model {
  id: string
  display_name: string
  quality_tier: string
  capabilities: Capabilities
  pricing: Pricing
  specialties: string[]
}

interface Variant {
  id: string
  model_id: string
  provider_id: string
  weight: number | null
  account_id: string | null
  upstream_model_name: string | null
  base_url: string | null
}

interface ProviderItem { id: string }
interface AccountItem { id: string; provider_id: string }

const QUALITY_TIERS = ["S", "A", "B", "C"]
const SPECIALTIES = [
  "SimpleQA", "CodeGeneration", "CodeReview", "Reasoning", "Creative",
  "Translation", "Summarization", "DataExtraction", "Math", "MultiModal", "Conversation"
]

const defaultCaps: Capabilities = {
  max_context_window: 128000, max_output_tokens: 4096,
  supports_streaming: true, supports_function_calling: false,
  supports_vision: false, supports_json_mode: false, supports_reasoning: false,
}

interface ModelFormState {
  id: string
  display_name: string
  quality_tier: string
  capabilities: Capabilities
  pricing: { input: string; output: string }
  specialties: string[]
}

const emptyModelForm: ModelFormState = {
  id: "", display_name: "", quality_tier: "A",
  capabilities: defaultCaps,
  pricing: { input: "0", output: "0" },
  specialties: [],
}

interface VariantFormState {
  id: string
  provider_id: string
  weight: string
  account_id: string
  upstream_model_name: string
}

const emptyVariantForm: VariantFormState = {
  id: "", provider_id: "", weight: "1.0", account_id: "", upstream_model_name: "",
}

// ════════════════════════════════════════
// Model Detail View
// ════════════════════════════════════════
function ModelDetail({ modelId, onBack, onUpdated }: { modelId: string; onBack: () => void; onUpdated: () => void }) {
  const [model, setModel] = useState<Model | null>(null)
  const [variants, setVariants] = useState<Variant[]>([])
  const [providers, setProviders] = useState<ProviderItem[]>([])
  const [accounts, setAccounts] = useState<AccountItem[]>([])
  const [loading, setLoading] = useState(true)
  const [activeTab, setActiveTab] = useState<"overview" | "variants">("overview")

  const [modelDialogOpen, setModelDialogOpen] = useState(false)
  const [modelForm, setModelForm] = useState<ModelFormState>(emptyModelForm)
  const [modelSaving, setModelSaving] = useState(false)

  const [variantDialogOpen, setVariantDialogOpen] = useState(false)
  const [editingVariant, setEditingVariant] = useState<string | null>(null)
  const [variantForm, setVariantForm] = useState<VariantFormState>(emptyVariantForm)
  const [variantSaving, setVariantSaving] = useState(false)

  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)

  const loadDetail = useCallback(async () => {
    setLoading(true)
    try {
      const [mRes, vRes, pRes, aRes] = await Promise.all([
        fetch("/api/models").then(r => r.json()),
        fetch("/api/variants").then(r => r.json()),
        fetch("/api/providers").then(r => r.json()),
        fetch("/api/accounts").then(r => r.json()),
      ])
      const foundModel = (mRes.models || []).find((m: Model) => m.id === modelId)
      setModel(foundModel || null)
      setVariants((vRes.variants || []).filter((v: Variant) => v.model_id === modelId))
      setProviders(pRes.providers || [])
      setAccounts(aRes.accounts || [])
    } catch { /* */ }
    setLoading(false)
  }, [modelId])

  useEffect(() => { loadDetail() }, [loadDetail])

  const enabledVariants = variants.length // all variants are "enabled" — no enabled field on variants

  // Model edit
  const openModelEdit = () => {
    if (!model) return
    setModelForm({
      id: model.id,
      display_name: model.display_name,
      quality_tier: model.quality_tier,
      capabilities: model.capabilities || defaultCaps,
      pricing: {
        input: (model.pricing?.input_per_1m_tokens ?? 0).toString(),
        output: (model.pricing?.output_per_1m_tokens ?? 0).toString(),
      },
      specialties: model.specialties || [],
    })
    setModelDialogOpen(true)
  }

  const saveModel = async () => {
    setModelSaving(true)
    await fetch(`/api/models/${modelId}`, {
      method: "PUT", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        id: modelId,
        display_name: modelForm.display_name,
        quality_tier: modelForm.quality_tier,
        capabilities: modelForm.capabilities,
        pricing: {
          input_per_1m_tokens: parseFloat(modelForm.pricing.input) || 0,
          output_per_1m_tokens: parseFloat(modelForm.pricing.output) || 0,
        },
        specialties: modelForm.specialties,
      }),
    })
    setModelSaving(false); setModelDialogOpen(false); loadDetail(); onUpdated()
  }

  const toggleSpecialty = (s: string) => {
    setModelForm(f => ({
      ...f,
      specialties: f.specialties.includes(s) ? f.specialties.filter(x => x !== s) : [...f.specialties, s]
    }))
  }

  const toggleCap = (key: keyof Capabilities) => {
    setModelForm(f => ({
      ...f,
      capabilities: { ...f.capabilities, [key]: !f.capabilities[key] }
    }))
  }

  // Variant CRUD
  const openNewVariant = () => { setVariantForm(emptyVariantForm); setEditingVariant(null); setVariantDialogOpen(true) }

  const openEditVariant = (v: Variant) => {
    setVariantForm({
      id: v.id,
      provider_id: v.provider_id,
      weight: (v.weight ?? 1.0).toString(),
      account_id: v.account_id || "",
      upstream_model_name: v.upstream_model_name || "",
    })
    setEditingVariant(v.id)
    setVariantDialogOpen(true)
  }

  const saveVariant = async () => {
    setVariantSaving(true)
    const body = {
      id: editingVariant || variantForm.id,
      model_id: modelId,
      provider_id: variantForm.provider_id,
      weight: parseFloat(variantForm.weight) || null,
      account_id: variantForm.account_id || null,
      upstream_model_name: variantForm.upstream_model_name || null,

    }
    const url = editingVariant ? `/api/variants/${editingVariant}` : "/api/variants"
    const method = editingVariant ? "PUT" : "POST"
    await fetch(url, { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    setVariantSaving(false); setVariantDialogOpen(false); loadDetail(); onUpdated()
  }

  const deleteVariant = async (id: string) => { await fetch(`/api/variants/${id}`, { method: "DELETE" }); loadDetail(); onUpdated() }

  // Filter accounts by selected provider
  const filteredAccounts = variantForm.provider_id
    ? accounts.filter(a => a.provider_id === variantForm.provider_id)
    : []

  if (loading) return <div className="flex items-center justify-center py-12"><Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" /></div>
  if (!model) return <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">Model not found</div>

  const capLabels: [keyof Capabilities, string][] = [
    ["supports_streaming", "Streaming"], ["supports_function_calling", "Function Calling"],
    ["supports_vision", "Vision"], ["supports_json_mode", "JSON Mode"], ["supports_reasoning", "Reasoning"],
  ]
  const enabledCaps = capLabels.filter(([k]) => model.capabilities?.[k])

  return (
    <div>
      {/* Header */}
      <div className="flex items-center gap-3 mb-6">
        <Button variant="ghost" size="icon" onClick={onBack}><ArrowLeft className="h-4 w-4" /></Button>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <h1 className="text-xl sm:text-2xl font-bold">{model.display_name}</h1>
            <Badge variant="secondary">{model.quality_tier}</Badge>
          </div>
          <p className="text-sm text-[hsl(var(--muted-foreground))] truncate">{model.id}</p>
        </div>
        <Button variant="outline" size="sm" onClick={openModelEdit}><Pencil className="h-3.5 w-3.5 mr-1.5" /> Edit</Button>
      </div>

      {/* Tabs */}
      <div className="flex gap-1 mb-4 border-b border-[hsl(var(--border))]">
        {([["overview", "Overview"], ["variants", `Variants (${variants.length})`]] as const).map(([key, label]) => (
          <button key={key} onClick={() => setActiveTab(key)}
            className={`px-4 py-2 text-sm font-medium border-b-2 transition-colors -mb-px ${activeTab === key ? "border-[hsl(var(--foreground))] text-[hsl(var(--foreground))]" : "border-transparent text-[hsl(var(--muted-foreground))] hover:text-[hsl(var(--foreground))]"}`}>{label}</button>
        ))}
      </div>

      {/* Overview Tab */}
      {activeTab === "overview" && (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <Card>
            <CardHeader className="pb-3"><CardTitle className="text-sm">Configuration</CardTitle></CardHeader>
            <CardContent className="space-y-3 text-sm">
              <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Context Window</span><span className="font-mono text-xs">{model.capabilities ? `${(model.capabilities.max_context_window / 1000).toFixed(0)}k tokens` : "—"}</span></div>
              <Separator />
              <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Max Output</span><span className="font-mono text-xs">{model.capabilities ? `${(model.capabilities.max_output_tokens / 1000).toFixed(0)}k tokens` : "—"}</span></div>
              <Separator />
              <div className="flex justify-between items-start"><span className="text-[hsl(var(--muted-foreground))]">Capabilities</span><div className="flex gap-1 flex-wrap justify-end">{enabledCaps.length > 0 ? enabledCaps.map(([, label]) => <Badge key={label} variant="outline" className="text-[10px]">{label}</Badge>) : <span className="text-[hsl(var(--muted-foreground))] text-xs">none</span>}</div></div>
              {model.specialties && model.specialties.length > 0 && (<><Separator /><div className="flex justify-between items-start"><span className="text-[hsl(var(--muted-foreground))]">Specialties</span><div className="flex gap-1 flex-wrap justify-end">{model.specialties.map(s => <Badge key={s} variant="secondary" className="text-[10px]">{s}</Badge>)}</div></div></>)}
            </CardContent>
          </Card>
          <Card>
            <CardHeader className="pb-3"><CardTitle className="text-sm">Pricing & Routing</CardTitle></CardHeader>
            <CardContent className="space-y-3 text-sm">
              <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Input</span><span className="font-mono text-xs">${model.pricing?.input_per_1m_tokens ?? 0} / 1M tokens</span></div>
              <Separator />
              <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Output</span><span className="font-mono text-xs">${model.pricing?.output_per_1m_tokens ?? 0} / 1M tokens</span></div>
              <Separator />
              <div className="flex justify-between"><span className="text-[hsl(var(--muted-foreground))]">Variants</span><span className="font-medium">{enabledVariants} / {variants.length}</span></div>
              <Separator />
              <Button variant="outline" size="sm" className="w-full" onClick={() => setActiveTab("variants")}><Layers className="h-3.5 w-3.5 mr-1.5" /> Manage Variants</Button>
            </CardContent>
          </Card>
        </div>
      )}

      {/* Variants Tab */}
      {activeTab === "variants" && (
        <div>
          <div className="flex items-center justify-between mb-4">
            <div><h2 className="text-lg font-semibold">Variants</h2><p className="text-sm text-[hsl(var(--muted-foreground))]">Provider routing variants for this model</p></div>
            <Button size="sm" onClick={openNewVariant}><Plus className="h-4 w-4 mr-1" /> New Variant</Button>
          </div>
          <Card>
            <CardContent className="p-0">
              {variants.length === 0 ? (
                <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">No variants yet</div>
              ) : (
                <div className="overflow-x-auto">
                  <table className="w-full text-sm">
                    <thead><tr className="border-b border-[hsl(var(--border))]">
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Variant ID</th>
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Provider</th>
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Account</th>
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Weight</th>
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden md:table-cell">Upstream Model</th>
                      <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden md:table-cell">Base URL</th>
                      <th className="text-right px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Actions</th>
                    </tr></thead>
                    <tbody>
                      {variants.map(v => (
                        <tr key={v.id} className="border-b border-[hsl(var(--border))] last:border-0 hover:bg-[hsl(var(--accent))]/50 transition-colors">
                          <td className="px-4 py-3 font-medium">{v.id}</td>
                          <td className="px-4 py-3"><span className="text-xs">{v.provider_id}</span></td>
                          <td className="px-4 py-3 hidden sm:table-cell"><span className="text-xs text-[hsl(var(--muted-foreground))]">{v.account_id || "default"}</span></td>
                          <td className="px-4 py-3 hidden sm:table-cell"><span className="font-mono text-xs">{v.weight ?? "—"}</span></td>
                          <td className="px-4 py-3 hidden md:table-cell"><span className="text-xs text-[hsl(var(--muted-foreground))]">{v.upstream_model_name || "—"}</span></td>
                          <td className="px-4 py-3"><div className="flex justify-end gap-1">
                            <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => openEditVariant(v)}><Pencil className="h-3.5 w-3.5" /></Button>
                            <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => setDeleteTarget(v.id)}><Trash2 className="h-3.5 w-3.5 text-red-400" /></Button>
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

      {/* Model Edit Dialog */}
      <Dialog open={modelDialogOpen} onOpenChange={setModelDialogOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader><DialogTitle>Edit Model: {modelId}</DialogTitle><DialogDescription>Update model configuration</DialogDescription></DialogHeader>
          <div className="space-y-4">
            <div>
              <Label>Display Name</Label>
              <Input value={modelForm.display_name} onChange={e => setModelForm(f => ({ ...f, display_name: e.target.value }))} placeholder="GPT-4o" />
            </div>
            <div>
              <Label>Quality Tier</Label>
              <Select value={modelForm.quality_tier} onValueChange={v => setModelForm(f => ({ ...f, quality_tier: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {QUALITY_TIERS.map(t => <SelectItem key={t} value={t}>{t}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <Label>Context Window</Label>
                <Input type="number" value={modelForm.capabilities.max_context_window} onChange={e => setModelForm(f => ({ ...f, capabilities: { ...f.capabilities, max_context_window: parseInt(e.target.value) || 0 } }))} />
              </div>
              <div>
                <Label>Max Output Tokens</Label>
                <Input type="number" value={modelForm.capabilities.max_output_tokens} onChange={e => setModelForm(f => ({ ...f, capabilities: { ...f.capabilities, max_output_tokens: parseInt(e.target.value) || 0 } }))} />
              </div>
            </div>
            <div>
              <Label className="mb-2 block">Capabilities</Label>
              <div className="grid grid-cols-2 gap-2">
                {(["supports_streaming", "supports_function_calling", "supports_vision", "supports_json_mode", "supports_reasoning"] as const).map(key => (
                  <label key={key} className="flex items-center gap-2 text-sm cursor-pointer">
                    <Switch checked={modelForm.capabilities[key]} onCheckedChange={() => toggleCap(key)} />
                    {key.replace("supports_", "").replace(/_/g, " ")}
                  </label>
                ))}
              </div>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <Label>Input $/1M tokens</Label>
                <Input type="number" step="0.01" value={modelForm.pricing.input} onChange={e => setModelForm(f => ({ ...f, pricing: { ...f.pricing, input: e.target.value } }))} />
              </div>
              <div>
                <Label>Output $/1M tokens</Label>
                <Input type="number" step="0.01" value={modelForm.pricing.output} onChange={e => setModelForm(f => ({ ...f, pricing: { ...f.pricing, output: e.target.value } }))} />
              </div>
            </div>
            <div>
              <Label className="mb-2 block">Specialties</Label>
              <div className="flex flex-wrap gap-2">
                {SPECIALTIES.map(s => (
                  <button
                    key={s}
                    type="button"
                    onClick={() => toggleSpecialty(s)}
                    className={`text-xs px-2 py-1 rounded cursor-pointer transition-colors ${
                      modelForm.specialties.includes(s)
                        ? "bg-blue-600 text-white"
                        : "bg-[hsl(var(--secondary))] text-[hsl(var(--secondary-foreground))] hover:bg-[hsl(var(--accent))]"
                    }`}
                  >
                    {s}
                  </button>
                ))}
              </div>
            </div>
          </div>
          <DialogFooter><Button variant="outline" onClick={() => setModelDialogOpen(false)}>Cancel</Button><Button onClick={saveModel} disabled={modelSaving}>{modelSaving ? "Saving..." : "Save"}</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Variant Edit Dialog */}
      <Dialog open={variantDialogOpen} onOpenChange={setVariantDialogOpen}>
        <DialogContent>
          <DialogHeader><DialogTitle>{editingVariant ? `Edit Variant: ${editingVariant}` : "New Variant"}</DialogTitle><DialogDescription>{editingVariant ? "Update variant" : `Add variant for ${modelId}`}</DialogDescription></DialogHeader>
          <div className="space-y-4">
            {!editingVariant && (
              <div>
                <Label>ID</Label>
                <Input value={variantForm.id} onChange={e => setVariantForm(f => ({ ...f, id: e.target.value }))} placeholder={`${modelId}@provider`} />
              </div>
            )}
            <div>
              <Label>Provider</Label>
              <Select value={variantForm.provider_id} onValueChange={v => setVariantForm(f => ({ ...f, provider_id: v, account_id: "" }))}>
                <SelectTrigger><SelectValue placeholder="Select provider" /></SelectTrigger>
                <SelectContent>
                  {providers.map(p => <SelectItem key={p.id} value={p.id}>{p.id}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div>
              <Label>Account</Label>
              <Select value={variantForm.account_id} onValueChange={v => setVariantForm(f => ({ ...f, account_id: v === "__none__" ? "" : v }))}>
                <SelectTrigger><SelectValue placeholder="Default (provider key)" /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="__none__">Default (provider key)</SelectItem>
                  {filteredAccounts.map(a => <SelectItem key={a.id} value={a.id}>{a.id}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div>
              <Label>Weight</Label>
              <Input type="number" step="0.1" value={variantForm.weight} onChange={e => setVariantForm(f => ({ ...f, weight: e.target.value }))} />
            </div>
            <div>
              <Label>Upstream Model Name</Label>
              <Input value={variantForm.upstream_model_name} onChange={e => setVariantForm(f => ({ ...f, upstream_model_name: e.target.value }))} placeholder="Same as model id" />
            </div>

          </div>
          <DialogFooter><Button variant="outline" onClick={() => setVariantDialogOpen(false)}>Cancel</Button><Button onClick={saveVariant} disabled={variantSaving}>{variantSaving ? "Saving..." : "Save"}</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog open={!!deleteTarget} onOpenChange={open => { if (!open) setDeleteTarget(null) }}
        title="Delete Variant" description={`Delete variant "${deleteTarget}"? This cannot be undone.`}
        onConfirm={() => { if (deleteTarget) deleteVariant(deleteTarget) }} />
    </div>
  )
}

// ════════════════════════════════════════
// Main Models Page (Table)
// ════════════════════════════════════════

export default function Models() {
  const [models, setModels] = useState<Model[]>([])
  const [variants, setVariants] = useState<{ id: string; model_id: string }[]>([])
  const [loading, setLoading] = useState(true)
  const [selectedModel, setSelectedModel] = useState<string | null>(null)
  const [search, setSearch] = useState("")
  const [dialogOpen, setDialogOpen] = useState(false)
  const [form, setForm] = useState<ModelFormState>(emptyModelForm)
  const [saving, setSaving] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [md, vd] = await Promise.all([
        fetch("/api/models").then(r => r.json()),
        fetch("/api/variants").then(r => r.json()),
      ])
      setModels(md.models || [])
      setVariants(vd.variants || [])
    } catch { /* */ }
    setLoading(false)
  }, [])

  useEffect(() => { load() }, [load])

  const handleNew = () => { setForm(emptyModelForm); setDialogOpen(true) }

  const handleEdit = (m: Model, e: React.MouseEvent) => {
    e.stopPropagation()
    setForm({
      id: m.id,
      display_name: m.display_name,
      quality_tier: m.quality_tier,
      capabilities: m.capabilities || defaultCaps,
      pricing: {
        input: (m.pricing?.input_per_1m_tokens ?? 0).toString(),
        output: (m.pricing?.output_per_1m_tokens ?? 0).toString(),
      },
      specialties: m.specialties || [],
    })
    setDialogOpen(true)
  }

  const handleDelete = async (id: string) => {
    await fetch(`/api/models/${id}`, { method: "DELETE" })
    load()
  }

  const handleSave = async () => {
    setSaving(true)
    const body = {
      id: form.id,
      display_name: form.display_name,
      quality_tier: form.quality_tier,
      capabilities: form.capabilities,
      pricing: {
        input_per_1m_tokens: parseFloat(form.pricing.input) || 0,
        output_per_1m_tokens: parseFloat(form.pricing.output) || 0,
      },
      specialties: form.specialties,
    }
    const isEdit = models.some(m => m.id === form.id)
    const url = isEdit ? `/api/models/${form.id}` : "/api/models"
    const method = isEdit ? "PUT" : "POST"
    await fetch(url, { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    setSaving(false)
    setDialogOpen(false)
    load()
  }

  const toggleSpecialty = (s: string) => {
    setForm(f => ({
      ...f,
      specialties: f.specialties.includes(s) ? f.specialties.filter(x => x !== s) : [...f.specialties, s]
    }))
  }

  const toggleCap = (key: keyof Capabilities) => {
    setForm(f => ({
      ...f,
      capabilities: { ...f.capabilities, [key]: !f.capabilities[key] }
    }))
  }

  if (selectedModel) return <ModelDetail modelId={selectedModel} onBack={() => { setSelectedModel(null); load() }} onUpdated={load} />

  if (loading) return <div className="flex items-center justify-center py-12"><Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" /></div>

  const filtered = search ? models.filter(m => m.display_name.toLowerCase().includes(search.toLowerCase()) || m.id.toLowerCase().includes(search.toLowerCase())) : models

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <div><h1 className="text-xl sm:text-2xl font-bold">Models</h1><p className="text-sm text-[hsl(var(--muted-foreground))]">Manage LLM model definitions</p></div>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> New Model</Button>
      </div>
      <div className="mb-4"><Input placeholder="Search models..." value={search} onChange={e => setSearch(e.target.value)} className="max-w-sm" /></div>
      <Card>
        <CardContent className="p-0">
          {filtered.length === 0 ? (
            <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">{search ? "No models match" : "No models configured"}</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead><tr className="border-b border-[hsl(var(--border))]">
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Model</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Quality Tier</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Context Window</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden md:table-cell">Capabilities</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden md:table-cell">Pricing</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Variants</th>
                  <th className="text-right px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Actions</th>
                </tr></thead>
                <tbody>
                  {filtered.map(m => {
                    const variantCount = variants.filter(v => v.model_id === m.id).length
                    return (
                      <tr key={m.id} className="border-b border-[hsl(var(--border))] last:border-0 hover:bg-[hsl(var(--accent))]/50 cursor-pointer transition-colors" onClick={() => setSelectedModel(m.id)}>
                        <td className="px-4 py-3"><div className="font-medium">{m.display_name}</div><div className="text-xs text-[hsl(var(--muted-foreground))]">{m.id}</div></td>
                        <td className="px-4 py-3"><Badge variant="secondary">{m.quality_tier}</Badge></td>
                        <td className="px-4 py-3 hidden sm:table-cell"><span className="text-xs">{m.capabilities ? `${(m.capabilities.max_context_window / 1000).toFixed(0)}k` : "—"}</span></td>
                        <td className="px-4 py-3 hidden md:table-cell"><div className="flex flex-wrap gap-1">
                          {m.capabilities?.supports_streaming && <Badge variant="outline" className="text-[10px] px-1 py-0">stream</Badge>}
                          {m.capabilities?.supports_vision && <Badge variant="outline" className="text-[10px] px-1 py-0">vision</Badge>}
                          {m.capabilities?.supports_function_calling && <Badge variant="outline" className="text-[10px] px-1 py-0">tools</Badge>}
                          {m.capabilities?.supports_reasoning && <Badge variant="outline" className="text-[10px] px-1 py-0">reasoning</Badge>}
                        </div></td>
                        <td className="px-4 py-3 hidden md:table-cell">{m.pricing && (m.pricing.input_per_1m_tokens > 0 || m.pricing.output_per_1m_tokens > 0) ? <span className="text-xs font-mono">${m.pricing.input_per_1m_tokens}/{m.pricing.output_per_1m_tokens}</span> : <span className="text-xs text-[hsl(var(--muted-foreground))]">—</span>}</td>
                        <td className="px-4 py-3 hidden sm:table-cell"><span className="font-mono text-xs">{variantCount}</span></td>
                        <td className="px-4 py-3" onClick={e => e.stopPropagation()}><div className="flex justify-end gap-1">
                          <Button variant="ghost" size="icon" className="h-8 w-8" onClick={(e) => handleEdit(m, e)}><Pencil className="h-3.5 w-3.5" /></Button>
                          <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => setDeleteTarget(m.id)}><Trash2 className="h-3.5 w-3.5 text-red-400" /></Button>
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

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{models.some(m => m.id === form.id) ? `Edit Model: ${form.id}` : "New Model"}</DialogTitle>
            <DialogDescription>{models.some(m => m.id === form.id) ? "Update model configuration" : "Add a new LLM model"}</DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            {!models.some(m => m.id === form.id) && (
              <div>
                <Label>ID</Label>
                <Input value={form.id} onChange={e => setForm(f => ({ ...f, id: e.target.value }))} placeholder="gpt-4o" />
              </div>
            )}
            <div>
              <Label>Display Name</Label>
              <Input value={form.display_name} onChange={e => setForm(f => ({ ...f, display_name: e.target.value }))} placeholder="GPT-4o" />
            </div>
            <div>
              <Label>Quality Tier</Label>
              <Select value={form.quality_tier} onValueChange={v => setForm(f => ({ ...f, quality_tier: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {QUALITY_TIERS.map(t => <SelectItem key={t} value={t}>{t}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <Label>Context Window</Label>
                <Input type="number" value={form.capabilities.max_context_window} onChange={e => setForm(f => ({ ...f, capabilities: { ...f.capabilities, max_context_window: parseInt(e.target.value) || 0 } }))} />
              </div>
              <div>
                <Label>Max Output Tokens</Label>
                <Input type="number" value={form.capabilities.max_output_tokens} onChange={e => setForm(f => ({ ...f, capabilities: { ...f.capabilities, max_output_tokens: parseInt(e.target.value) || 0 } }))} />
              </div>
            </div>
            <div>
              <Label className="mb-2 block">Capabilities</Label>
              <div className="grid grid-cols-2 gap-2">
                {(["supports_streaming", "supports_function_calling", "supports_vision", "supports_json_mode", "supports_reasoning"] as const).map(key => (
                  <label key={key} className="flex items-center gap-2 text-sm cursor-pointer">
                    <Switch checked={form.capabilities[key]} onCheckedChange={() => toggleCap(key)} />
                    {key.replace("supports_", "").replace(/_/g, " ")}
                  </label>
                ))}
              </div>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <Label>Input $/1M tokens</Label>
                <Input type="number" step="0.01" value={form.pricing.input} onChange={e => setForm(f => ({ ...f, pricing: { ...f.pricing, input: e.target.value } }))} />
              </div>
              <div>
                <Label>Output $/1M tokens</Label>
                <Input type="number" step="0.01" value={form.pricing.output} onChange={e => setForm(f => ({ ...f, pricing: { ...f.pricing, output: e.target.value } }))} />
              </div>
            </div>
            <div>
              <Label className="mb-2 block">Specialties</Label>
              <div className="flex flex-wrap gap-2">
                {SPECIALTIES.map(s => (
                  <button
                    key={s}
                    type="button"
                    onClick={() => toggleSpecialty(s)}
                    className={`text-xs px-2 py-1 rounded cursor-pointer transition-colors ${
                      form.specialties.includes(s)
                        ? "bg-blue-600 text-white"
                        : "bg-[hsl(var(--secondary))] text-[hsl(var(--secondary-foreground))] hover:bg-[hsl(var(--accent))]"
                    }`}
                  >
                    {s}
                  </button>
                ))}
              </div>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDialogOpen(false)}>Cancel</Button>
            <Button onClick={handleSave} disabled={saving}>{saving ? "Saving..." : "Save"}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={open => { if (!open) setDeleteTarget(null) }}
        title="Delete Model"
        description={`Are you sure you want to delete model "${deleteTarget}"? This action cannot be undone.`}
        onConfirm={() => { if (deleteTarget) handleDelete(deleteTarget) }}
      />
    </div>
  )
}
