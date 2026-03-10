import { useState } from "react"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { setToken } from "@/lib/api"

export default function LoginGate({ onAuth }: { onAuth: () => void }) {
  const [token, setTokenInput] = useState("")
  const [error, setError] = useState("")
  const [loading, setLoading] = useState(false)

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    setError("")
    setLoading(true)
    try {
      const res = await fetch("/api/models", {
        headers: { Authorization: `Bearer ${token}` },
      })
      if (res.ok) {
        setToken(token)
        onAuth()
      } else {
        setError("Invalid token")
      }
    } catch {
      setError("Connection failed")
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="min-h-screen flex items-center justify-center bg-[hsl(var(--background))]">
      <Card className="w-full max-w-sm">
        <CardHeader className="text-center">
          <CardTitle className="text-xl bg-gradient-to-r from-blue-400 to-purple-400 bg-clip-text text-transparent">
            Prism Admin
          </CardTitle>
          <p className="text-sm text-[hsl(var(--muted-foreground))]">Enter admin token to continue</p>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="token">Admin Token</Label>
              <Input
                id="token"
                type="password"
                placeholder="Enter token..."
                value={token}
                onChange={e => setTokenInput(e.target.value)}
                autoFocus
              />
            </div>
            {error && <p className="text-sm text-red-500">{error}</p>}
            <Button type="submit" className="w-full" disabled={loading || !token}>
              {loading ? "Verifying..." : "Sign In"}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  )
}
