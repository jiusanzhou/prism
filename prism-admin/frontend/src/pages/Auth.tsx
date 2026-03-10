import { useEffect, useState } from "react"
import { api } from "@/lib/api"
import { usePageTitle } from "@/hooks/usePageTitle"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"
import { Plus, X, Save } from "lucide-react"

interface AuthConfig {
  enabled: boolean
  header: string
  keys: string[]
  strip_prefix: string | null
  skip_paths: string[]
}

export default function Auth() {
  usePageTitle("Auth")
  const [form, setForm] = useState<AuthConfig>({
    enabled: false, header: "authorization", keys: [], strip_prefix: "Bearer ", skip_paths: [],
  })
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)

  useEffect(() => {
    api("/api/auth").then(r => r.json()).then(d => {
      setForm({
        enabled: d.enabled ?? false,
        header: d.header || "authorization",
        keys: d.keys || [],
        strip_prefix: d.strip_prefix ?? "Bearer ",
        skip_paths: d.skip_paths || [],
      })
      setLoading(false)
    }).catch(() => setLoading(false))
  }, [])

  const handleSave = async () => {
    setSaving(true)
    await api("/api/auth", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(form),
    })
    setSaving(false)
    setSaved(true)
    setTimeout(() => setSaved(false), 2000)
  }

  if (loading) return <div className="text-[hsl(var(--muted-foreground))]">Loading...</div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <h1 className="text-xl sm:text-2xl font-bold">Authentication</h1>
        <Button size="sm" onClick={handleSave} disabled={saving}>
          <Save className="h-4 w-4 mr-1" /> {saving ? "Saving..." : saved ? "Saved!" : "Save"}
        </Button>
      </div>
      <div className="max-w-2xl space-y-4">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">General</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex items-center gap-3">
              <Switch checked={form.enabled} onCheckedChange={v => setForm(f => ({ ...f, enabled: v }))} />
              <Label>Authentication Enabled</Label>
            </div>
            <div>
              <Label>Header Name</Label>
              <Input value={form.header} onChange={e => setForm(f => ({ ...f, header: e.target.value }))} />
            </div>
            <div>
              <Label>Strip Prefix</Label>
              <Input value={form.strip_prefix || ""} onChange={e => setForm(f => ({ ...f, strip_prefix: e.target.value || null }))} placeholder="Bearer " />
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <CardTitle className="text-base">API Keys</CardTitle>
              <Button variant="ghost" size="sm" onClick={() => setForm(f => ({ ...f, keys: [...f.keys, ""] }))}><Plus className="h-3 w-3 mr-1" /> Add</Button>
            </div>
          </CardHeader>
          <CardContent className="space-y-2">
            {form.keys.map((k, i) => (
              <div key={i} className="flex gap-2">
                <Input
                  type="password"
                  value={k}
                  onChange={e => setForm(f => ({ ...f, keys: f.keys.map((v, idx) => idx === i ? e.target.value : v) }))}
                  placeholder="sk-..."
                />
                <Button variant="ghost" size="icon" onClick={() => setForm(f => ({ ...f, keys: f.keys.filter((_, idx) => idx !== i) }))}><X className="h-4 w-4" /></Button>
              </div>
            ))}
            {form.keys.length === 0 && <span className="text-sm text-[hsl(var(--muted-foreground))]">No API keys configured</span>}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <CardTitle className="text-base">Skip Paths</CardTitle>
              <Button variant="ghost" size="sm" onClick={() => setForm(f => ({ ...f, skip_paths: [...f.skip_paths, ""] }))}><Plus className="h-3 w-3 mr-1" /> Add</Button>
            </div>
          </CardHeader>
          <CardContent className="space-y-2">
            {form.skip_paths.map((p, i) => (
              <div key={i} className="flex gap-2">
                <Input
                  value={p}
                  onChange={e => setForm(f => ({ ...f, skip_paths: f.skip_paths.map((v, idx) => idx === i ? e.target.value : v) }))}
                  placeholder="/health"
                />
                <Button variant="ghost" size="icon" onClick={() => setForm(f => ({ ...f, skip_paths: f.skip_paths.filter((_, idx) => idx !== i) }))}><X className="h-4 w-4" /></Button>
              </div>
            ))}
            {form.skip_paths.length === 0 && <span className="text-sm text-[hsl(var(--muted-foreground))]">No paths configured</span>}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
