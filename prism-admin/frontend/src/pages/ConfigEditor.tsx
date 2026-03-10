import { useEffect, useState } from "react"
import { api } from "@/lib/api"
import { usePageTitle } from "@/hooks/usePageTitle"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from "@/components/ui/dialog"
import { Textarea } from "@/components/ui/textarea"
import { Download, Upload, CheckCircle, AlertTriangle } from "lucide-react"

interface ValidationResult {
  status: string
  providers?: number
  models?: number
  variants?: number
  warnings?: string[]
  error?: string
}

export default function ConfigEditor() {
  usePageTitle("Config")
  const [content, setContent] = useState("")
  const [loading, setLoading] = useState(true)
  const [validation, setValidation] = useState<ValidationResult | null>(null)
  const [saving, setSaving] = useState(false)
  const [confirmOpen, setConfirmOpen] = useState(false)

  const loadConfig = () => {
    setLoading(true)
    api("/api/config")
      .then(r => r.json())
      .then(data => {
        setContent(JSON.stringify(data, null, 2))
        setLoading(false)
      })
      .catch(() => setLoading(false))
  }

  useEffect(loadConfig, [])

  const handleValidate = async () => {
    const res = await api("/api/config/validate", {
      method: "POST",
      headers: { "Content-Type": "text/plain" },
      body: content,
    })
    const data = await res.json()
    setValidation(data)
  }

  const handleSave = async () => {
    setSaving(true)
    await api("/api/config", {
      method: "PUT",
      headers: { "Content-Type": "text/plain" },
      body: content,
    })
    setSaving(false)
    setConfirmOpen(false)
    setValidation(null)
  }

  if (loading) return <div className="text-[hsl(var(--muted-foreground))]">Loading...</div>

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6 flex-wrap gap-2">
        <h1 className="text-xl sm:text-2xl font-bold">Config Editor</h1>
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={loadConfig}><Download className="h-4 w-4 mr-1" /> Load</Button>
          <Button variant="outline" size="sm" onClick={handleValidate}><CheckCircle className="h-4 w-4 mr-1" /> Validate</Button>
          <Button size="sm" onClick={() => setConfirmOpen(true)}><Upload className="h-4 w-4 mr-1" /> Save</Button>
        </div>
      </div>

      {validation && (
        <Card className="mb-4">
          <CardContent className="p-4">
            {validation.status === "valid" ? (
              <div>
                <div className="flex items-center gap-2 text-green-400 mb-2">
                  <CheckCircle className="h-4 w-4" />
                  <span className="font-medium">Valid</span>
                  <span className="text-sm text-[hsl(var(--muted-foreground))]">
                    {validation.providers} providers, {validation.models} models, {validation.variants} variants
                  </span>
                </div>
                {validation.warnings && validation.warnings.length > 0 && (
                  <div className="space-y-1">
                    {validation.warnings.map((w, i) => (
                      <div key={i} className="flex items-center gap-2 text-yellow-400 text-sm">
                        <AlertTriangle className="h-3 w-3 shrink-0" />
                        {w}
                      </div>
                    ))}
                  </div>
                )}
              </div>
            ) : (
              <div className="flex items-center gap-2 text-red-400">
                <AlertTriangle className="h-4 w-4" />
                <span className="font-medium">Invalid: </span>
                <span className="text-sm">{validation.error}</span>
              </div>
            )}
          </CardContent>
        </Card>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="text-sm text-[hsl(var(--muted-foreground))]">
            YAML / JSON Configuration
          </CardTitle>
        </CardHeader>
        <CardContent>
          <Textarea
            value={content}
            onChange={e => setContent(e.target.value)}
            className="font-mono text-xs min-h-[60vh] resize-y"
            spellCheck={false}
          />
        </CardContent>
      </Card>

      <Dialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Save Configuration</DialogTitle>
            <DialogDescription>
              This will overwrite the config file on disk. The router will need to be reloaded to apply changes.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConfirmOpen(false)}>Cancel</Button>
            <Button onClick={handleSave} disabled={saving}>{saving ? "Saving..." : "Confirm Save"}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
