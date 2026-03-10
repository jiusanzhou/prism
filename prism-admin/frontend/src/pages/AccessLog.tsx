import { useEffect, useState } from "react"
import { api } from "@/lib/api"
import { usePageTitle } from "@/hooks/usePageTitle"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Textarea } from "@/components/ui/textarea"
import { Plus, X, Save } from "lucide-react"

interface AccessLogConfig {
  enabled: boolean
  output: string
  file_path: string | null
  format: string
  text_format: string
  fields: string[]
  max_file_size_mb: number
  max_files: number
}

export default function AccessLog() {
  usePageTitle("Access Log")
  const [form, setForm] = useState<AccessLogConfig>({
    enabled: false, output: "stdout", file_path: null, format: "json",
    text_format: "", fields: [], max_file_size_mb: 100, max_files: 5,
  })
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)

  useEffect(() => {
    api("/api/access-log").then(r => r.json()).then(d => {
      setForm({
        enabled: d.enabled ?? false,
        output: d.output || "stdout",
        file_path: d.file_path || null,
        format: d.format || "json",
        text_format: d.text_format || "",
        fields: d.fields || [],
        max_file_size_mb: d.max_file_size_mb ?? 100,
        max_files: d.max_files ?? 5,
      })
      setLoading(false)
    }).catch(() => setLoading(false))
  }, [])

  const handleSave = async () => {
    setSaving(true)
    await api("/api/access-log", {
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
        <h1 className="text-xl sm:text-2xl font-bold">Access Log</h1>
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
              <Label>Access Logging Enabled</Label>
            </div>
            <div>
              <Label>Output</Label>
              <Select value={form.output} onValueChange={v => setForm(f => ({ ...f, output: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="stdout">Stdout</SelectItem>
                  <SelectItem value="file">File</SelectItem>
                  <SelectItem value="both">Both</SelectItem>
                </SelectContent>
              </Select>
            </div>
            {(form.output === "file" || form.output === "both") && (
              <div>
                <Label>File Path</Label>
                <Input value={form.file_path || ""} onChange={e => setForm(f => ({ ...f, file_path: e.target.value || null }))} placeholder="/var/log/prism/access.log" />
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">Format</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div>
              <Label>Format</Label>
              <Select value={form.format} onValueChange={v => setForm(f => ({ ...f, format: v }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="json">JSON</SelectItem>
                  <SelectItem value="text">Text</SelectItem>
                </SelectContent>
              </Select>
            </div>
            {form.format === "text" && (
              <div>
                <Label>Text Format Template</Label>
                <Textarea
                  value={form.text_format}
                  onChange={e => setForm(f => ({ ...f, text_format: e.target.value }))}
                  className="font-mono text-xs min-h-[80px]"
                  placeholder='[%START_TIME%] "%METHOD% %PATH%" %RESPONSE_CODE%'
                />
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <CardTitle className="text-base">Fields (JSON format)</CardTitle>
              <Button variant="ghost" size="sm" onClick={() => setForm(f => ({ ...f, fields: [...f.fields, ""] }))}><Plus className="h-3 w-3 mr-1" /> Add</Button>
            </div>
          </CardHeader>
          <CardContent className="space-y-2">
            {form.fields.map((field, i) => (
              <div key={i} className="flex gap-2">
                <Input
                  value={field}
                  onChange={e => setForm(f => ({ ...f, fields: f.fields.map((v, idx) => idx === i ? e.target.value : v) }))}
                  placeholder="%METHOD%"
                />
                <Button variant="ghost" size="icon" onClick={() => setForm(f => ({ ...f, fields: f.fields.filter((_, idx) => idx !== i) }))}><X className="h-4 w-4" /></Button>
              </div>
            ))}
            {form.fields.length === 0 && <span className="text-sm text-[hsl(var(--muted-foreground))]">Empty = all default fields</span>}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">Rotation</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <Label>Max File Size (MB)</Label>
                <Input type="number" value={form.max_file_size_mb} onChange={e => setForm(f => ({ ...f, max_file_size_mb: parseInt(e.target.value) || 100 }))} />
              </div>
              <div>
                <Label>Max Files</Label>
                <Input type="number" value={form.max_files} onChange={e => setForm(f => ({ ...f, max_files: parseInt(e.target.value) || 5 }))} />
              </div>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
