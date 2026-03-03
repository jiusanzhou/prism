import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"

interface Model { id: string; display_name: string; quality_tier: string }

export default function Models() {
  const [models, setModels] = useState<Model[]>([])
  useEffect(() => {
    fetch("/api/models").then(r => r.json()).then(d => setModels(d.models || []))
  }, [])

  return (
    <div>
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-2xl font-bold">Models</h1>
        <span className="text-sm text-[hsl(var(--muted-foreground))]">{models.length} total</span>
      </div>
      <div className="space-y-2">
        {models.map(m => (
          <Card key={m.id}>
            <CardContent className="p-4 flex items-center justify-between">
              <div>
                <div className="font-medium">{m.display_name}</div>
                <div className="text-sm text-[hsl(var(--muted-foreground))]">{m.id}</div>
              </div>
              <span className="text-xs bg-[hsl(var(--secondary))] px-2 py-1 rounded">{m.quality_tier}</span>
            </CardContent>
          </Card>
        ))}
      </div>
    </div>
  )
}
