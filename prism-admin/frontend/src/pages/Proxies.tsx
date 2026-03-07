import { useEffect, useState, useCallback } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Trash2, Loader2 } from "lucide-react"
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

export default function Proxies() {
  const [proxies, setProxies] = useState<Proxy[]>([])
  const [loading, setLoading] = useState(true)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)

  const load = useCallback(() => {
    fetch("/api/proxies")
      .then(r => r.json())
      .then(d => { setProxies(d || []); setLoading(false) })
      .catch(() => setLoading(false))
  }, [])

  useEffect(() => {
    load()
    const interval = setInterval(load, 30000)
    return () => clearInterval(interval)
  }, [load])

  const remove = async (instanceId: string) => {
    await fetch(`/api/proxies/${instanceId}`, { method: "DELETE" })
    load()
  }

  if (loading) return <div className="flex items-center justify-center py-12"><Loader2 className="h-6 w-6 animate-spin text-[hsl(var(--muted-foreground))]" /></div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <div><h1 className="text-xl sm:text-2xl font-bold">Proxies</h1><p className="text-sm text-[hsl(var(--muted-foreground))]">All registered proxy instances</p></div>
        <span className="text-sm text-[hsl(var(--muted-foreground))]">
          {proxies.length} total &middot; auto-refreshes every 30s
        </span>
      </div>

      <Card>
        <CardContent className="p-0">
          {proxies.length === 0 ? (
            <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">No proxy instances registered</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead><tr className="border-b border-[hsl(var(--border))]">
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Instance ID</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Cluster</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Address</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden md:table-cell">Version</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Status</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden sm:table-cell">Heartbeat</th>
                  <th className="text-left px-4 py-3 font-medium text-[hsl(var(--muted-foreground))] hidden md:table-cell">Config Version</th>
                  <th className="text-right px-4 py-3 font-medium text-[hsl(var(--muted-foreground))]">Actions</th>
                </tr></thead>
                <tbody>
                  {proxies.map(p => (
                    <tr key={p.instance_id} className="border-b border-[hsl(var(--border))] last:border-0 hover:bg-[hsl(var(--accent))]/50 transition-colors">
                      <td className="px-4 py-3 font-mono text-xs font-medium">{p.instance_id}</td>
                      <td className="px-4 py-3"><span className="text-xs">{p.cluster_id}</span></td>
                      <td className="px-4 py-3 hidden sm:table-cell"><span className="font-mono text-xs">{p.address}</span></td>
                      <td className="px-4 py-3 hidden md:table-cell"><span className="text-xs">{p.version}</span></td>
                      <td className="px-4 py-3"><Badge variant="outline" className={`text-xs ${statusColor(p.status)}`}>{p.status}</Badge></td>
                      <td className="px-4 py-3 hidden sm:table-cell"><span className="text-xs text-[hsl(var(--muted-foreground))]">{timeAgo(p.last_heartbeat)}</span></td>
                      <td className="px-4 py-3 hidden md:table-cell"><span className="text-xs">v{p.config_version}</span></td>
                      <td className="px-4 py-3"><div className="flex justify-end gap-1">
                        <Button variant="ghost" size="icon" className="h-8 w-8" onClick={() => setDeleteTarget(p.instance_id)}><Trash2 className="h-3.5 w-3.5 text-red-400" /></Button>
                      </div></td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={open => { if (!open) setDeleteTarget(null) }}
        title="Remove Proxy"
        description={`Are you sure you want to remove proxy "${deleteTarget}"? This action cannot be undone.`}
        confirmText="Remove"
        onConfirm={() => { if (deleteTarget) remove(deleteTarget) }}
      />
    </div>
  )
}
