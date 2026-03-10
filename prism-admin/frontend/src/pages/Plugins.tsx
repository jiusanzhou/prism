import { useEffect, useState } from "react"
import { api } from "@/lib/api"
import { usePageTitle } from "@/hooks/usePageTitle"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Textarea } from "@/components/ui/textarea"
import { Badge } from "@/components/ui/badge"
import { ConfirmDialog } from "@/components/ui/confirm-dialog"
import { Plus, Trash2, Pencil, GripVertical } from "lucide-react"
import {
  DndContext, closestCenter, KeyboardSensor, PointerSensor,
  useSensor, useSensors, type DragEndEvent,
} from "@dnd-kit/core"
import {
  arrayMove, SortableContext, sortableKeyboardCoordinates,
  useSortable, verticalListSortingStrategy,
} from "@dnd-kit/sortable"
import { CSS } from "@dnd-kit/utilities"

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

const PHASES = ["config", "discovery", "proxy", "route", "analyze"] as const
type Phase = typeof PHASES[number]

const phaseStyle: Record<string, { active: string; inactive: string; badge: string }> = {
  config:    { active: "bg-amber-600 text-white",    inactive: "text-amber-400 hover:bg-amber-600/20",  badge: "bg-amber-600/20 text-amber-400 border-amber-600/30" },
  discovery: { active: "bg-cyan-600 text-white",     inactive: "text-cyan-400 hover:bg-cyan-600/20",    badge: "bg-cyan-600/20 text-cyan-400 border-cyan-600/30" },
  proxy:     { active: "bg-blue-600 text-white",     inactive: "text-blue-400 hover:bg-blue-600/20",    badge: "bg-blue-600/20 text-blue-400 border-blue-600/30" },
  route:     { active: "bg-purple-600 text-white",   inactive: "text-purple-400 hover:bg-purple-600/20",badge: "bg-purple-600/20 text-purple-400 border-purple-600/30" },
  analyze:   { active: "bg-green-600 text-white",    inactive: "text-green-400 hover:bg-green-600/20",  badge: "bg-green-600/20 text-green-400 border-green-600/30" },
}

function SortableRow({ plugin, phase, onEdit, onDelete }: {
  plugin: Plugin; phase: string
  onEdit: (p: Plugin) => void; onDelete: (name: string) => void
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: plugin.name })
  const style = { transform: CSS.Transform.toString(transform), transition, opacity: isDragging ? 0.5 : 1 }
  const otherPhases = plugin.phases.filter(p => p !== phase)

  return (
    <div ref={setNodeRef} style={style}
      className="flex items-center gap-2 px-3 py-2.5 border-b border-[hsl(var(--border))] last:border-0 group hover:bg-[hsl(var(--accent))]/50 transition-colors"
    >
      <button {...attributes} {...listeners}
        className="cursor-grab active:cursor-grabbing touch-none text-[hsl(var(--muted-foreground))] hover:text-[hsl(var(--foreground))] shrink-0"
      >
        <GripVertical className="h-4 w-4" />
      </button>
      <button className="font-medium text-sm truncate min-w-0 hover:underline cursor-pointer text-left"
        onClick={() => onEdit(plugin)}>{plugin.name}</button>
      <Badge variant="outline" className="text-[10px] px-1.5 shrink-0">{plugin.source.type}</Badge>
      <Badge variant={plugin.enabled ? "secondary" : "destructive"}
        className={`text-[10px] px-1.5 shrink-0 ${plugin.enabled ? "bg-green-900/50 text-green-400" : ""}`}
      >{plugin.enabled ? "on" : "off"}</Badge>
      <span className="text-xs text-[hsl(var(--muted-foreground))] shrink-0 ml-auto">pri: {plugin.priority}</span>
      {otherPhases.length > 0 && (
        <span className="text-[10px] text-[hsl(var(--muted-foreground))] italic shrink-0 hidden sm:inline">
          +{otherPhases.map(p => <span key={p} className={`ml-0.5 px-1 rounded border ${phaseStyle[p]?.badge || ""}`}>{p}</span>)}
        </span>
      )}
      <div className="flex gap-0.5 shrink-0 opacity-0 group-hover:opacity-100 transition-opacity">
        <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => onEdit(plugin)}><Pencil className="h-3 w-3" /></Button>
        <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => onDelete(plugin.name)}><Trash2 className="h-3 w-3 text-red-400" /></Button>
      </div>
    </div>
  )
}

export default function Plugins() {
  usePageTitle("Plugins")
  const [plugins, setPlugins] = useState<Plugin[]>([])
  const [loading, setLoading] = useState(true)
  const [activeTab, setActiveTab] = useState<Phase | "all">("all")
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [form, setForm] = useState<FormState>(emptyForm)
  const [saving, setSaving] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  )

  const load = () => {
    setLoading(true)
    api("/api/plugins").then(r => r.json()).then(d => { setPlugins(d.plugins || []); setLoading(false) }).catch(() => setLoading(false))
  }
  useEffect(load, [])

  const getPhasePlugins = (phase: Phase) =>
    plugins.filter(p => p.phases.includes(phase)).sort((a, b) => a.priority - b.priority)

  const allSorted = [...plugins].sort((a, b) => a.priority - b.priority)

  const currentPlugins = activeTab === "all" ? allSorted : getPhasePlugins(activeTab)
  const currentIds = currentPlugins.map(p => p.name)

  const handleDragEnd = async (event: DragEndEvent) => {
    const { active, over } = event
    if (!over || active.id === over.id) return
    const oldIndex = currentIds.indexOf(active.id as string)
    const newIndex = currentIds.indexOf(over.id as string)
    if (oldIndex === -1 || newIndex === -1) return

    const reordered = arrayMove(currentPlugins, oldIndex, newIndex)
    const updatedPlugins = plugins.map(p => {
      const idx = reordered.findIndex(rp => rp.name === p.name)
      return idx !== -1 ? { ...p, priority: (idx + 1) * 10 } : p
    })
    setPlugins(updatedPlugins)

    const updates = reordered.map((p, idx) => ({ ...p, newPri: (idx + 1) * 10 })).filter(u => u.newPri !== u.priority)
    await Promise.all(updates.map(u => {
      let source: Record<string, unknown> = {}
      const orig = plugins.find(p => p.name === u.name)!
      switch (orig.source.type) {
        case "wasm": source = { wasm: orig.source.path }; break
        case "builtin": source = { builtin: orig.source.name }; break
        case "url": source = { url: orig.source.url, ...(orig.source.sha256 ? { sha256: orig.source.sha256 } : {}) }; break
        case "oci": source = { oci: orig.source.oci, ...(orig.source.digest ? { digest: orig.source.digest } : {}) }; break
      }
      return api(`/api/plugins/${u.name}`, {
        method: "PUT", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name: u.name, source, phases: [], config: orig.config || {}, enabled: orig.enabled, priority: u.newPri }),
      })
    }))
  }

  const handleNew = () => { setForm(emptyForm); setEditing(null); setDialogOpen(true) }
  const handleEdit = (p: Plugin) => {
    setForm({
      name: p.name, source_type: p.source.type || "wasm",
      wasm_path: p.source.path || "", builtin_name: p.source.name || "",
      url_value: p.source.url || "", url_sha256: p.source.sha256 || "",
      oci_value: p.source.oci || "", oci_digest: p.source.digest || "",
      config_json: JSON.stringify(p.config || {}, null, 2),
      enabled: p.enabled, priority: p.priority.toString(),
    })
    setEditing(p.name); setDialogOpen(true)
  }
  const handleDelete = async (name: string) => { await api(`/api/plugins/${name}`, { method: "DELETE" }); load() }
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
    const body = { name: editing || form.name, source, phases: [], config: configParsed, enabled: form.enabled, priority: parseInt(form.priority) || 100 }
    const url = editing ? `/api/plugins/${editing}` : "/api/plugins"
    await api(url, { method: editing ? "PUT" : "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    setSaving(false); setDialogOpen(false); load()
  }

  if (loading) return <div className="text-[hsl(var(--muted-foreground))]">Loading...</div>

  const tabs: { key: Phase | "all"; label: string }[] = [
    { key: "all", label: "All" },
    ...PHASES.map(p => ({ key: p as Phase, label: p.charAt(0).toUpperCase() + p.slice(1) })),
  ]

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <h1 className="text-xl sm:text-2xl font-bold">Plugins</h1>
        <Button size="sm" onClick={handleNew}><Plus className="h-4 w-4 mr-1" /> New Plugin</Button>
      </div>

      {/* Tabs */}
      <div className="flex gap-1 mb-4 overflow-x-auto pb-1 scrollbar-none">
        {tabs.map(tab => {
          const isActive = activeTab === tab.key
          const count = tab.key === "all" ? plugins.length : getPhasePlugins(tab.key as Phase).length
          const style = tab.key === "all"
            ? { active: "bg-[hsl(var(--foreground))] text-[hsl(var(--background))]", inactive: "text-[hsl(var(--muted-foreground))] hover:bg-[hsl(var(--accent))]" }
            : phaseStyle[tab.key] || { active: "", inactive: "" }
          return (
            <button
              key={tab.key}
              onClick={() => setActiveTab(tab.key as Phase | "all")}
              className={`flex items-center gap-1.5 px-3 py-1.5 rounded-md text-sm font-medium transition-colors whitespace-nowrap ${isActive ? style.active : style.inactive}`}
            >
              {tab.label}
              <span className={`text-[10px] px-1.5 py-0.5 rounded-full ${isActive ? "bg-white/20" : "bg-[hsl(var(--secondary))]"}`}>
                {count}
              </span>
            </button>
          )
        })}
      </div>

      {/* Plugin list */}
      <Card>
        <CardContent className="p-0">
          {currentPlugins.length === 0 ? (
            <div className="px-4 py-12 text-center text-[hsl(var(--muted-foreground))]">
              {activeTab === "all" ? "No plugins configured" : `No plugins in ${activeTab} phase`}
            </div>
          ) : (
            <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
              <SortableContext items={currentIds} strategy={verticalListSortingStrategy}>
                {currentPlugins.map(p => (
                  <SortableRow
                    key={p.name} plugin={p} phase={activeTab === "all" ? "" : activeTab}
                    onEdit={handleEdit} onDelete={name => setDeleteTarget(name)}
                  />
                ))}
              </SortableContext>
            </DndContext>
          )}
        </CardContent>
      </Card>

      {/* Edit/New dialog */}
      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent>
          <DialogHeader><DialogTitle>{editing ? `Edit: ${editing}` : "New Plugin"}</DialogTitle></DialogHeader>
          <div className="space-y-4 max-h-[60vh] overflow-y-auto pr-2">
            {!editing && <div><Label>Name</Label><Input value={form.name} onChange={e => setForm(f => ({ ...f, name: e.target.value }))} placeholder="openai-compat" /></div>}
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
            {form.source_type === "wasm" && <div><Label>WASM Path</Label><Input value={form.wasm_path} onChange={e => setForm(f => ({ ...f, wasm_path: e.target.value }))} placeholder="./plugins/plugin.wasm" /></div>}
            {form.source_type === "builtin" && <div><Label>Builtin Name</Label><Input value={form.builtin_name} onChange={e => setForm(f => ({ ...f, builtin_name: e.target.value }))} placeholder="my-plugin" /></div>}
            {form.source_type === "url" && (<><div><Label>URL</Label><Input value={form.url_value} onChange={e => setForm(f => ({ ...f, url_value: e.target.value }))} placeholder="https://cdn.example.com/plugin.wasm" /></div><div><Label>SHA256</Label><Input value={form.url_sha256} onChange={e => setForm(f => ({ ...f, url_sha256: e.target.value }))} placeholder="optional" /></div></>)}
            {form.source_type === "oci" && (<><div><Label>OCI Image</Label><Input value={form.oci_value} onChange={e => setForm(f => ({ ...f, oci_value: e.target.value }))} placeholder="ghcr.io/prism-plugins/name:v1" /></div><div><Label>Digest</Label><Input value={form.oci_digest} onChange={e => setForm(f => ({ ...f, oci_digest: e.target.value }))} placeholder="optional" /></div></>)}
            <div><Label>Config (JSON)</Label><Textarea value={form.config_json} onChange={e => setForm(f => ({ ...f, config_json: e.target.value }))} className="font-mono text-xs min-h-[100px]" placeholder='{"key": "value"}' /></div>
            <div className="grid grid-cols-2 gap-3">
              <div className="flex items-center gap-2"><Switch checked={form.enabled} onCheckedChange={v => setForm(f => ({ ...f, enabled: v }))} /><Label>Enabled</Label></div>
              <div><Label>Priority</Label><Input type="number" value={form.priority} onChange={e => setForm(f => ({ ...f, priority: e.target.value }))} /></div>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDialogOpen(false)}>Cancel</Button>
            <Button onClick={handleSave} disabled={saving}>{saving ? "Saving..." : "Save"}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={!!deleteTarget} onOpenChange={open => { if (!open) setDeleteTarget(null) }}
        title="Delete Plugin" description={`Delete plugin "${deleteTarget}"? This cannot be undone.`}
        onConfirm={() => { if (deleteTarget) handleDelete(deleteTarget) }}
      />
    </div>
  )
}
