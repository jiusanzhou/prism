import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"
import { Badge } from "@/components/ui/badge"
import { Plus, Trash2, Pencil, X, Loader2 } from "lucide-react"
import { ConfirmDialog } from "@/components/ui/confirm-dialog"

interface RateLimit {
  name: string
  key_headers: string[]
  window_secs: number
  max_requests: number
  max_tokens: number | null
  enabled: boolean
  overrides: number
}

interface OverrideForm {
  match_headers: { key: string; value: string }[]
  max_requests: string
  max_tokens: string
}

interface FormState {
  name: string
  key_headers: string
  window_secs: string
  max_requests: string
  max_tokens: string
  enabled: boolean
  overrides: OverrideForm[]
}

const emptyForm: FormState = {
  name: "", key_headers: "", window_secs: "60", max_requests: "1000",
  max_tokens: "", enabled: true, overrides: [],
}

export default function RateLimits() {
  const [rules, setRules] = useState<RateLimit[]>([])
  const [loading, setLoading] = useState(true)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)
  const [togglingName, setTogglingName] = useState<string | null>(null)

  const load = () => {
    setLoading(true)
    fetch("/api/rate-limits").then(r => r.json()).then(d => { setRules(d.rate_limits || []); setLoading(false) }).catch(() => setLoading(false))
  }
  useEffect(load, [])

  const handleNew = () => { setForm(emptyForm); setEditing(null); setDialogOpen(true) }

  const handleEdit = (r: RateLimit) => {
    setForm({
      name: r.name,
      key_headers: (r.key_headers || []).join(", "),
      window_secs: r.window_secs.toString(),
      max_requests: r.max_requests.toString(),
      max_tokens: r.max_tokens?.toString() || "",
      enabled: r.enabled,
      overrides: [],
    })
    setEditing(r.name)
    setDialogOpen(true)
  }

  const handleDelete = async (name: string) => {
    await fetch(`/api/rate-limits/${name}`, { method: "DELETE" })
    load()
  }

  const toggleEnabled = async (r: RateLimit) => {
    setTogglingName(r.name)
    setRules(prev => prev.map(rule => rule.name === r.name ? { ...rule, enabled: !rule.enabled } : rule))
    await fetch(`/api/rate-limits/${r.name}`, {
      method: "PUT", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        name: r.name, key_headers: r.key_headers, window_secs: r.window_secs,
        max_requests: r.max_requests, max_tokens: r.max_tokens, enabled: !r.enabled, overrides: [],
      }),
    })
    setTogglingName(null)
  }

  const handleSave = async () => {
    setSaving(true)
    const body = {
      name: editing || form.name,
      key_headers: form.key_headers.split(",").map(s => s.trim()).filter(Boolean),
      window_secs: parseInt(form.window_secs) || 60,
      max_requests: parseInt(form.max_requests) || 1000,
      max_tokens: form.max_tokens ? parseInt(form.max_tokens) : null,
      enabled: form.enabled,
      overrides: form.overrides.map(o => ({
        match_headers: Object.fromEntries(o.match_headers.filter(h => h.key).map(h => [h.key, h.value])),
        max_requests: o.max_requests ? parseInt(o.max_requests) : null,
        max_tokens: o.max_tokens ? parseInt(o.max_tokens) : null,
      })),
    }
    const url = editing ? `/api/rate-limits/${editing}` : "/api/rate-limits"
    const method = editing ? "PUT" : "POST"
    await fetch(url, { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    setSaving(false)
    setDialogOpen(false)
    load()
  }

  const addOverride = () => setForm(f => ({ ...f, overrides: [...f.overrides, { match_headers: [{ key: "", value: "" }], max_requests: "", max_tokens: "" }] }))
  const removeOverride = (i: number) => setForm(f => ({ ...f, overrides: f.overrides.filter((_, idx) => idx !== i) }))

  const formatWindow = (secs: number) => {
    if (secs < 60) return `${secs}s`
    if (secs < 3600) return `${Math.floor(secs / 60)}m`
    return `${Math.floor(secs / 3600)}h`
  }

  if (loading) return <div className="flex items-center justify-center py-12"><Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" /></div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <div><h1 className="text-xl sm:text-2xl font-bold">Rate Limits</h1><p className="text-sm text-[hsl(var(--muted-foreground))]">Request throttling rules</p></div>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> New Rule</Button>
      </div>
      <Card>
        <CardContent className="p-0">
          {rules.length === 0 ? (
            <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">No rate limit rules</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead><tr className="border-b border-[hsl(var(--border))]">
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Name</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Key Headers</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Window</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Max Requests</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden md:table-cell">Max Tokens</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Status</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Overrides</th>
                  <th className="text-right px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Actions</th>
                </tr></thead>
                <tbody>
                  {rules.map(r => (
                    <tr key={r.name} className="border-b border-[hsl(var(--border))] last:border-0 hover:bg-[hsl(var(--accent))]/50 transition-colors">
                      <td className="px-4 py-3 font-medium">{r.name}</td>
                      <td className="px-4 py-3 hidden sm:table-cell"><div className="flex flex-wrap gap-1">{r.key_headers && r.key_headers.length > 0 ? r.key_headers.map(h => <Badge key={h} variant="outline" className="text-[10px]">{h}</Badge>) : <span className="text-xs text-[hsl(var(--muted-foreground))] italic">global</span>}</div></td>
                      <td className="px-4 py-3"><span className="font-mono text-xs">{formatWindow(r.window_secs)}</span></td>
                      <td className="px-4 py-3"><span className="font-mono text-xs">{r.max_requests.toLocaleString()}</span></td>
                      <td className="px-4 py-3 hidden md:table-cell"><span className="font-mono text-xs">{r.max_tokens != null ? r.max_tokens.toLocaleString() : "—"}</span></td>
                      <td className="px-4 py-3"><Switch checked={r.enabled} disabled={togglingName === r.name} onCheckedChange={() => toggleEnabled(r)} /></td>
                      <td className="px-4 py-3 hidden sm:table-cell">{r.overrides > 0 ? <Badge variant="secondary" className="text-[10px]">{r.overrides}</Badge> : <span className="text-xs text-[hsl(var(--muted-foreground))]">—</span>}</td>
                      <td className="px-4 py-3"><div className="flex justify-end gap-1">
                        <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => handleEdit(r)}><Pencil className="h-3.5 w-3.5" /></Button>
                        <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => setDeleteTarget(r.name)}><Trash2 className="h-3.5 w-3.5 text-red-400" /></Button>
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
            <DialogTitle>{editing ? `Edit Rule: ${editing}` : "New Rate Limit Rule"}</DialogTitle>
          </DialogHeader>
          <div className="space-y-4 max-h-[60vh] overflow-y-auto pr-2">
            {!editing && (
              <div>
                <Label>Name</Label>
                <Input value={form.name} onChange={e => setForm(f => ({ ...f, name: e.target.value }))} placeholder="global" />
              </div>
            )}
            <div>
              <Label>Key Headers (comma-separated)</Label>
              <Input value={form.key_headers} onChange={e => setForm(f => ({ ...f, key_headers: e.target.value }))} placeholder="x-user, x-api-key (empty = global)" />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <Label>Window (secs)</Label>
                <Input type="number" value={form.window_secs} onChange={e => setForm(f => ({ ...f, window_secs: e.target.value }))} />
              </div>
              <div>
                <Label>Max Requests</Label>
                <Input type="number" value={form.max_requests} onChange={e => setForm(f => ({ ...f, max_requests: e.target.value }))} />
              </div>
            </div>
            <div>
              <Label>Max Tokens (optional)</Label>
              <Input type="number" value={form.max_tokens} onChange={e => setForm(f => ({ ...f, max_tokens: e.target.value }))} placeholder="Leave empty for unlimited" />
            </div>
            <div className="flex items-center gap-2">
              <Switch checked={form.enabled} onCheckedChange={v => setForm(f => ({ ...f, enabled: v }))} />
              <Label>Enabled</Label>
            </div>
            <div>
              <div className="flex items-center justify-between mb-2">
                <Label>Overrides</Label>
                <Button variant="ghost" size="sm" onClick={addOverride}><Plus className="h-3 w-3 mr-1" /> Add Override</Button>
              </div>
              {form.overrides.map((o, i) => (
                <div key={i} className="border border-[hsl(var(--border))] rounded-md p-3 mb-2 space-y-2">
                  <div className="flex items-center justify-between">
                    <span className="text-xs font-medium">Override {i + 1}</span>
                    <Button variant="ghost" size="icon" className="h-6 w-6" onClick={() => removeOverride(i)}><X className="h-3 w-3" /></Button>
                  </div>
                  {o.match_headers.map((h, hi) => (
                    <div key={hi} className="flex gap-2">
                      <Input placeholder="Header" value={h.key} onChange={e => {
                        const newOverrides = [...form.overrides]
                        newOverrides[i].match_headers[hi].key = e.target.value
                        setForm(f => ({ ...f, overrides: newOverrides }))
                      }} className="flex-1" />
                      <Input placeholder="Value" value={h.value} onChange={e => {
                        const newOverrides = [...form.overrides]
                        newOverrides[i].match_headers[hi].value = e.target.value
                        setForm(f => ({ ...f, overrides: newOverrides }))
                      }} className="flex-1" />
                    </div>
                  ))}
                  <div className="grid grid-cols-2 gap-2">
                    <div>
                      <Label className="text-xs">Max Requests</Label>
                      <Input type="number" value={o.max_requests} onChange={e => {
                        const newOverrides = [...form.overrides]
                        newOverrides[i].max_requests = e.target.value
                        setForm(f => ({ ...f, overrides: newOverrides }))
                      }} />
                    </div>
                    <div>
                      <Label className="text-xs">Max Tokens</Label>
                      <Input type="number" value={o.max_tokens} onChange={e => {
                        const newOverrides = [...form.overrides]
                        newOverrides[i].max_tokens = e.target.value
                        setForm(f => ({ ...f, overrides: newOverrides }))
                      }} />
                    </div>
                  </div>
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
        title="Delete Rate Limit"
        description={`Are you sure you want to delete rate limit "${deleteTarget}"? This action cannot be undone.`}
        onConfirm={() => { if (deleteTarget) handleDelete(deleteTarget) }}
      />
    </div>
  )
}
