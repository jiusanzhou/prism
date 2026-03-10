import { useEffect, useState, useRef } from "react"
import { api } from "@/lib/api"
import { usePageTitle } from "@/hooks/usePageTitle"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Badge } from "@/components/ui/badge"
import { Switch } from "@/components/ui/switch"
import { Plus, Trash2, Pencil, X, Shield, Loader2, ChevronDown } from "lucide-react"
import { ConfirmDialog } from "@/components/ui/confirm-dialog"

interface ClientKey {
  name: string
  key_masked: string
  enabled: boolean
  allowed_models: string[]
  quota: { max_requests_per_minute: number; max_tokens_per_day: number; max_concurrent: number }
  metadata: Record<string, string>
}

interface ClientKeyFull {
  key: string
  name: string
  enabled: boolean
  allowed_models: string[]
  quota: { max_requests_per_minute: number; max_tokens_per_day: number; max_concurrent: number }
  metadata: Record<string, string>
}

interface FormState {
  name: string
  key: string
  enabled: boolean
  allowed_models: string[]
  metadata: { key: string; value: string }[]
}

const emptyForm: FormState = {
  name: "", key: "", enabled: true,
  allowed_models: [], metadata: [],
}

function generateKey(): string {
  const chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
  let result = "sk-"
  for (let i = 0; i < 32; i++) {
    result += chars.charAt(Math.floor(Math.random() * chars.length))
  }
  return result
}

// ── Model Selector with wildcard/regex support ──
function ModelAllowSelector({ value, onChange, availableModels }: {
  value: string[]
  onChange: (v: string[]) => void
  availableModels: string[]
}) {
  const [open, setOpen] = useState(false)
  const [customInput, setCustomInput] = useState("")
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const h = (e: MouseEvent) => { if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false) }
    document.addEventListener("mousedown", h); return () => document.removeEventListener("mousedown", h)
  }, [open])

  const remove = (m: string) => onChange(value.filter(x => x !== m))
  const addModel = (m: string) => { if (!value.includes(m)) onChange([...value, m]) }
  const addCustom = () => {
    const t = customInput.trim()
    if (t && !value.includes(t)) onChange([...value, t])
    setCustomInput("")
  }
  const unselected = availableModels.filter(m => !value.includes(m))

  return (
    <div ref={ref} className="relative">
      <div
        className="flex flex-wrap items-center gap-1.5 min-h-[36px] w-full rounded-md border border-[hsl(var(--input))] bg-transparent px-3 py-1.5 text-sm cursor-pointer"
        onClick={() => setOpen(!open)}
      >
        {value.length === 0 && <span className="text-[hsl(var(--muted-foreground))]">All models (click to restrict)</span>}
        {value.map(m => (
          <Badge key={m} variant="secondary" className="gap-1 pr-1">
            {m.includes("*") || m.includes("?") ? <span className="italic">{m}</span> : m}
            <button type="button" className="ml-0.5 rounded-full hover:bg-[hsl(var(--muted-foreground)/0.2)] p-0.5" onClick={e => { e.stopPropagation(); remove(m) }}>
              <X className="h-3 w-3" />
            </button>
          </Badge>
        ))}
        <ChevronDown className="h-4 w-4 ml-auto shrink-0 text-[hsl(var(--muted-foreground))]" />
      </div>
      {open && (
        <div className="absolute z-50 mt-1 w-full rounded-md border border-[hsl(var(--border))] bg-[hsl(var(--popover))] text-[hsl(var(--popover-foreground))] shadow-md">
          <div className="max-h-48 overflow-y-auto p-1">
            {unselected.length > 0 ? unselected.map(m => (
              <button key={m} type="button" className="w-full text-left px-3 py-1.5 text-sm rounded hover:bg-[hsl(var(--accent))]" onClick={() => addModel(m)}>{m}</button>
            )) : (
              <div className="px-3 py-1.5 text-sm text-[hsl(var(--muted-foreground))]">All models selected</div>
            )}
          </div>
          <div className="border-t border-[hsl(var(--border))] p-2">
            <p className="text-xs text-[hsl(var(--muted-foreground))] mb-1.5">Wildcard patterns: * matches any, ? matches one char</p>
            <div className="flex gap-2">
              <Input
                placeholder="gpt-4*, claude-*..."
                value={customInput}
                onChange={e => setCustomInput(e.target.value)}
                onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); addCustom() } }}
                className="h-8 text-sm" onClick={e => e.stopPropagation()}
              />
              <Button size="sm" variant="outline" className="h-8 shrink-0" onClick={addCustom} disabled={!customInput.trim()}>Add</Button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

export default function ClientKeys() {
  usePageTitle("Client Keys")
  const [keys, setKeys] = useState<ClientKey[]>([])
  const [models, setModels] = useState<string[]>([])
  const [loading, setLoading] = useState(true)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)

  const load = () => {
    setLoading(true)
    Promise.all([
      api("/api/client-keys").then(r => r.json()),
      api("/api/models").then(r => r.json()),
    ]).then(([kd, md]) => {
      setKeys(kd.client_keys || [])
      setModels((md.models || []).map((m: { id: string }) => m.id))
      setLoading(false)
    }).catch(() => setLoading(false))
  }
  useEffect(load, [])

  const handleNew = () => {
    setForm({ ...emptyForm, key: generateKey() })
    setEditing(null)
    setDialogOpen(true)
  }

  const handleEdit = async (name: string) => {
    const res = await api(`/api/client-keys/${encodeURIComponent(name)}`)
    const data: ClientKeyFull = await res.json()
    setForm({
      name: data.name,
      key: data.key,
      enabled: data.enabled,
      allowed_models: data.allowed_models || [],
      metadata: Object.entries(data.metadata || {}).map(([key, value]) => ({ key, value })),
    })
    setEditing(name)
    setDialogOpen(true)
  }

  const handleDelete = async (name: string) => {
    await api(`/api/client-keys/${encodeURIComponent(name)}`, { method: "DELETE" })
    load()
  }

  const handleSave = async () => {
    setSaving(true)
    const metadata: Record<string, string> = {}
    form.metadata.forEach(m => { if (m.key) metadata[m.key] = m.value })
    const body = {
      name: editing || form.name,
      key: form.key,
      enabled: form.enabled,
      allowed_models: form.allowed_models,
      quota: {
        max_requests_per_minute: 0,
        max_tokens_per_day: 0,
        max_concurrent: 0,
      },
      metadata,
    }
    const url = editing ? `/api/client-keys/${encodeURIComponent(editing)}` : "/api/client-keys"
    const method = editing ? "PUT" : "POST"
    await api(url, { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    setSaving(false)
    setDialogOpen(false)
    load()
  }

  const addMeta = () => setForm(f => ({ ...f, metadata: [...f.metadata, { key: "", value: "" }] }))
  const removeMeta = (i: number) => setForm(f => ({ ...f, metadata: f.metadata.filter((_, idx) => idx !== i) }))
  const updateMeta = (i: number, field: "key" | "value", val: string) =>
    setForm(f => ({ ...f, metadata: f.metadata.map((m, idx) => idx === i ? { ...m, [field]: val } : m) }))

  if (loading) return <div className="flex items-center justify-center py-12"><Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" /></div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <div>
          <h1 className="text-xl sm:text-2xl font-bold">Client Keys</h1>
          <p className="text-sm text-[hsl(var(--muted-foreground))]">API key management</p>
        </div>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> Add Client Key</Button>
      </div>
      <Card>
        <CardContent className="p-0">
          {keys.length === 0 ? (
            <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">No client keys configured</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead><tr className="border-b border-[hsl(var(--border))]">
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Name</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Key</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Status</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Allowed Models</th>
                  <th className="text-right px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Actions</th>
                </tr></thead>
                <tbody>
                  {keys.map(k => (
                    <tr key={k.name} className="border-b border-[hsl(var(--border))] last:border-0 hover:bg-[hsl(var(--accent))]/50 transition-colors">
                      <td className="px-4 py-3 font-medium">{k.name}</td>
                      <td className="px-4 py-3"><span className="font-mono text-xs text-[hsl(var(--muted-foreground))]">{k.key_masked}</span></td>
                      <td className="px-4 py-3"><Badge variant={k.enabled ? "default" : "outline"}>{k.enabled ? "ON" : "OFF"}</Badge></td>
                      <td className="px-4 py-3 hidden sm:table-cell">
                        {k.allowed_models.length > 0 ? (
                          <div className="flex items-center gap-1.5 flex-wrap">
                            <Shield className="h-3 w-3 text-[hsl(var(--muted-foreground))] shrink-0" />
                            {k.allowed_models.slice(0, 3).map(m => (
                              <Badge key={m} variant="outline" className="text-[10px]">{m}</Badge>
                            ))}
                            {k.allowed_models.length > 3 && (
                              <span className="text-xs text-[hsl(var(--muted-foreground))]">+{k.allowed_models.length - 3}</span>
                            )}
                          </div>
                        ) : (
                          <span className="text-xs text-[hsl(var(--muted-foreground))] italic">all</span>
                        )}
                      </td>
                      <td className="px-4 py-3">
                        <div className="flex justify-end gap-1">
                          <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => handleEdit(k.name)}>
                            <Pencil className="h-3.5 w-3.5" />
                          </Button>
                          <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => setDeleteTarget(k.name)}>
                            <Trash2 className="h-3.5 w-3.5 text-red-400" />
                          </Button>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{editing ? `Edit Client Key: ${editing}` : "New Client Key"}</DialogTitle>
          </DialogHeader>
          <div className="space-y-4">
            {!editing && (
              <div>
                <Label>Name</Label>
                <Input value={form.name} onChange={e => setForm(f => ({ ...f, name: e.target.value }))} placeholder="team-alpha" />
              </div>
            )}
            <div>
              <Label>API Key</Label>
              <div className="flex gap-2">
                <Input
                  value={form.key}
                  onChange={e => setForm(f => ({ ...f, key: e.target.value }))}
                  placeholder="sk-..."
                  className="flex-1 font-mono text-sm"
                />
                <Button variant="outline" size="sm" type="button" onClick={() => setForm(f => ({ ...f, key: generateKey() }))}>
                  Generate
                </Button>
              </div>
            </div>
            <div className="flex items-center gap-2">
              <Switch checked={form.enabled} onCheckedChange={v => setForm(f => ({ ...f, enabled: v }))} />
              <Label>Enabled</Label>
            </div>
            <div>
              <Label className="mb-2 block">Allowed Models</Label>
              <ModelAllowSelector
                value={form.allowed_models}
                onChange={allowed_models => setForm(f => ({ ...f, allowed_models }))}
                availableModels={models}
              />
              <p className="text-xs text-[hsl(var(--muted-foreground))] mt-1">Empty = all models. Supports wildcard patterns (e.g. gpt-4*, claude-*).</p>
            </div>
            <div>
              <div className="flex items-center justify-between mb-2">
                <Label>Metadata</Label>
                <Button variant="ghost" size="sm" onClick={addMeta}><Plus className="h-3 w-3 mr-1" /> Add</Button>
              </div>
              {form.metadata.map((m, i) => (
                <div key={i} className="flex gap-2 mb-2">
                  <Input placeholder="Key" value={m.key} onChange={e => updateMeta(i, "key", e.target.value)} className="flex-1" />
                  <Input placeholder="Value" value={m.value} onChange={e => updateMeta(i, "value", e.target.value)} className="flex-1" />
                  <Button variant="ghost" size="icon" onClick={() => removeMeta(i)}><X className="h-4 w-4" /></Button>
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

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={open => { if (!open) setDeleteTarget(null) }}
        title="Delete Client Key"
        description={`Are you sure you want to delete client key "${deleteTarget}"? This action cannot be undone.`}
        onConfirm={() => { if (deleteTarget) handleDelete(deleteTarget) }}
      />
    </div>
  )
}
