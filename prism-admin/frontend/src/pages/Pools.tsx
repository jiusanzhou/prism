import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from "@/components/ui/select"
import { Badge } from "@/components/ui/badge"
import { Plus, Trash2, Pencil } from "lucide-react"

interface Pool {
  id: string
  name: string
  variants: string[]
  selection_mode: string | null
}

interface FormState {
  id: string
  name: string
  variants: string[]
  selection_mode: string
}

const SELECTION_MODES = ["Scored", "RoundRobin", "Random", "WeightedRandom"]
const emptyForm: FormState = { id: "", name: "", variants: [], selection_mode: "Scored" }

export default function Pools() {
  const [pools, setPools] = useState<Pool[]>([])
  const [availableVariants, setAvailableVariants] = useState<string[]>([])
  const [loading, setLoading] = useState(true)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)

  const load = () => {
    setLoading(true)
    Promise.all([
      fetch("/api/pools").then(r => r.json()),
      fetch("/api/variants").then(r => r.json()),
    ]).then(([pd, vd]) => {
      setPools(pd.pools || [])
      setAvailableVariants((vd.variants || []).map((v: { id: string }) => v.id))
      setLoading(false)
    }).catch(() => setLoading(false))
  }
  useEffect(load, [])

  const handleNew = () => { setForm(emptyForm); setEditing(null); setDialogOpen(true) }

  const handleEdit = (p: Pool) => {
    setForm({
      id: p.id,
      name: p.name,
      variants: p.variants || [],
      selection_mode: p.selection_mode || "Scored",
    })
    setEditing(p.id)
    setDialogOpen(true)
  }

  const handleDelete = async (id: string) => {
    if (!confirm(`Delete pool "${id}"?`)) return
    await fetch(`/api/pools/${id}`, { method: "DELETE" })
    load()
  }

  const handleSave = async () => {
    setSaving(true)
    const body = {
      id: editing || form.id,
      name: form.name,
      variants: form.variants,
      selection_mode: form.selection_mode,
    }
    const url = editing ? `/api/pools/${editing}` : "/api/pools"
    const method = editing ? "PUT" : "POST"
    await fetch(url, { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    setSaving(false)
    setDialogOpen(false)
    load()
  }

  const toggleVariant = (v: string) => {
    setForm(f => ({
      ...f,
      variants: f.variants.includes(v) ? f.variants.filter(x => x !== v) : [...f.variants, v],
    }))
  }

  if (loading) return <div className="text-[hsl(var(--muted-foreground))]">Loading...</div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <h1 className="text-xl sm:text-2xl font-bold">Pools</h1>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> New Pool</Button>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
        {pools.map(p => (
          <Card key={p.id}>
            <CardContent className="p-4">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <div className="font-medium">{p.name}</div>
                  <div className="text-sm text-[hsl(var(--muted-foreground))]">{p.id}</div>
                </div>
                {p.selection_mode && <Badge variant="secondary">{p.selection_mode}</Badge>}
              </div>
              <div className="mt-2 flex flex-wrap gap-1">
                {p.variants.map(v => <Badge key={v} variant="outline" className="text-[10px]">{v}</Badge>)}
              </div>
              <div className="text-xs text-[hsl(var(--muted-foreground))] mt-1">{p.variants.length} variants</div>
              <div className="flex gap-1 mt-3 justify-end">
                <Button variant="ghost" size="sm" onClick={() => handleEdit(p)}><Pencil className="h-3 w-3 mr-1" /> Edit</Button>
                <Button variant="ghost" size="sm" onClick={() => handleDelete(p.id)}><Trash2 className="h-3 w-3 mr-1 text-red-400" /> Delete</Button>
              </div>
            </CardContent>
          </Card>
        ))}
        {pools.length === 0 && (
          <div className="col-span-full text-center py-12 text-[hsl(var(--muted-foreground))]">No pools configured</div>
        )}
      </div>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{editing ? `Edit Pool: ${editing}` : "New Pool"}</DialogTitle>
          </DialogHeader>
          <div className="space-y-4 max-h-[60vh] overflow-y-auto pr-2">
            {!editing && (
              <div>
                <Label>ID</Label>
                <Input value={form.id} onChange={e => setForm(f => ({ ...f, id: e.target.value }))} placeholder="default" />
              </div>
            )}
            <div>
              <Label>Name</Label>
              <Input value={form.name} onChange={e => setForm(f => ({ ...f, name: e.target.value }))} placeholder="Default Pool" />
            </div>
            <div>
              <Label>Selection Mode</Label>
              <Select value={form.selection_mode} onValueChange={v => setForm(f => ({ ...f, selection_mode: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {SELECTION_MODES.map(m => <SelectItem key={m} value={m}>{m}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div>
              <Label className="mb-2 block">Variants</Label>
              <div className="flex flex-wrap gap-2">
                {availableVariants.map(v => (
                  <button
                    key={v}
                    type="button"
                    onClick={() => toggleVariant(v)}
                    className={`text-xs px-2 py-1 rounded cursor-pointer transition-colors ${
                      form.variants.includes(v)
                        ? "bg-blue-600 text-white"
                        : "bg-[hsl(var(--secondary))] text-[hsl(var(--secondary-foreground))] hover:bg-[hsl(var(--accent))]"
                    }`}
                  >
                    {v}
                  </button>
                ))}
                {availableVariants.length === 0 && (
                  <span className="text-xs text-[hsl(var(--muted-foreground))]">No variants available. Create variants first.</span>
                )}
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
