import { useEffect, useState, useCallback } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, DialogDescription } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from "@/components/ui/select"
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
import { Plus, Trash2, Pencil, ArrowLeft, Loader2 } from "lucide-react"
import { ConfirmDialog } from "@/components/ui/confirm-dialog"

interface Pool { id: string; name: string; variants: string[]; selection_mode: string | null }
interface VariantFull { id: string; model_id: string; provider_id: string; account_id?: string | null; weight?: number | null }
interface ModelInfo { id: string; display_name: string }

const SELECTION_MODES = ["Scored", "RoundRobin", "Random", "WeightedRandom"]
interface FormState { id: string; name: string; variants: string[]; selection_mode: string }
const emptyForm: FormState = { id: "", name: "", variants: [], selection_mode: "Scored" }

// ═══ Pool Detail ═══
function PoolDetail({ poolId, onBack, onUpdated }: { poolId: string; onBack: () => void; onUpdated: () => void }) {
  const [pool, setPool] = useState<Pool | null>(null)
  const [allVariants, setAllVariants] = useState<VariantFull[]>([])
  const [models, setModels] = useState<ModelInfo[]>([])
  const [loading, setLoading] = useState(true)

  const [editDialogOpen, setEditDialogOpen] = useState(false)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)

  const loadDetail = useCallback(async () => {
    setLoading(true)
    try {
      const [pRes, vRes, mRes] = await Promise.all([
        fetch("/api/pools").then(r => r.json()),
        fetch("/api/variants").then(r => r.json()),
        fetch("/api/models").then(r => r.json()),
      ])
      const found = (pRes.pools || []).find((p: Pool) => p.id === poolId)
      setPool(found || null)
      setAllVariants(vRes.variants || [])
      setModels(mRes.models || [])
    } catch { /* */ }
    setLoading(false)
  }, [poolId])

  useEffect(() => { loadDetail() }, [loadDetail])

  const modelName = (id: string) => models.find(m => m.id === id)?.display_name || id
  const poolVariants = pool ? allVariants.filter(v => pool.variants.includes(v.id)) : []

  // Group pool variants by model
  const byModel = new Map<string, VariantFull[]>()
  for (const v of poolVariants) {
    const list = byModel.get(v.model_id) || []
    list.push(v)
    byModel.set(v.model_id, list)
  }

  const openEdit = () => {
    if (!pool) return
    setForm({ id: pool.id, name: pool.name, variants: [...pool.variants], selection_mode: pool.selection_mode || "Scored" })
    setEditDialogOpen(true)
  }

  const savePool = async () => {
    setSaving(true)
    await fetch(`/api/pools/${poolId}`, {
      method: "PUT", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: poolId, name: form.name, variants: form.variants, selection_mode: form.selection_mode }),
    })
    setSaving(false); setEditDialogOpen(false); loadDetail(); onUpdated()
  }

  const toggleVariant = (vid: string) => setForm(f => ({
    ...f, variants: f.variants.includes(vid) ? f.variants.filter(x => x !== vid) : [...f.variants, vid]
  }))

  // Group all variants by model for the edit dialog
  const allByModel = new Map<string, VariantFull[]>()
  for (const v of allVariants) {
    const list = allByModel.get(v.model_id) || []
    list.push(v)
    allByModel.set(v.model_id, list)
  }

  if (loading) return <div className="flex items-center justify-center py-12"><Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" /></div>
  if (!pool) return <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">Pool not found</div>

  return (
    <div>
      {/* Header */}
      <div className="flex items-center gap-3 mb-6">
        <Button variant="ghost" size="icon" onClick={onBack}><ArrowLeft className="h-4 w-4" /></Button>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <h1 className="text-xl sm:text-2xl font-bold">{pool.name}</h1>
            {pool.selection_mode && <Badge variant="secondary">{pool.selection_mode}</Badge>}
          </div>
          <p className="text-sm text-[hsl(var(--muted-foreground))]">{pool.id}</p>
        </div>
        <Button variant="outline" size="sm" onClick={openEdit}><Pencil className="h-3.5 w-3.5 mr-1.5" /> Edit</Button>
      </div>

      {/* Overview cards */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-6">
        <Card>
          <CardHeader className="pb-2"><CardTitle className="text-sm">Selection Mode</CardTitle></CardHeader>
          <CardContent><span className="text-lg font-medium">{pool.selection_mode || "Scored"}</span></CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2"><CardTitle className="text-sm">Variants</CardTitle></CardHeader>
          <CardContent><span className="text-lg font-medium">{pool.variants.length}</span></CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2"><CardTitle className="text-sm">Models Covered</CardTitle></CardHeader>
          <CardContent><span className="text-lg font-medium">{byModel.size}</span></CardContent>
        </Card>
      </div>

      {/* Variants grouped by model */}
      <div className="flex items-center justify-between mb-4">
        <h2 className="text-lg font-semibold">Variants by Model</h2>
      </div>

      {poolVariants.length === 0 ? (
        <Card><CardContent className="py-12 text-center text-[hsl(var(--muted-foreground))]">No variants in this pool. Click Edit to add variants.</CardContent></Card>
      ) : (
        <div className="space-y-3">
          {Array.from(byModel.entries()).map(([mid, variants]) => (
            <Card key={mid}>
              <CardHeader className="pb-2">
                <div className="flex items-center gap-2">
                  <CardTitle className="text-sm">{modelName(mid)}</CardTitle>
                  <span className="text-xs text-[hsl(var(--muted-foreground))]">({mid})</span>
                  <Badge variant="outline" className="text-[10px] ml-auto">{variants.length} variant{variants.length !== 1 ? "s" : ""}</Badge>
                </div>
              </CardHeader>
              <CardContent className="p-0">
                <div className="overflow-x-auto">
                  <table className="w-full text-sm">
                    <thead><tr className="border-b border-t border-[hsl(var(--border))]">
                      <th className="text-left px-4 py-2 font-medium text-[hsl(var(--muted-foreground))]">Variant ID</th>
                      <th className="text-left px-4 py-2 font-medium text-[hsl(var(--muted-foreground))]">Provider</th>
                      <th className="text-left px-4 py-2 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Account</th>
                      <th className="text-left px-4 py-2 font-medium text-[hsl(var(--muted-foreground))]">Weight</th>
                    </tr></thead>
                    <tbody>
                      {variants.map(v => (
                        <tr key={v.id} className="border-b border-[hsl(var(--border))] last:border-0 hover:bg-[hsl(var(--accent))]/50">
                          <td className="px-4 py-2 font-mono text-xs">{v.id}</td>
                          <td className="px-4 py-2 text-xs">{v.provider_id}</td>
                          <td className="px-4 py-2 text-xs hidden sm:table-cell">{v.account_id || "default"}</td>
                          <td className="px-4 py-2 font-mono text-xs">{v.weight ?? "—"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      )}

      {/* Edit Dialog */}
      <Dialog open={editDialogOpen} onOpenChange={setEditDialogOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader><DialogTitle>Edit Pool: {poolId}</DialogTitle><DialogDescription>Update pool configuration and variants</DialogDescription></DialogHeader>
          <div className="space-y-4">
            <div><Label>Name</Label><Input value={form.name} onChange={e => setForm(f => ({ ...f, name: e.target.value }))} placeholder="Pool name" /></div>
            <div>
              <Label>Selection Mode</Label>
              <Select value={form.selection_mode} onValueChange={v => setForm(f => ({ ...f, selection_mode: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>{SELECTION_MODES.map(m => <SelectItem key={m} value={m}>{m}</SelectItem>)}</SelectContent>
              </Select>
            </div>
            <Separator />
            <div>
              <div className="flex items-center justify-between mb-2">
                <Label>Variants ({form.variants.length} selected)</Label>
                <div className="flex gap-2">
                  <Button variant="ghost" size="sm" onClick={() => setForm(f => ({ ...f, variants: allVariants.map(v => v.id) }))}>All</Button>
                  <Button variant="ghost" size="sm" onClick={() => setForm(f => ({ ...f, variants: [] }))}>None</Button>
                </div>
              </div>
              {allByModel.size === 0 ? (
                <p className="text-xs text-[hsl(var(--muted-foreground))]">No variants available.</p>
              ) : (
                <div className="space-y-3 max-h-[40vh] overflow-y-auto pr-1">
                  {Array.from(allByModel.entries()).map(([mid, variants]) => {
                    const selectedCount = variants.filter(v => form.variants.includes(v.id)).length
                    const allSelected = selectedCount === variants.length
                    return (
                      <div key={mid} className="border border-[hsl(var(--border))] rounded-md p-3">
                        <div className="flex items-center justify-between mb-2">
                          <div className="flex items-center gap-2">
                            <span className="text-sm font-medium">{modelName(mid)}</span>
                            <Badge variant="outline" className="text-[10px]">{selectedCount}/{variants.length}</Badge>
                          </div>
                          <Button variant="ghost" size="sm" className="h-6 text-xs" onClick={() => {
                            const vids = variants.map(v => v.id)
                            setForm(f => ({
                              ...f,
                              variants: allSelected
                                ? f.variants.filter(v => !vids.includes(v))
                                : [...new Set([...f.variants, ...vids])]
                            }))
                          }}>{allSelected ? "Deselect all" : "Select all"}</Button>
                        </div>
                        <div className="flex flex-wrap gap-1.5">
                          {variants.map(v => (
                            <button key={v.id} type="button" onClick={() => toggleVariant(v.id)}
                              className={`text-xs px-2 py-1 rounded cursor-pointer transition-colors ${
                                form.variants.includes(v.id) ? "bg-blue-600 text-white" : "bg-[hsl(var(--secondary))] text-[hsl(var(--secondary-foreground))] hover:bg-[hsl(var(--accent))]"
                              }`}>
                              {v.id} <span className="opacity-60">({v.provider_id})</span>
                            </button>
                          ))}
                        </div>
                      </div>
                    )
                  })}
                </div>
              )}
            </div>
          </div>
          <DialogFooter><Button variant="outline" onClick={() => setEditDialogOpen(false)}>Cancel</Button><Button onClick={savePool} disabled={saving}>{saving ? "Saving..." : "Save"}</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}

// ═══ Main Pools Page ═══
export default function Pools() {
  const [pools, setPools] = useState<Pool[]>([])
  const [allVariants, setAllVariants] = useState<VariantFull[]>([])
  const [models, setModels] = useState<ModelInfo[]>([])
  const [loading, setLoading] = useState(true)
  const [search, setSearch] = useState("")
  const [selectedPool, setSelectedPool] = useState<string | null>(null)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [pRes, vRes, mRes] = await Promise.all([
        fetch("/api/pools").then(r => r.json()),
        fetch("/api/variants").then(r => r.json()),
        fetch("/api/models").then(r => r.json()),
      ])
      setPools(pRes.pools || [])
      setAllVariants(vRes.variants || [])
      setModels(mRes.models || [])
    } catch { /* */ }
    setLoading(false)
  }, [])

  useEffect(() => { load() }, [load])

  const modelName = (id: string) => models.find(m => m.id === id)?.display_name || id

  // For each pool, get the set of model names covered
  const getPoolModels = (p: Pool) => {
    const modelIds = new Set<string>()
    for (const vid of p.variants) {
      const v = allVariants.find(x => x.id === vid)
      if (v) modelIds.add(v.model_id)
    }
    return Array.from(modelIds)
  }

  const handleNew = () => { setForm(emptyForm); setDialogOpen(true) }

  // Group all variants by model for the new pool dialog
  const allByModel = new Map<string, VariantFull[]>()
  for (const v of allVariants) {
    const list = allByModel.get(v.model_id) || []
    list.push(v)
    allByModel.set(v.model_id, list)
  }

  const toggleVariant = (vid: string) => setForm(f => ({
    ...f, variants: f.variants.includes(vid) ? f.variants.filter(x => x !== vid) : [...f.variants, vid]
  }))

  const handleSave = async () => {
    setSaving(true)
    await fetch("/api/pools", {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: form.id, name: form.name, variants: form.variants, selection_mode: form.selection_mode }),
    })
    setSaving(false); setDialogOpen(false); setForm(emptyForm); load()
  }

  const handleDelete = async (id: string) => { await fetch(`/api/pools/${id}`, { method: "DELETE" }); load() }

  if (selectedPool) return <PoolDetail poolId={selectedPool} onBack={() => { setSelectedPool(null); load() }} onUpdated={load} />

  if (loading) return <div className="flex items-center justify-center py-12"><Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" /></div>

  const filtered = search ? pools.filter(p => p.name.toLowerCase().includes(search.toLowerCase()) || p.id.toLowerCase().includes(search.toLowerCase())) : pools

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <div><h1 className="text-xl sm:text-2xl font-bold">Pools</h1><p className="text-sm text-[hsl(var(--muted-foreground))]">Variant pools with selection strategies</p></div>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> New Pool</Button>
      </div>
      <div className="mb-4"><Input placeholder="Search pools..." value={search} onChange={e => setSearch(e.target.value)} className="max-w-sm" /></div>
      <Card>
        <CardContent className="p-0">
          {filtered.length === 0 ? (
            <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">{search ? "No pools match" : "No pools configured"}</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead><tr className="border-b border-[hsl(var(--border))]">
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Pool</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Mode</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Models</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Variants</th>
                  <th className="text-right px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Actions</th>
                </tr></thead>
                <tbody>
                  {filtered.map(p => {
                    const poolModels = getPoolModels(p)
                    return (
                      <tr key={p.id} className="border-b border-[hsl(var(--border))] last:border-0 hover:bg-[hsl(var(--accent))]/50 cursor-pointer transition-colors"
                        onClick={() => setSelectedPool(p.id)}>
                        <td className="px-4 py-3"><div className="font-medium">{p.name}</div><div className="text-xs text-[hsl(var(--muted-foreground))]">{p.id}</div></td>
                        <td className="px-4 py-3">{p.selection_mode ? <Badge variant="secondary">{p.selection_mode}</Badge> : <span className="text-xs text-[hsl(var(--muted-foreground))]">Scored</span>}</td>
                        <td className="px-4 py-3"><div className="flex flex-wrap gap-1">
                          {poolModels.slice(0, 3).map(mid => <Badge key={mid} variant="outline" className="text-[10px]">{modelName(mid)}</Badge>)}
                          {poolModels.length > 3 && <span className="text-[10px] text-[hsl(var(--muted-foreground))]">+{poolModels.length - 3}</span>}
                          {poolModels.length === 0 && <span className="text-xs text-[hsl(var(--muted-foreground))] italic">none</span>}
                        </div></td>
                        <td className="px-4 py-3"><span className="font-mono text-xs">{p.variants.length}</span></td>
                        <td className="px-4 py-3" onClick={e => e.stopPropagation()}><div className="flex justify-end gap-1">
                          <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => setSelectedPool(p.id)}><Pencil className="h-3.5 w-3.5" /></Button>
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

      {/* New Pool Dialog */}
      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader><DialogTitle>New Pool</DialogTitle><DialogDescription>Create a variant pool</DialogDescription></DialogHeader>
          <div className="space-y-4">
            <div><Label>ID</Label><Input value={form.id} onChange={e => setForm(f => ({ ...f, id: e.target.value }))} placeholder="default" /><p className="text-xs text-[hsl(var(--muted-foreground))] mt-1">Cannot be changed later</p></div>
            <div><Label>Name</Label><Input value={form.name} onChange={e => setForm(f => ({ ...f, name: e.target.value }))} placeholder="Default Pool" /></div>
            <div>
              <Label>Selection Mode</Label>
              <Select value={form.selection_mode} onValueChange={v => setForm(f => ({ ...f, selection_mode: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>{SELECTION_MODES.map(m => <SelectItem key={m} value={m}>{m}</SelectItem>)}</SelectContent>
              </Select>
            </div>
            <Separator />
            <div>
              <div className="flex items-center justify-between mb-2">
                <Label>Variants ({form.variants.length} selected)</Label>
                <div className="flex gap-2">
                  <Button variant="ghost" size="sm" onClick={() => setForm(f => ({ ...f, variants: allVariants.map(v => v.id) }))}>All</Button>
                  <Button variant="ghost" size="sm" onClick={() => setForm(f => ({ ...f, variants: [] }))}>None</Button>
                </div>
              </div>
              {allByModel.size === 0 ? (
                <p className="text-xs text-[hsl(var(--muted-foreground))]">No variants available.</p>
              ) : (
                <div className="space-y-3 max-h-[40vh] overflow-y-auto pr-1">
                  {Array.from(allByModel.entries()).map(([mid, variants]) => {
                    const selectedCount = variants.filter(v => form.variants.includes(v.id)).length
                    const allSelected = selectedCount === variants.length
                    return (
                      <div key={mid} className="border border-[hsl(var(--border))] rounded-md p-3">
                        <div className="flex items-center justify-between mb-2">
                          <div className="flex items-center gap-2">
                            <span className="text-sm font-medium">{modelName(mid)}</span>
                            <Badge variant="outline" className="text-[10px]">{selectedCount}/{variants.length}</Badge>
                          </div>
                          <Button variant="ghost" size="sm" className="h-6 text-xs" onClick={() => {
                            const vids = variants.map(v => v.id)
                            setForm(f => ({
                              ...f,
                              variants: allSelected ? f.variants.filter(v => !vids.includes(v)) : [...new Set([...f.variants, ...vids])]
                            }))
                          }}>{allSelected ? "Deselect all" : "Select all"}</Button>
                        </div>
                        <div className="flex flex-wrap gap-1.5">
                          {variants.map(v => (
                            <button key={v.id} type="button" onClick={() => toggleVariant(v.id)}
                              className={`text-xs px-2 py-1 rounded cursor-pointer transition-colors ${
                                form.variants.includes(v.id) ? "bg-blue-600 text-white" : "bg-[hsl(var(--secondary))] text-[hsl(var(--secondary-foreground))] hover:bg-[hsl(var(--accent))]"
                              }`}>
                              {v.id} <span className="opacity-60">({v.provider_id})</span>
                            </button>
                          ))}
                        </div>
                      </div>
                    )
                  })}
                </div>
              )}
            </div>
          </div>
          <DialogFooter><Button variant="outline" onClick={() => setDialogOpen(false)}>Cancel</Button><Button onClick={handleSave} disabled={saving || !form.id}>{saving ? "Creating..." : "Create"}</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog open={!!deleteTarget} onOpenChange={open => { if (!open) setDeleteTarget(null) }}
        title="Delete Pool" description={`Delete pool "${deleteTarget}"? This cannot be undone.`}
        onConfirm={() => { if (deleteTarget) handleDelete(deleteTarget) }} />
    </div>
  )
}
