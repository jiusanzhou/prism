import { useEffect, useState } from "react"
import { api } from "@/lib/api"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from "@/components/ui/select"
import { Badge } from "@/components/ui/badge"
import { Switch } from "@/components/ui/switch"
import { Plus, Trash2, Pencil, X, Eye, EyeOff } from "lucide-react"
import { ConfirmDialog } from "@/components/ui/confirm-dialog"

interface Account {
  id: string
  provider_id: string
  billing_tag: string | null
  enabled: boolean
  weight: number
  rate_limit: { rpm: number | null; tpm: number | null; concurrent: number | null } | null
}

interface AccountFull {
  id: string
  provider_id: string
  api_key: string | null
  headers: Record<string, string>
  project_id: string | null
  billing_tag: string | null
  weight: number
  enabled: boolean
  rate_limit: { rpm: number | null; tpm: number | null; concurrent: number | null } | null
  metadata: Record<string, string>
}

interface FormState {
  id: string
  provider_id: string
  api_key: string
  headers: { key: string; value: string }[]
  project_id: string
  billing_tag: string
  weight: string
  enabled: boolean
  rpm: string
  tpm: string
  concurrent: string
  metadata: { key: string; value: string }[]
}

const emptyForm: FormState = {
  id: "", provider_id: "", api_key: "", headers: [], project_id: "",
  billing_tag: "", weight: "1.0", enabled: true, rpm: "", tpm: "", concurrent: "", metadata: [],
}

export default function Accounts() {
  const [accounts, setAccounts] = useState<Account[]>([])
  const [providers, setProviders] = useState<{ id: string }[]>([])
  const [loading, setLoading] = useState(true)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)
  const [showKey, setShowKey] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)

  const load = () => {
    setLoading(true)
    Promise.all([
      api("/api/accounts").then(r => r.json()),
      api("/api/providers").then(r => r.json()),
    ]).then(([ad, pd]) => {
      setAccounts(ad.accounts || [])
      setProviders(pd.providers || [])
      setLoading(false)
    }).catch(() => setLoading(false))
  }
  useEffect(load, [])

  const handleNew = () => { setForm(emptyForm); setEditing(null); setShowKey(false); setDialogOpen(true) }

  const handleEdit = async (id: string) => {
    const res = await api(`/api/accounts/${id}`)
    const data: AccountFull = await res.json()
    setForm({
      id: data.id,
      provider_id: data.provider_id,
      api_key: data.api_key || "",
      headers: Object.entries(data.headers || {}).map(([key, value]) => ({ key, value })),
      project_id: data.project_id || "",
      billing_tag: data.billing_tag || "",
      weight: (data.weight ?? 1.0).toString(),
      enabled: data.enabled,
      rpm: data.rate_limit?.rpm?.toString() || "",
      tpm: data.rate_limit?.tpm?.toString() || "",
      concurrent: data.rate_limit?.concurrent?.toString() || "",
      metadata: Object.entries(data.metadata || {}).map(([key, value]) => ({ key, value })),
    })
    setEditing(id)
    setShowKey(false)
    setDialogOpen(true)
  }

  const handleDelete = async (id: string) => {
    await api(`/api/accounts/${id}`, { method: "DELETE" })
    load()
  }

  const handleSave = async () => {
    setSaving(true)
    const headers: Record<string, string> = {}
    form.headers.forEach(h => { if (h.key) headers[h.key] = h.value })
    const metadata: Record<string, string> = {}
    form.metadata.forEach(m => { if (m.key) metadata[m.key] = m.value })
    const hasRateLimit = form.rpm || form.tpm || form.concurrent
    const body = {
      id: editing || form.id,
      provider_id: form.provider_id,
      api_key: form.api_key || null,
      headers,
      project_id: form.project_id || null,
      billing_tag: form.billing_tag || null,
      weight: parseFloat(form.weight) || 1.0,
      enabled: form.enabled,
      rate_limit: hasRateLimit ? {
        rpm: form.rpm ? parseInt(form.rpm) : null,
        tpm: form.tpm ? parseInt(form.tpm) : null,
        concurrent: form.concurrent ? parseInt(form.concurrent) : null,
      } : null,
      metadata,
    }
    const url = editing ? `/api/accounts/${editing}` : "/api/accounts"
    const method = editing ? "PUT" : "POST"
    await api(url, { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    setSaving(false)
    setDialogOpen(false)
    load()
  }

  const addHeader = () => setForm(f => ({ ...f, headers: [...f.headers, { key: "", value: "" }] }))
  const removeHeader = (i: number) => setForm(f => ({ ...f, headers: f.headers.filter((_, idx) => idx !== i) }))
  const updateHeader = (i: number, field: "key" | "value", val: string) =>
    setForm(f => ({ ...f, headers: f.headers.map((h, idx) => idx === i ? { ...h, [field]: val } : h) }))

  const addMeta = () => setForm(f => ({ ...f, metadata: [...f.metadata, { key: "", value: "" }] }))
  const removeMeta = (i: number) => setForm(f => ({ ...f, metadata: f.metadata.filter((_, idx) => idx !== i) }))
  const updateMeta = (i: number, field: "key" | "value", val: string) =>
    setForm(f => ({ ...f, metadata: f.metadata.map((m, idx) => idx === i ? { ...m, [field]: val } : m) }))

  if (loading) return <div className="text-[hsl(var(--muted-foreground))]">Loading...</div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <h1 className="text-xl sm:text-2xl font-bold">Accounts</h1>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> New Account</Button>
      </div>
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
        {accounts.map(a => (
          <Card key={a.id}>
            <CardContent className="p-4">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="font-medium text-base sm:text-lg truncate">{a.id}</span>
                    <Badge variant="secondary">{a.provider_id}</Badge>
                    <Badge variant={a.enabled ? "default" : "outline"}>
                      {a.enabled ? "ON" : "OFF"}
                    </Badge>
                  </div>
                  <div className="flex flex-wrap gap-2 mt-2 text-sm text-[hsl(var(--muted-foreground))]">
                    {a.billing_tag && <span>billing: {a.billing_tag}</span>}
                    <span>weight: {a.weight}</span>
                    {a.rate_limit && (
                      <span>
                        limits: {[
                          a.rate_limit.rpm != null && `${a.rate_limit.rpm} rpm`,
                          a.rate_limit.tpm != null && `${a.rate_limit.tpm} tpm`,
                          a.rate_limit.concurrent != null && `${a.rate_limit.concurrent} conc`,
                        ].filter(Boolean).join(", ") || "—"}
                      </span>
                    )}
                  </div>
                </div>
                <div className="flex gap-1 shrink-0">
                  <Button variant="ghost" size="icon" onClick={() => handleEdit(a.id)}>
                    <Pencil className="h-4 w-4" />
                  </Button>
                  <Button variant="ghost" size="icon" onClick={() => setDeleteTarget(a.id)}>
                    <Trash2 className="h-4 w-4 text-red-400" />
                  </Button>
                </div>
              </div>
            </CardContent>
          </Card>
        ))}
        {accounts.length === 0 && (
          <div className="col-span-full text-center py-12 text-[hsl(var(--muted-foreground))]">No accounts configured</div>
        )}
      </div>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{editing ? `Edit Account: ${editing}` : "New Account"}</DialogTitle>
          </DialogHeader>
          <div className="space-y-4">
            {!editing && (
              <div>
                <Label>ID</Label>
                <Input value={form.id} onChange={e => setForm(f => ({ ...f, id: e.target.value }))} placeholder="acct-openai-1" />
              </div>
            )}
            <div>
              <Label>Provider</Label>
              <Select value={form.provider_id} onValueChange={v => setForm(f => ({ ...f, provider_id: v }))}>
                <SelectTrigger><SelectValue placeholder="Select provider" /></SelectTrigger>
                <SelectContent>
                  {providers.map(p => <SelectItem key={p.id} value={p.id}>{p.id}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div>
              <Label>API Key</Label>
              <div className="flex gap-2">
                <Input
                  type={showKey ? "text" : "password"}
                  value={form.api_key}
                  onChange={e => setForm(f => ({ ...f, api_key: e.target.value }))}
                  placeholder="sk-..."
                  className="flex-1"
                />
                <Button variant="ghost" size="icon" type="button" onClick={() => setShowKey(v => !v)}>
                  {showKey ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                </Button>
              </div>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <Label>Project ID</Label>
                <Input value={form.project_id} onChange={e => setForm(f => ({ ...f, project_id: e.target.value }))} placeholder="Optional" />
              </div>
              <div>
                <Label>Billing Tag</Label>
                <Input value={form.billing_tag} onChange={e => setForm(f => ({ ...f, billing_tag: e.target.value }))} placeholder="Optional" />
              </div>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <Label>Weight</Label>
                <Input type="number" step="0.1" value={form.weight} onChange={e => setForm(f => ({ ...f, weight: e.target.value }))} />
              </div>
              <div className="flex items-end pb-2">
                <div className="flex items-center gap-2">
                  <Switch checked={form.enabled} onCheckedChange={v => setForm(f => ({ ...f, enabled: v }))} />
                  <Label>Enabled</Label>
                </div>
              </div>
            </div>
            <div>
              <Label className="mb-2 block">Rate Limits</Label>
              <div className="grid grid-cols-3 gap-3">
                <div>
                  <Label className="text-xs text-[hsl(var(--muted-foreground))]">RPM</Label>
                  <Input type="number" value={form.rpm} onChange={e => setForm(f => ({ ...f, rpm: e.target.value }))} placeholder="—" />
                </div>
                <div>
                  <Label className="text-xs text-[hsl(var(--muted-foreground))]">TPM</Label>
                  <Input type="number" value={form.tpm} onChange={e => setForm(f => ({ ...f, tpm: e.target.value }))} placeholder="—" />
                </div>
                <div>
                  <Label className="text-xs text-[hsl(var(--muted-foreground))]">Concurrent</Label>
                  <Input type="number" value={form.concurrent} onChange={e => setForm(f => ({ ...f, concurrent: e.target.value }))} placeholder="—" />
                </div>
              </div>
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
        title="Delete Account"
        description={`Are you sure you want to delete account "${deleteTarget}"? This action cannot be undone.`}
        onConfirm={() => { if (deleteTarget) handleDelete(deleteTarget) }}
      />
    </div>
  )
}
