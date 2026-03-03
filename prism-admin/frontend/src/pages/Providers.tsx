import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Trash2, ExternalLink } from "lucide-react"

interface Provider {
  id: string
  base_url: string
  protocols: string[]
  circuit_breaker_threshold: number
  circuit_breaker_recovery_secs: number
}

export default function Providers() {
  const [providers, setProviders] = useState<Provider[]>([])
  const [loading, setLoading] = useState(true)

  const load = () => {
    setLoading(true)
    fetch("/api/providers")
      .then(r => r.json())
      .then(d => { setProviders(d.providers || []); setLoading(false) })
      .catch(() => setLoading(false))
  }

  useEffect(load, [])

  const handleDelete = async (id: string) => {
    if (!confirm(`Delete provider "${id}"?`)) return
    await fetch(`/api/providers/${id}`, { method: "DELETE" })
    load()
  }

  if (loading) return <div className="p-8 text-[hsl(var(--muted-foreground))]">Loading...</div>

  return (
    <div>
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-2xl font-bold">Providers</h1>
        <span className="text-sm text-[hsl(var(--muted-foreground))]">{providers.length} total</span>
      </div>
      <div className="space-y-3">
        {providers.map(p => (
          <Card key={p.id}>
            <CardContent className="p-4 flex items-center justify-between">
              <div className="flex-1">
                <div className="font-medium text-lg">{p.id}</div>
                <div className="text-sm text-[hsl(var(--muted-foreground))] flex items-center gap-1">
                  <ExternalLink className="h-3 w-3" />
                  {p.base_url}
                </div>
                <div className="flex gap-2 mt-1">
                  {p.protocols.map(proto => (
                    <span key={proto} className="text-xs bg-[hsl(var(--secondary))] px-2 py-0.5 rounded">{proto}</span>
                  ))}
                  <span className="text-xs text-[hsl(var(--muted-foreground))]">
                    CB: {p.circuit_breaker_threshold} failures / {p.circuit_breaker_recovery_secs}s recovery
                  </span>
                </div>
              </div>
              <Button variant="ghost" size="icon" onClick={() => handleDelete(p.id)}>
                <Trash2 className="h-4 w-4 text-red-400" />
              </Button>
            </CardContent>
          </Card>
        ))}
        {providers.length === 0 && (
          <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">No providers configured</div>
        )}
      </div>
    </div>
  )
}
