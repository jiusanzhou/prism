import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from "@/components/ui/select"
import { Plus, Trash2, Pencil, Loader2 } from "lucide-react"
import { ConfirmDialog } from "@/components/ui/confirm-dialog"

interface Variant {
  id: string
  model_id: string
  provider_id: string
  weight: number | null
  account_id: string | null
  upstream_model_name: string | null
  base_url: string | null
}

interface ModelItem { id: string; display_name: string }
interface ProviderItem { id: string }
interface AccountItem { id: string }

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
  const [models, setModels] = useState<ModelItem[]>([])
  const [providers, setProviders] = useState<ProviderItem[]>([])
  const [accounts, setAccounts] = useState<AccountItem[]>([])
  const [loading, setLoading] = useState(true)
  const [search, setSearch] = useState("")
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)

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

  const modelName = (id: string) => models.find(m => m.id === id)?.display_name || id

  if (loading) return <div className="flex items-center justify-center py-12"><Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" /></div>

  const filtered = search ? variants.filter(v => v.id.toLowerCase().includes(search.toLowerCase()) || v.model_id.toLowerCase().includes(search.toLowerCase()) || v.provider_id.toLowerCase().includes(search.toLowerCase())) : variants

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <div><h1 className="text-xl sm:text-2xl font-bold">Variants</h1><p className="text-sm text-[hsl(var(--muted-foreground))]">Model-provider routing variants</p></div>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> New Variant</Button>
      </div>
      <div className="mb-4"><Input placeholder="Search variants..." value={search} onChange={e => setSearch(e.target.value)} className="max-w-sm" /></div>
      <Card>
        <CardContent className="p-0">
          {filtered.length === 0 ? (
            <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">{search ? "No variants match" : "No variants configured"}</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead><tr className="border-b border-[hsl(var(--border))]">
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Variant ID</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Model</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Provider</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Account</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Weight</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden md:table-cell">Upstream Model</th>
                  <th className="text-right px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Actions</th>
                </tr></thead>
                <tbody>
                  {filtered.map(v => (
                    <tr key={v.id} className="border-b border-[hsl(var(--border))] last:border-0 hover:bg-[hsl(var(--accent))]/50 transition-colors">
                      <td className="px-4 py-3 font-medium">{v.id}</td>
                      <td className="px-4 py-3"><span className="text-xs">{modelName(v.model_id)}</span></td>
                      <td className="px-4 py-3"><span className="text-xs">{v.provider_id}</span></td>
                      <td className="px-4 py-3 hidden sm:table-cell"><span className="text-xs text-[hsl(var(--muted-foreground))]">{v.account_id || "—"}</span></td>
                      <td className="px-4 py-3 hidden sm:table-cell"><span className="font-mono text-xs">{v.weight ?? "—"}</span></td>
                      <td className="px-4 py-3 hidden md:table-cell"><span className="text-xs text-[hsl(var(--muted-foreground))]">{v.upstream_model_name || "—"}</span></td>
                      <td className="px-4 py-3"><div className="flex justify-end gap-1">
                        <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => handleEdit(v)}><Pencil className="h-3.5 w-3.5" /></Button>
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

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={open => { if (!open) setDeleteTarget(null) }}
        title="Delete Variant"
        description={`Are you sure you want to delete variant "${deleteTarget}"? This action cannot be undone.`}
        onConfirm={() => { if (deleteTarget) handleDelete(deleteTarget) }}
      />
    </div>
  )
}
