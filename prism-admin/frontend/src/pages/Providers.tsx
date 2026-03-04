import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Trash2, Pencil, Plus, ExternalLink, X } from "lucide-react"

interface Provider {
  id: string
  base_url: string
  protocols: string[]
  circuit_breaker_threshold: number
  circuit_breaker_recovery_secs: number
}

interface ProviderForm {
  id: string
  base_url: string
  protocols: string
  api_key: string
  timeout_ms: string
  circuit_breaker_threshold: string
  circuit_breaker_recovery_secs: string
  headers: { key: string; value: string }[]
}

const emptyForm: ProviderForm = {
  id: "", base_url: "", protocols: "openai", api_key: "", timeout_ms: "",
  circuit_breaker_threshold: "5", circuit_breaker_recovery_secs: "30", headers: [],
}

export default function Providers() {
  const [providers, setProviders] = useState<Provider[]>([])
  const [loading, setLoading] = useState(true)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [form, setForm] = useState<ProviderForm>(emptyForm)
  const [saving, setSaving] = useState(false)

  const load = () => {
    setLoading(true)
    fetch("/api/providers")
      .then(r => r.json())
      .then(d => { setProviders(d.providers || []); setLoading(false) })
      .catch(() => setLoading(false))
  }

  useEffect(load, [])

  const handleDelete = async (id: string) => {
    if (!confirm(`Delete provider "${id}"?`)) return
    await fetch(`/api/providers/${id}`, { method: "DELETE" })
    load()
  }

  const handleEdit = async (id: string) => {
    const res = await fetch(`/api/providers/${id}`)
    const data = await res.json()
    setForm({
      id,
      base_url: data.base_url || "",
      protocols: (data.protocols || []).join(", "),
      api_key: data.api_key || "",
      timeout_ms: data.timeout_ms?.toString() || "",
      circuit_breaker_threshold: (data.circuit_breaker_threshold ?? 5).toString(),
      circuit_breaker_recovery_secs: (data.circuit_breaker_recovery_secs ?? 30).toString(),
      headers: Object.entries(data.headers || {}).map(([key, value]) => ({ key, value: value as string })),
    })
    setEditing(id)
    setDialogOpen(true)
  }

  const handleNew = () => {
    setForm(emptyForm)
    setEditing(null)
    setDialogOpen(true)
  }

  const handleSave = async () => {
    setSaving(true)
    const id = editing || form.id
    const headers: Record<string, string> = {}
    form.headers.forEach(h => { if (h.key) headers[h.key] = h.value })
    const body = {
      base_url: form.base_url,
      protocols: form.protocols.split(",").map(s => s.trim()).filter(Boolean),
      api_key: form.api_key || null,
      timeout_ms: form.timeout_ms ? parseInt(form.timeout_ms) : null,
      circuit_breaker_threshold: parseInt(form.circuit_breaker_threshold) || 5,
      circuit_breaker_recovery_secs: parseInt(form.circuit_breaker_recovery_secs) || 30,
      headers,
    }
    await fetch(`/api/providers/${id}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    })
    setSaving(false)
    setDialogOpen(false)
    load()
  }

  const addHeader = () => setForm(f => ({ ...f, headers: [...f.headers, { key: "", value: "" }] }))
  const removeHeader = (i: number) => setForm(f => ({ ...f, headers: f.headers.filter((_, idx) => idx !== i) }))
  const updateHeader = (i: number, field: "key" | "value", val: string) =>
    setForm(f => ({ ...f, headers: f.headers.map((h, idx) => idx === i ? { ...h, [field]: val } : h) }))

  if (loading) return <div className="text-[hsl(var(--muted-foreground))]">Loading...</div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <h1 className="text-xl sm:text-2xl font-bold">Providers</h1>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> New Provider</Button>
      </div>
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
        {providers.map(p => (
          <Card key={p.id}>
            <CardContent className="p-4">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0 flex-1">
                  <div className="font-medium text-base sm:text-lg">{p.id}</div>
                  <div className="text-sm text-[hsl(var(--muted-foreground))] flex items-center gap-1 truncate">
                    <ExternalLink className="h-3 w-3 shrink-0" />
                    <span className="truncate">{p.base_url}</span>
                  </div>
                  <div className="flex flex-wrap gap-1.5 mt-2">
                    {p.protocols.map(proto => (
                      <span key={proto} className="text-xs bg-[hsl(var(--secondary))] px-2 py-0.5 rounded">{proto}</span>
                    ))}
                    <span className="text-xs text-[hsl(var(--muted-foreground))]">
                      CB: {p.circuit_breaker_threshold}/{p.circuit_breaker_recovery_secs}s
                    </span>
                  </div>
                </div>
                <div className="flex gap-1 shrink-0">
                  <Button variant="ghost" size="icon" onClick={() => handleEdit(p.id)}>
                    <Pencil className="h-4 w-4" />
                  </Button>
                  <Button variant="ghost" size="icon" onClick={() => handleDelete(p.id)}>
                    <Trash2 className="h-4 w-4 text-red-400" />
                  </Button>
                </div>
              </div>
            </CardContent>
          </Card>
        ))}
        {providers.length === 0 && (
          <div className="col-span-full text-center py-12 text-[hsl(var(--muted-foreground))]">No providers configured</div>
        )}
      </div>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{editing ? `Edit Provider: ${editing}` : "New Provider"}</DialogTitle>
          </DialogHeader>
          <div className="space-y-4">
            {!editing && (
              <div>
                <Label>ID</Label>
                <Input value={form.id} onChange={e => setForm(f => ({ ...f, id: e.target.value }))} placeholder="openai" />
              </div>
            )}
            <div>
              <Label>Base URL</Label>
              <Input value={form.base_url} onChange={e => setForm(f => ({ ...f, base_url: e.target.value }))} placeholder="https://api.openai.com/v1" />
            </div>
            <div>
              <Label>Protocols (comma-separated)</Label>
              <Input value={form.protocols} onChange={e => setForm(f => ({ ...f, protocols: e.target.value }))} placeholder="openai, anthropic" />
            </div>
            <div>
              <Label>API Key</Label>
              <Input type="password" value={form.api_key} onChange={e => setForm(f => ({ ...f, api_key: e.target.value }))} placeholder="sk-..." />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <Label>Timeout (ms)</Label>
                <Input type="number" value={form.timeout_ms} onChange={e => setForm(f => ({ ...f, timeout_ms: e.target.value }))} placeholder="30000" />
              </div>
              <div>
                <Label>CB Threshold</Label>
                <Input type="number" value={form.circuit_breaker_threshold} onChange={e => setForm(f => ({ ...f, circuit_breaker_threshold: e.target.value }))} />
              </div>
            </div>
            <div>
              <Label>CB Recovery (secs)</Label>
              <Input type="number" value={form.circuit_breaker_recovery_secs} onChange={e => setForm(f => ({ ...f, circuit_breaker_recovery_secs: e.target.value }))} />
            </div>
            <div>
              <div className="flex items-center justify-between mb-2">
                <Label>Headers</Label>
                <Button variant="ghost" size="sm" onClick={addHeader}><Plus className="h-3 w-3 mr-1" /> Add</Button>
              </div>
              {form.headers.map((h, i) => (
                <div key={i} className="flex gap-2 mb-2">
                  <Input placeholder="Key" value={h.key} onChange={e => updateHeader(i, "key", e.target.value)} className="flex-1" />
                  <Input placeholder="Value" value={h.value} onChange={e => updateHeader(i, "value", e.target.value)} className="flex-1" />
                  <Button variant="ghost" size="icon" onClick={() => removeHeader(i)}><X className="h-4 w-4" /></Button>
                </div>
              ))}
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
