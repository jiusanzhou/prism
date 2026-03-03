import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"

interface Variant { id: string; model_id: string; provider_id: string; weight: number }

export default function Variants() {
  const [variants, setVariants] = useState<Variant[]>([])
  useEffect(() => {
    fetch("/api/variants").then(r => r.json()).then(d => setVariants(d.variants || []))
  }, [])

  return (
    <div>
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-2xl font-bold">Variants</h1>
        <span className="text-sm text-[hsl(var(--muted-foreground))]">{variants.length} total</span>
      </div>
      <div className="space-y-2">
        {variants.map(v => (
          <Card key={v.id}>
            <CardContent className="p-4 flex items-center gap-4">
              <div className="flex-1">
                <div className="font-medium">{v.id}</div>
                <div className="text-sm text-[hsl(var(--muted-foreground))]">
                  {v.provider_id} → {v.model_id}
                </div>
              </div>
              <span className="text-xs bg-[hsl(var(--accent))] px-2 py-1 rounded">weight: {v.weight}</span>
            </CardContent>
          </Card>
        ))}
      </div>
    </div>
  )
}
