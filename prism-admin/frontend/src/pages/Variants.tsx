import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from "@/components/ui/select"
import { Badge } from "@/components/ui/badge"
import { Plus, Trash2, Pencil } from "lucide-react"

interface Variant {
  id: string
  model_id: string
  provider_id: string
  weight: number | null
  account_id: string | null
  upstream_model_name: string | null
  base_url: string | null
}

interface FormState {
  id: string
  model_id: string
  provider_id: string
  weight: string
  account_id: string
  upstream_model_name: string
  base_url: string
}

const emptyForm: FormState = {
  id: "", model_id: "", provider_id: "", weight: "1.0", account_id: "", upstream_model_name: "", base_url: "",
}

export default function Variants() {
  const [variants, setVariants] = useState<Variant[]>([])
  const [models, setModels] = useState<{ id: string }[]>([])
  const [providers, setProviders] = useState<{ id: string }[]>([])
  const [accounts, setAccounts] = useState<{ id: string }[]>([])
  const [loading, setLoading] = useState(true)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)

  const load = () => {
    setLoading(true)
    Promise.all([
      fetch("/api/variants").then(r => r.json()),
      fetch("/api/models").then(r => r.json()),
      fetch("/api/providers").then(r => r.json()),
      fetch("/api/accounts").then(r => r.json()),
    ]).then(([vd, md, pd, ad]) => {
      setVariants(vd.variants || [])
      setModels(md.models || [])
      setProviders(pd.providers || [])
      setAccounts(ad.accounts || [])
      setLoading(false)
    }).catch(() => setLoading(false))
  }
  useEffect(load, [])

  const handleNew = () => { setForm(emptyForm); setEditing(null); setDialogOpen(true) }

  const handleEdit = (v: Variant) => {
    setForm({
      id: v.id,
      model_id: v.model_id,
      provider_id: v.provider_id,
      weight: (v.weight ?? 1.0).toString(),
      account_id: v.account_id || "",
      upstream_model_name: v.upstream_model_name || "",
      base_url: v.base_url || "",
    })
    setEditing(v.id)
    setDialogOpen(true)
  }

  const handleDelete = async (id: string) => {
    if (!confirm(`Delete variant "${id}"?`)) return
    await fetch(`/api/variants/${id}`, { method: "DELETE" })
    load()
  }

  const handleSave = async () => {
    setSaving(true)
    const body = {
      id: editing || form.id,
      model_id: form.model_id,
      provider_id: form.provider_id,
      weight: parseFloat(form.weight) || null,
      account_id: form.account_id || null,
      upstream_model_name: form.upstream_model_name || null,
      base_url: form.base_url || null,
    }
    const url = editing ? `/api/variants/${editing}` : "/api/variants"
    const method = editing ? "PUT" : "POST"
    await fetch(url, { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    setSaving(false)
    setDialogOpen(false)
    load()
  }

  if (loading) return <div className="text-[hsl(var(--muted-foreground))]">Loading...</div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <h1 className="text-xl sm:text-2xl font-bold">Variants</h1>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> New Variant</Button>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
        {variants.map(v => (
          <Card key={v.id}>
            <CardContent className="p-4">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <div className="font-medium truncate">{v.id}</div>
                  <div className="text-sm text-[hsl(var(--muted-foreground))]">
                    {v.provider_id} → {v.model_id}
                  </div>
                </div>
                <Badge variant="secondary">w: {v.weight ?? "—"}</Badge>
              </div>
              {v.upstream_model_name && (
                <div className="mt-1 text-xs text-[hsl(var(--muted-foreground))]">upstream: {v.upstream_model_name}</div>
              )}
              {v.base_url && (
                <div className="mt-1 text-xs text-[hsl(var(--muted-foreground))] truncate">url: {v.base_url}</div>
              )}
              <div className="flex gap-1 mt-3 justify-end">
                <Button variant="ghost" size="sm" onClick={() => handleEdit(v)}><Pencil className="h-3 w-3 mr-1" /> Edit</Button>
                <Button variant="ghost" size="sm" onClick={() => handleDelete(v.id)}><Trash2 className="h-3 w-3 mr-1 text-red-400" /> Delete</Button>
              </div>
            </CardContent>
          </Card>
        ))}
        {variants.length === 0 && (
          <div className="col-span-full text-center py-12 text-[hsl(var(--muted-foreground))]">No variants configured</div>
        )}
      </div>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{editing ? `Edit Variant: ${editing}` : "New Variant"}</DialogTitle>
          </DialogHeader>
          <div className="space-y-4">
            {!editing && (
              <div>
                <Label>ID</Label>
                <Input value={form.id} onChange={e => setForm(f => ({ ...f, id: e.target.value }))} placeholder="gpt-4o@openai" />
              </div>
            )}
            <div>
              <Label>Model</Label>
              <Select value={form.model_id} onValueChange={v => setForm(f => ({ ...f, model_id: v }))}>
                <SelectTrigger><SelectValue placeholder="Select model" /></SelectTrigger>
                <SelectContent>
                  {models.map(m => <SelectItem key={m.id} value={m.id}>{m.id}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
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
              <Label>Account</Label>
              <Select value={form.account_id} onValueChange={v => setForm(f => ({ ...f, account_id: v === "__none__" ? "" : v }))}>
                <SelectTrigger><SelectValue placeholder="None (use provider default)" /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="__none__">None</SelectItem>
                  {accounts.map(a => <SelectItem key={a.id} value={a.id}>{a.id}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div>
              <Label>Weight</Label>
              <Input type="number" step="0.1" value={form.weight} onChange={e => setForm(f => ({ ...f, weight: e.target.value }))} />
            </div>
            <div>
              <Label>Upstream Model Name</Label>
              <Input value={form.upstream_model_name} onChange={e => setForm(f => ({ ...f, upstream_model_name: e.target.value }))} placeholder="Optional override" />
            </div>
            <div>
              <Label>Base URL Override</Label>
              <Input value={form.base_url} onChange={e => setForm(f => ({ ...f, base_url: e.target.value }))} placeholder="Optional URL override" />
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
