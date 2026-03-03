import { useEffect, useState } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Activity, Box, GitBranch, Shield, Gauge, ScrollText } from "lucide-react"

interface Status {
  providers: number
  models: number
  variants: number
  rate_limits: number
  auth_enabled: boolean
  concurrency_limits: number
  access_log_enabled: boolean
}

export default function Dashboard() {
  const [status, setStatus] = useState<Status | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    fetch("/api/status")
      .then(r => r.json())
      .then(setStatus)
      .catch(e => setError(e.message))
  }, [])

  if (error) return <div className="p-8 text-red-400">Error: {error}</div>
  if (!status) return <div className="p-8 text-[hsl(var(--muted-foreground))]">Loading...</div>

  const cards = [
    { title: "Providers", value: status.providers, icon: Box, color: "text-blue-400" },
    { title: "Models", value: status.models, icon: Activity, color: "text-green-400" },
    { title: "Variants", value: status.variants, icon: GitBranch, color: "text-purple-400" },
    { title: "Rate Limits", value: status.rate_limits, icon: Gauge, color: "text-yellow-400" },
    { title: "Auth", value: status.auth_enabled ? "ON" : "OFF", icon: Shield, color: status.auth_enabled ? "text-green-400" : "text-[hsl(var(--muted-foreground))]" },
    { title: "Access Log", value: status.access_log_enabled ? "ON" : "OFF", icon: ScrollText, color: status.access_log_enabled ? "text-green-400" : "text-[hsl(var(--muted-foreground))]" },
  ]

  return (
    <div>
      <h1 className="text-2xl font-bold mb-6">Dashboard</h1>
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {cards.map(c => (
          <Card key={c.title}>
            <CardHeader className="flex flex-row items-center justify-between pb-2">
              <CardTitle className="text-sm font-medium text-[hsl(var(--muted-foreground))]">{c.title}</CardTitle>
              <c.icon className={`h-4 w-4 ${c.color}`} />
            </CardHeader>
            <CardContent>
              <div className={`text-2xl font-bold ${c.color}`}>{c.value}</div>
            </CardContent>
          </Card>
        ))}
      </div>
    </div>
  )
}
