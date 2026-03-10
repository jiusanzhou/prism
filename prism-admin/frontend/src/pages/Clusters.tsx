import { useEffect, useState } from "react"
import { api } from "@/lib/api"
import { usePageTitle } from "@/hooks/usePageTitle"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import { Trash2, Plus, RefreshCw, ChevronDown, ChevronRight, Loader2 } from "lucide-react"
import { ConfirmDialog } from "@/components/ui/confirm-dialog"

interface Proxy {
  instance_id: string
  cluster_id: string
  address: string
  version: string
  status: string
  last_heartbeat: string
  registered_at: string
  config_version: number
  metadata: Record<string, string>
}

interface ClusterItem {
  id: string
  name: string
  description: string
  tags: Record<string, string>
  created_at: string
  proxy_count: number
  online_count: number
}

interface ClusterDetail extends ClusterItem {
  proxies: Proxy[]
}

interface ClusterForm {
  id: string
  name: string
  description: string
  tags: string
}

const emptyForm: ClusterForm = { id: "", name: "", description: "", tags: "" }

function timeAgo(dateStr: string) {
  const diff = Date.now() - new Date(dateStr).getTime()
  const secs = Math.floor(diff / 1000)
  if (secs < 60) return `${secs}s ago`
  const mins = Math.floor(secs / 60)
  if (mins < 60) return `${mins}m ago`
  const hrs = Math.floor(mins / 60)
  if (hrs < 24) return `${hrs}h ago`
  return `${Math.floor(hrs / 24)}d ago`
}

function statusColor(status: string) {
  switch (status) {
    case "online": return "bg-green-500/20 text-green-400 border-green-500/30"
    case "offline": return "bg-red-500/20 text-red-400 border-red-500/30"
    case "degraded": return "bg-yellow-500/20 text-yellow-400 border-yellow-500/30"
    default: return "bg-gray-500/20 text-gray-400 border-gray-500/30"
  }
}

export default function Clusters() {
  usePageTitle("Clusters")
  const [clusters, setClusters] = useState<ClusterItem[]>([])
  const [loading, setLoading] = useState(true)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [form, setForm] = useState<ClusterForm>(emptyForm)
  const [saving, setSaving] = useState(false)
  const [expanded, setExpanded] = useState<string | null>(null)
  const [detail, setDetail] = useState<ClusterDetail | null>(null)
  const [syncing, setSyncing] = useState<string | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)

  const load = () => {
    setLoading(true)
    api("/api/clusters")
      .then(r => r.json())
      .then(d => { setClusters(d || []); setLoading(false) })
      .catch(() => setLoading(false))
  }

  useEffect(load, [])

  const toggle = (id: string) => {
    if (expanded === id) {
      setExpanded(null)
      setDetail(null)
      return
    }
    setExpanded(id)
    api(`/api/clusters/${id}`)
      .then(r => r.json())
      .then(setDetail)
      .catch(() => setDetail(null))
  }

  const save = async () => {
    setSaving(true)
    try {
      let tags: Record<string, string> = {}
      if (form.tags.trim()) {
        try { tags = JSON.parse(form.tags) } catch { /* ignore */ }
      }
      await api("/api/clusters", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ id: form.id, name: form.name, description: form.description, tags }),
      })
      setDialogOpen(false)
      setForm(emptyForm)
      load()
    } finally {
      setSaving(false)
    }
  }

  const remove = async (id: string) => {
    await api(`/api/clusters/${id}`, { method: "DELETE" })
    load()
  }

  const sync = async (id: string) => {
    setSyncing(id)
    try {
      await api(`/api/clusters/${id}/sync`, { method: "POST" })
    } finally {
      setSyncing(null)
    }
  }

  if (loading) return <div className="flex items-center justify-center py-12"><Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" /></div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <div><h1 className="text-xl sm:text-2xl font-bold">Clusters</h1><p className="text-sm text-[hsl(var(--muted-foreground))]">Proxy cluster management</p></div>
        <Button size="sm" onClick={() => { setForm(emptyForm); setDialogOpen(true) }}>
          <Plus className="h-4 w-4 mr-1" /> New Cluster
        </Button>
      </div>

      <Card>
        <CardContent className="p-0">
          {clusters.length === 0 ? (
            <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">No clusters configured</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead><tr className="border-b border-[hsl(var(--border))]">
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] w-8"></th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Cluster</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Description</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden md:table-cell">Tags</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Proxies</th>
                  <th className="text-right px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Actions</th>
                </tr></thead>
                <tbody>
                  {clusters.map(c => (<>
                    <tr key={c.id} className="border-b border-[hsl(var(--border))] last:border-0 hover:bg-[hsl(var(--accent))]/50 cursor-pointer transition-colors" onClick={() => toggle(c.id)}>
                      <td className="px-4 py-3">{expanded === c.id ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}</td>
                      <td className="px-4 py-3"><div className="font-medium">{c.name}</div><div className="text-xs text-[hsl(var(--muted-foreground))]">{c.id}</div></td>
                      <td className="px-4 py-3 hidden sm:table-cell"><span className="text-xs text-[hsl(var(--muted-foreground))]">{c.description || "—"}</span></td>
                      <td className="px-4 py-3 hidden md:table-cell"><div className="flex flex-wrap gap-1">{Object.entries(c.tags || {}).map(([k, v]) => <Badge key={k} variant="secondary" className="text-[10px]">{k}: {v}</Badge>)}{Object.keys(c.tags || {}).length === 0 && <span className="text-xs text-[hsl(var(--muted-foreground))]">—</span>}</div></td>
                      <td className="px-4 py-3"><Badge variant="outline" className="text-xs">{c.online_count}/{c.proxy_count} online</Badge></td>
                      <td className="px-4 py-3" onClick={e => e.stopPropagation()}><div className="flex justify-end gap-1">
                        <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => sync(c.id)} disabled={syncing === c.id}><RefreshCw className={`h-3.5 w-3.5 ${syncing === c.id ? "animate-spin" : ""}`} /></Button>
                        <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => setDeleteTarget(c.id)}><Trash2 className="h-3.5 w-3.5 text-red-400" /></Button>
                      </div></td>
                    </tr>
                    {expanded === c.id && detail && detail.id === c.id && (
                      <tr key={`${c.id}-detail`}><td colSpan={6} className="px-4 pb-4 pt-0">
                        {detail.proxies.length === 0 ? (
                          <div className="text-sm text-[hsl(var(--muted-foreground))] py-4 text-center">No proxies registered in this cluster</div>
                        ) : (
                          <div className="border border-[hsl(var(--border))] rounded-md overflow-hidden">
                            <table className="w-full text-sm">
                              <thead>
                                <tr className="border-b border-[hsl(var(--border))] bg-[hsl(var(--muted))]/50">
                                  <th className="text-left px-3 py-2 font-medium">Instance</th>
                                  <th className="text-left px-3 py-2 font-medium">Address</th>
                                  <th className="text-left px-3 py-2 font-medium">Version</th>
                                  <th className="text-left px-3 py-2 font-medium">Status</th>
                                  <th className="text-left px-3 py-2 font-medium">Heartbeat</th>
                                  <th className="text-left px-3 py-2 font-medium">Config</th>
                                </tr>
                              </thead>
                              <tbody>
                                {detail.proxies.map(p => (
                                  <tr key={p.instance_id} className="border-b border-[hsl(var(--border))] last:border-0">
                                    <td className="px-3 py-2 font-mono text-xs">{p.instance_id}</td>
                                    <td className="px-3 py-2 font-mono text-xs">{p.address}</td>
                                    <td className="px-3 py-2 text-xs">{p.version}</td>
                                    <td className="px-3 py-2">
                                      <Badge variant="outline" className={`text-xs ${statusColor(p.status)}`}>
                                        {p.status}
                                      </Badge>
                                    </td>
                                    <td className="px-3 py-2 text-xs text-[hsl(var(--muted-foreground))]">
                                      {timeAgo(p.last_heartbeat)}
                                    </td>
                                    <td className="px-3 py-2 text-xs">v{p.config_version}</td>
                                  </tr>
                                ))}
                              </tbody>
                            </table>
                          </div>
                        )}
                      </td></tr>
                    )}
                  </>))}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New Cluster</DialogTitle>
          </DialogHeader>
          <div className="space-y-3">
            <div>
              <Label>ID</Label>
              <Input value={form.id} onChange={e => setForm({ ...form, id: e.target.value })} placeholder="production-us-east" />
            </div>
            <div>
              <Label>Name</Label>
              <Input value={form.name} onChange={e => setForm({ ...form, name: e.target.value })} placeholder="Production US East" />
            </div>
            <div>
              <Label>Description</Label>
              <Textarea value={form.description} onChange={e => setForm({ ...form, description: e.target.value })} placeholder="Primary production cluster" rows={2} />
            </div>
            <div>
              <Label>Tags (JSON)</Label>
              <Input value={form.tags} onChange={e => setForm({ ...form, tags: e.target.value })} placeholder='{"env": "production", "region": "us-east-1"}' />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDialogOpen(false)}>Cancel</Button>
            <Button onClick={save} disabled={saving || !form.id || !form.name}>
              {saving ? "Creating..." : "Create"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={open => { if (!open) setDeleteTarget(null) }}
        title="Delete Cluster"
        description={`Are you sure you want to delete cluster "${deleteTarget}"? This action cannot be undone.`}
        onConfirm={() => { if (deleteTarget) remove(deleteTarget) }}
      />
    </div>
  )
}
