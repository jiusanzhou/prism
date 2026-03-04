import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Badge } from "@/components/ui/badge"
import { Plus, Trash2, Pencil } from "lucide-react"

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

interface FormState {
  id: string
  display_name: string
  quality_tier: string
  capabilities: Capabilities
  pricing: { input: string; output: string }
  specialties: string[]
}

const emptyForm: FormState = {
  id: "", display_name: "", quality_tier: "A",
  capabilities: defaultCaps,
  pricing: { input: "0", output: "0" },
  specialties: [],
}

export default function Models() {
  const [models, setModels] = useState<Model[]>([])
  const [loading, setLoading] = useState(true)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)

  const load = () => {
    setLoading(true)
    fetch("/api/models").then(r => r.json()).then(d => { setModels(d.models || []); setLoading(false) }).catch(() => setLoading(false))
  }
  useEffect(load, [])

  const handleNew = () => { setForm(emptyForm); setEditing(null); setDialogOpen(true) }

  const handleEdit = (m: Model) => {
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
    setEditing(m.id)
    setDialogOpen(true)
  }

  const handleDelete = async (id: string) => {
    if (!confirm(`Delete model "${id}"?`)) return
    await fetch(`/api/models/${id}`, { method: "DELETE" })
    load()
  }

  const handleSave = async () => {
    setSaving(true)
    const body = {
      id: editing || form.id,
      display_name: form.display_name,
      quality_tier: form.quality_tier,
      capabilities: form.capabilities,
      pricing: {
        input_per_1m_tokens: parseFloat(form.pricing.input) || 0,
        output_per_1m_tokens: parseFloat(form.pricing.output) || 0,
      },
      specialties: form.specialties,
    }
    const url = editing ? `/api/models/${editing}` : "/api/models"
    const method = editing ? "PUT" : "POST"
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

  if (loading) return <div className="text-[hsl(var(--muted-foreground))]">Loading...</div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <h1 className="text-xl sm:text-2xl font-bold">Models</h1>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> New Model</Button>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
        {models.map(m => (
          <Card key={m.id}>
            <CardContent className="p-4">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <div className="font-medium">{m.display_name}</div>
                  <div className="text-sm text-[hsl(var(--muted-foreground))]">{m.id}</div>
                </div>
                <Badge variant="secondary">{m.quality_tier}</Badge>
              </div>
              {m.capabilities && (
                <div className="mt-2 flex flex-wrap gap-1">
                  <span className="text-xs text-[hsl(var(--muted-foreground))]">
                    {(m.capabilities.max_context_window / 1000).toFixed(0)}k ctx
                  </span>
                  {m.capabilities.supports_streaming && <Badge variant="outline" className="text-[10px] px-1 py-0">stream</Badge>}
                  {m.capabilities.supports_vision && <Badge variant="outline" className="text-[10px] px-1 py-0">vision</Badge>}
                  {m.capabilities.supports_function_calling && <Badge variant="outline" className="text-[10px] px-1 py-0">tools</Badge>}
                  {m.capabilities.supports_reasoning && <Badge variant="outline" className="text-[10px] px-1 py-0">reasoning</Badge>}
                </div>
              )}
              {m.pricing && (m.pricing.input_per_1m_tokens > 0 || m.pricing.output_per_1m_tokens > 0) && (
                <div className="mt-1 text-xs text-[hsl(var(--muted-foreground))]">
                  ${m.pricing.input_per_1m_tokens}/{m.pricing.output_per_1m_tokens} per 1M tok
                </div>
              )}
              {m.specialties && m.specialties.length > 0 && (
                <div className="mt-2 flex flex-wrap gap-1">
                  {m.specialties.map(s => <Badge key={s} variant="secondary" className="text-[10px]">{s}</Badge>)}
                </div>
              )}
              <div className="flex gap-1 mt-3 justify-end">
                <Button variant="ghost" size="sm" onClick={() => handleEdit(m)}><Pencil className="h-3 w-3 mr-1" /> Edit</Button>
                <Button variant="ghost" size="sm" onClick={() => handleDelete(m.id)}><Trash2 className="h-3 w-3 mr-1 text-red-400" /> Delete</Button>
              </div>
            </CardContent>
          </Card>
        ))}
        {models.length === 0 && (
          <div className="col-span-full text-center py-12 text-[hsl(var(--muted-foreground))]">No models configured</div>
        )}
      </div>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{editing ? `Edit Model: ${editing}` : "New Model"}</DialogTitle>
          </DialogHeader>
          <div className="space-y-4 max-h-[60vh] overflow-y-auto pr-2">
            {!editing && (
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
    </div>
  )
}
