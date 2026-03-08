import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Badge } from "@/components/ui/badge"
import { Switch } from "@/components/ui/switch"
import { Key, Plus, Trash2, Pencil, X, Shield } from "lucide-react"
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
  max_requests_per_minute: string
  max_tokens_per_day: string
  max_concurrent: string
  allowed_models: string
  metadata: { key: string; value: string }[]
}

const emptyForm: FormState = {
  name: "", key: "", enabled: true,
  max_requests_per_minute: "0", max_tokens_per_day: "0", max_concurrent: "0",
  allowed_models: "", metadata: [],
}

function generateKey(): string {
  const chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
  let result = "sk-"
  for (let i = 0; i < 32; i++) {
    result += chars.charAt(Math.floor(Math.random() * chars.length))
  }
  return result
}

export default function ClientKeys() {
  const [keys, setKeys] = useState<ClientKey[]>([])
  const [loading, setLoading] = useState(true)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)

  const load = () => {
    setLoading(true)
    fetch("/api/client-keys").then(r => r.json()).then(data => {
      setKeys(data.client_keys || [])
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
    const res = await fetch(`/api/client-keys/${encodeURIComponent(name)}`)
    const data: ClientKeyFull = await res.json()
    setForm({
      name: data.name,
      key: data.key,
      enabled: data.enabled,
      max_requests_per_minute: (data.quota?.max_requests_per_minute ?? 0).toString(),
      max_tokens_per_day: (data.quota?.max_tokens_per_day ?? 0).toString(),
      max_concurrent: (data.quota?.max_concurrent ?? 0).toString(),
      allowed_models: (data.allowed_models || []).join(", "),
      metadata: Object.entries(data.metadata || {}).map(([key, value]) => ({ key, value })),
    })
    setEditing(name)
    setDialogOpen(true)
  }

  const handleDelete = async (name: string) => {
    await fetch(`/api/client-keys/${encodeURIComponent(name)}`, { method: "DELETE" })
    load()
  }

  const handleSave = async () => {
    setSaving(true)
    const metadata: Record<string, string> = {}
    form.metadata.forEach(m => { if (m.key) metadata[m.key] = m.value })
    const allowedModels = form.allowed_models.trim()
      ? form.allowed_models.split(",").map(s => s.trim()).filter(Boolean)
      : []
    const body = {
      name: editing || form.name,
      key: form.key,
      enabled: form.enabled,
      allowed_models: allowedModels,
      quota: {
        max_requests_per_minute: parseInt(form.max_requests_per_minute) || 0,
        max_tokens_per_day: parseInt(form.max_tokens_per_day) || 0,
        max_concurrent: parseInt(form.max_concurrent) || 0,
      },
      metadata,
    }
    const url = editing ? `/api/client-keys/${encodeURIComponent(editing)}` : "/api/client-keys"
    const method = editing ? "PUT" : "POST"
    await fetch(url, { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    setSaving(false)
    setDialogOpen(false)
    load()
  }

  const addMeta = () => setForm(f => ({ ...f, metadata: [...f.metadata, { key: "", value: "" }] }))
  const removeMeta = (i: number) => setForm(f => ({ ...f, metadata: f.metadata.filter((_, idx) => idx !== i) }))
  const updateMeta = (i: number, field: "key" | "value", val: string) =>
    setForm(f => ({ ...f, metadata: f.metadata.map((m, idx) => idx === i ? { ...m, [field]: val } : m) }))

  if (loading) return <div className="text-[hsl(var(--muted-foreground))]">Loading...</div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <h1 className="text-xl sm:text-2xl font-bold">Client Keys</h1>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> Add Client Key</Button>
      </div>
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
        {keys.map(k => (
          <Card key={k.name}>
            <CardContent className="p-4">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <Key className="h-4 w-4 text-[hsl(var(--muted-foreground))] shrink-0" />
                    <span className="font-medium text-base sm:text-lg truncate">{k.name}</span>
                    <Badge variant={k.enabled ? "default" : "outline"}>
                      {k.enabled ? "ON" : "OFF"}
                    </Badge>
                  </div>
                  <div className="flex flex-wrap gap-2 mt-2 text-sm text-[hsl(var(--muted-foreground))]">
                    <span className="font-mono">{k.key_masked}</span>
                  </div>
                  <div className="flex flex-wrap gap-2 mt-1 text-sm text-[hsl(var(--muted-foreground))]">
                    <span>{k.quota.max_requests_per_minute} rpm</span>
                    <span>{k.quota.max_tokens_per_day} tok/day</span>
                    <span>{k.quota.max_concurrent} conc</span>
                    {k.allowed_models.length > 0 && (
                      <span className="flex items-center gap-1">
                        <Shield className="h-3 w-3" />
                        {k.allowed_models.length} model{k.allowed_models.length !== 1 ? "s" : ""}
                      </span>
                    )}
                  </div>
                </div>
                <div className="flex gap-1 shrink-0">
                  <Button variant="ghost" size="icon" onClick={() => handleEdit(k.name)}>
                    <Pencil className="h-4 w-4" />
                  </Button>
                  <Button variant="ghost" size="icon" onClick={() => setDeleteTarget(k.name)}>
                    <Trash2 className="h-4 w-4 text-red-400" />
                  </Button>
                </div>
              </div>
            </CardContent>
          </Card>
        ))}
        {keys.length === 0 && (
          <div className="col-span-full text-center py-12 text-[hsl(var(--muted-foreground))]">No client keys configured</div>
        )}
      </div>

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
              <Label className="mb-2 block">Quota</Label>
              <div className="grid grid-cols-3 gap-3">
                <div>
                  <Label className="text-xs text-[hsl(var(--muted-foreground))]">Requests/min</Label>
                  <Input type="number" value={form.max_requests_per_minute} onChange={e => setForm(f => ({ ...f, max_requests_per_minute: e.target.value }))} placeholder="0" />
                </div>
                <div>
                  <Label className="text-xs text-[hsl(var(--muted-foreground))]">Tokens/day</Label>
                  <Input type="number" value={form.max_tokens_per_day} onChange={e => setForm(f => ({ ...f, max_tokens_per_day: e.target.value }))} placeholder="0" />
                </div>
                <div>
                  <Label className="text-xs text-[hsl(var(--muted-foreground))]">Concurrent</Label>
                  <Input type="number" value={form.max_concurrent} onChange={e => setForm(f => ({ ...f, max_concurrent: e.target.value }))} placeholder="0" />
                </div>
              </div>
              <p className="text-xs text-[hsl(var(--muted-foreground))] mt-1">0 = unlimited</p>
            </div>
            <div>
              <Label>Allowed Models</Label>
              <Input
                value={form.allowed_models}
                onChange={e => setForm(f => ({ ...f, allowed_models: e.target.value }))}
                placeholder="Comma-separated (empty = all models)"
              />
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
