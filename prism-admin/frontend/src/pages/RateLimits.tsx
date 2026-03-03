import { useEffect, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"

interface RateLimit { name: string; window_secs: number; max_requests: number; enabled: boolean }

export default function RateLimits() {
  const [rules, setRules] = useState<RateLimit[]>([])
  useEffect(() => {
    fetch("/api/rate-limits").then(r => r.json()).then(d => setRules(d.rate_limits || []))
  }, [])

  return (
    <div>
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-2xl font-bold">Rate Limits</h1>
        <span className="text-sm text-[hsl(var(--muted-foreground))]">{rules.length} rules</span>
      </div>
      <div className="space-y-2">
        {rules.map(r => (
          <Card key={r.name}>
            <CardContent className="p-4 flex items-center justify-between">
              <div>
                <div className="font-medium">{r.name}</div>
                <div className="text-sm text-[hsl(var(--muted-foreground))]">
                  {r.max_requests} req / {r.window_secs}s
                </div>
              </div>
              <span className={`text-xs px-2 py-1 rounded ${r.enabled ? "bg-green-900 text-green-300" : "bg-red-900 text-red-300"}`}>
                {r.enabled ? "enabled" : "disabled"}
              </span>
            </CardContent>
          </Card>
        ))}
        {rules.length === 0 && (
          <div className="text-center py-12 text-[hsl(var(--muted-foreground))]">No rate limit rules</div>
        )}
      </div>
    </div>
  )
}
