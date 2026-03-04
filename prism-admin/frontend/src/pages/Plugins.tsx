import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Textarea } from "@/components/ui/textarea"
import { Badge } from "@/components/ui/badge"
import { Plus, Trash2, Pencil } from "lucide-react"

interface Plugin {
  name: string
  source: { type: string; path?: string; name?: string; url?: string; sha256?: string; oci?: string; digest?: string }
  phases: string[]
  config: unknown
  enabled: boolean
  priority: number
}

interface FormState {
  name: string
  source_type: string
  wasm_path: string
  builtin_name: string
  url_value: string
  url_sha256: string
  oci_value: string
  oci_digest: string
  config_json: string
  enabled: boolean
  priority: string
}

const emptyForm: FormState = {
  name: "", source_type: "wasm", wasm_path: "", builtin_name: "",
  url_value: "", url_sha256: "", oci_value: "", oci_digest: "",
  config_json: "{}", enabled: true, priority: "100",
}

const phaseColors: Record<string, string> = {
  config: "bg-amber-600/20 text-amber-400 border-amber-600/30",
  discovery: "bg-cyan-600/20 text-cyan-400 border-cyan-600/30",
  proxy: "bg-blue-600/20 text-blue-400 border-blue-600/30",
  route: "bg-purple-600/20 text-purple-400 border-purple-600/30",
  analyze: "bg-green-600/20 text-green-400 border-green-600/30",
}

export default function Plugins() {
  const [plugins, setPlugins] = useState<Plugin[]>([])
  const [loading, setLoading] = useState(true)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)

  const load = () => {
    setLoading(true)
    fetch("/api/plugins").then(r => r.json()).then(d => { setPlugins(d.plugins || []); setLoading(false) }).catch(() => setLoading(false))
  }
  useEffect(load, [])

  const handleNew = () => { setForm(emptyForm); setEditing(null); setDialogOpen(true) }

  const handleEdit = (p: Plugin) => {
    setForm({
      name: p.name,
      source_type: p.source.type || "wasm",
      wasm_path: p.source.path || "",
      builtin_name: p.source.name || "",
      url_value: p.source.url || "",
      url_sha256: p.source.sha256 || "",
      oci_value: p.source.oci || "",
      oci_digest: p.source.digest || "",
      config_json: JSON.stringify(p.config || {}, null, 2),
      enabled: p.enabled,
      priority: p.priority.toString(),
    })
    setEditing(p.name)
    setDialogOpen(true)
  }

  const handleDelete = async (name: string) => {
    if (!confirm(`Delete plugin "${name}"?`)) return
    await fetch(`/api/plugins/${name}`, { method: "DELETE" })
    load()
  }

  const handleSave = async () => {
    setSaving(true)
    let source: Record<string, unknown> = {}
    switch (form.source_type) {
      case "wasm": source = { wasm: form.wasm_path }; break
      case "builtin": source = { builtin: form.builtin_name }; break
      case "url": source = { url: form.url_value, ...(form.url_sha256 ? { sha256: form.url_sha256 } : {}) }; break
      case "oci": source = { oci: form.oci_value, ...(form.oci_digest ? { digest: form.oci_digest } : {}) }; break
    }
    let configParsed = {}
    try { configParsed = JSON.parse(form.config_json) } catch { /* keep empty */ }
    const body = {
      name: editing || form.name,
      source,
      phases: [],  // phases are declared by the plugin itself, not user-configurable
      config: configParsed,
      enabled: form.enabled,
      priority: parseInt(form.priority) || 100,
    }
    const url = editing ? `/api/plugins/${editing}` : "/api/plugins"
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
        <h1 className="text-xl sm:text-2xl font-bold">Plugins</h1>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> New Plugin</Button>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
        {plugins.map(p => (
          <Card key={p.name}>
            <CardContent className="p-4">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <div className="font-medium">{p.name}</div>
                  <div className="text-xs text-[hsl(var(--muted-foreground))] mt-0.5 truncate">
                    {p.source.type === "wasm" && p.source.path}
                    {p.source.type === "builtin" && `builtin: ${p.source.name}`}
                    {p.source.type === "url" && p.source.url}
                    {p.source.type === "oci" && p.source.oci}
                  </div>
                  <div className="flex flex-wrap gap-1 mt-2">
                    {p.phases.length > 0 ? (
                      p.phases.map(ph => (
                        <span key={ph} className={`text-[10px] px-1.5 py-0.5 rounded border ${phaseColors[ph] || "bg-[hsl(var(--secondary))] text-[hsl(var(--secondary-foreground))]"}`}>
                          {ph}
                        </span>
                      ))
                    ) : (
                      <span className="text-[10px] text-[hsl(var(--muted-foreground))] italic">phases detected on load</span>
                    )}
                  </div>
                </div>
                <div className="flex flex-col items-end gap-1 shrink-0">
                  <Badge variant={p.enabled ? "secondary" : "destructive"} className={p.enabled ? "bg-green-900/50 text-green-400" : ""}>
                    {p.enabled ? "on" : "off"}
                  </Badge>
                  <span className="text-xs text-[hsl(var(--muted-foreground))]">pri: {p.priority}</span>
                </div>
              </div>
              <div className="flex gap-1 mt-3 justify-end">
                <Button variant="ghost" size="sm" onClick={() => handleEdit(p)}><Pencil className="h-3 w-3 mr-1" /> Edit</Button>
                <Button variant="ghost" size="sm" onClick={() => handleDelete(p.name)}><Trash2 className="h-3 w-3 mr-1 text-red-400" /> Delete</Button>
              </div>
            </CardContent>
          </Card>
        ))}
        {plugins.length === 0 && (
          <div className="col-span-full text-center py-12 text-[hsl(var(--muted-foreground))]">No plugins configured</div>
        )}
      </div>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{editing ? `Edit Plugin: ${editing}` : "New Plugin"}</DialogTitle>
          </DialogHeader>
          <div className="space-y-4 max-h-[60vh] overflow-y-auto pr-2">
            {!editing && (
              <div>
                <Label>Name</Label>
                <Input value={form.name} onChange={e => setForm(f => ({ ...f, name: e.target.value }))} placeholder="openai-compat" />
              </div>
            )}
            <div>
              <Label>Source Type</Label>
              <Select value={form.source_type} onValueChange={v => setForm(f => ({ ...f, source_type: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="wasm">WASM</SelectItem>
                  <SelectItem value="builtin">Builtin</SelectItem>
                  <SelectItem value="url">URL</SelectItem>
                  <SelectItem value="oci">OCI</SelectItem>
                </SelectContent>
              </Select>
            </div>
            {form.source_type === "wasm" && (
              <div>
                <Label>WASM Path</Label>
                <Input value={form.wasm_path} onChange={e => setForm(f => ({ ...f, wasm_path: e.target.value }))} placeholder="./plugins/plugin.wasm" />
              </div>
            )}
            {form.source_type === "builtin" && (
              <div>
                <Label>Builtin Name</Label>
                <Input value={form.builtin_name} onChange={e => setForm(f => ({ ...f, builtin_name: e.target.value }))} placeholder="my-plugin" />
              </div>
            )}
            {form.source_type === "url" && (
              <>
                <div>
                  <Label>URL</Label>
                  <Input value={form.url_value} onChange={e => setForm(f => ({ ...f, url_value: e.target.value }))} placeholder="https://cdn.example.com/plugin.wasm" />
                </div>
                <div>
                  <Label>SHA256 (optional)</Label>
                  <Input value={form.url_sha256} onChange={e => setForm(f => ({ ...f, url_sha256: e.target.value }))} placeholder="abc123..." />
                </div>
              </>
            )}
            {form.source_type === "oci" && (
              <>
                <div>
                  <Label>OCI Image</Label>
                  <Input value={form.oci_value} onChange={e => setForm(f => ({ ...f, oci_value: e.target.value }))} placeholder="ghcr.io/prism-plugins/name:v1" />
                </div>
                <div>
                  <Label>Digest (optional)</Label>
                  <Input value={form.oci_digest} onChange={e => setForm(f => ({ ...f, oci_digest: e.target.value }))} placeholder="sha256:..." />
                </div>
              </>
            )}
            <div>
              <Label>Config (JSON)</Label>
              <Textarea
                value={form.config_json}
                onChange={e => setForm(f => ({ ...f, config_json: e.target.value }))}
                className="font-mono text-xs min-h-[100px]"
                placeholder='{"key": "value"}'
              />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="flex items-center gap-2">
                <Switch checked={form.enabled} onCheckedChange={v => setForm(f => ({ ...f, enabled: v }))} />
                <Label>Enabled</Label>
              </div>
              <div>
                <Label>Priority</Label>
                <Input type="number" value={form.priority} onChange={e => setForm(f => ({ ...f, priority: e.target.value }))} />
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
