import { useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Badge } from "@/components/ui/badge"
import { Textarea } from "@/components/ui/textarea"
import { Plus, Trash2, X, Upload, Download, CheckCircle2, XCircle, Loader2 } from "lucide-react"

interface RawProvider {
  name: string
  type: string
  protocol?: string
  tokens: string[] | null
  rawConfigs?: {
    openaiCustomUrl?: string
    azureBaseUrl?: string
    azureApiVersion?: string
    azureDeployments?: Record<string, string>
    id?: string
    type?: string
    apiTokens?: string[]
    [key: string]: unknown
  }
}

interface MappedProvider {
  provider: {
    id: string
    base_url: string
    protocols: string[]
    circuit_breaker_threshold: number
    circuit_breaker_recovery_secs: number
  }
  accounts: {
    id: string
    provider_id: string
    api_key: string
    enabled: boolean
    weight: number
  }[]
}

interface ImportResult {
  id: string
  type: "provider" | "account"
  status: "success" | "error"
  error?: string
}

type Step = "input" | "preview" | "importing" | "done"

function parseCurl(curlCmd: string): { url: string; headers: { key: string; value: string }[] } {
  const headers: { key: string; value: string }[] = []

  // Normalize line continuations and collapse
  const normalized = curlCmd.replace(/\\\s*\n/g, " ").trim()

  // Extract URL: first bare argument after 'curl' that looks like a URL
  let url = ""
  // Try quoted URL after curl
  const urlMatchDouble = normalized.match(/curl\s+["']([^"']+)["']/)
  const urlMatchBare = normalized.match(/curl\s+(?:-[^\s]+\s+(?:["'][^"']*["']|[^\s]+)\s+)*\s*(https?:\/\/[^\s"']+)/)
  if (urlMatchDouble) {
    url = urlMatchDouble[1]
  } else if (urlMatchBare) {
    url = urlMatchBare[1]
  } else {
    // Fallback: find any URL in the string
    const anyUrl = normalized.match(/(https?:\/\/[^\s"']+)/)
    if (anyUrl) url = anyUrl[1]
  }

  // Extract -H headers (handle both single and double quotes)
  const headerRegex = /-H\s+(?:"([^"]*?)"|'([^']*?)')/g
  let match
  while ((match = headerRegex.exec(normalized)) !== null) {
    const headerStr = match[1] || match[2]
    const colonIdx = headerStr.indexOf(":")
    if (colonIdx > 0) {
      headers.push({
        key: headerStr.substring(0, colonIdx).trim(),
        value: headerStr.substring(colonIdx + 1).trim(),
      })
    }
  }

  // Extract -b cookies
  const cookieRegex = /-b\s+(?:"([^"]*?)"|'([^']*?)'|(\S+))/g
  while ((match = cookieRegex.exec(normalized)) !== null) {
    const cookieVal = match[1] || match[2] || match[3]
    if (cookieVal) {
      const existing = headers.find(h => h.key.toLowerCase() === "cookie")
      if (existing) {
        existing.value += "; " + cookieVal
      } else {
        headers.push({ key: "Cookie", value: cookieVal })
      }
    }
  }

  return { url, headers }
}

function mapProvider(raw: RawProvider): MappedProvider {
  const base_url = raw.rawConfigs?.openaiCustomUrl || raw.rawConfigs?.azureBaseUrl || ""
  const protocols = [raw.type || "openai"]

  const provider = {
    id: raw.name,
    base_url,
    protocols,
    circuit_breaker_threshold: 5,
    circuit_breaker_recovery_secs: 30,
  }

  const tokens = raw.tokens || raw.rawConfigs?.apiTokens || []
  const accounts = tokens.map((token: string, i: number) => ({
    id: `${raw.name}-${i + 1}`,
    provider_id: raw.name,
    api_key: token,
    enabled: true,
    weight: 1.0,
  }))

  return { provider, accounts }
}

export default function Tools() {
  const [step, setStep] = useState<Step>("input")
  const [mode, setMode] = useState<"curl" | "manual">("curl")
  const [curlInput, setCurlInput] = useState("")
  const [manualUrl, setManualUrl] = useState("")
  const [manualHeaders, setManualHeaders] = useState<{ key: string; value: string }[]>([])
  const [fetching, setFetching] = useState(false)
  const [fetchError, setFetchError] = useState("")

  const [rawProviders, setRawProviders] = useState<RawProvider[]>([])
  const [mapped, setMapped] = useState<MappedProvider[]>([])
  const [selected, setSelected] = useState<Set<number>>(new Set())

  const [results, setResults] = useState<ImportResult[]>([])

  const handleFetch = async () => {
    setFetchError("")
    setFetching(true)

    let url: string
    let headers: Record<string, string> = {}

    if (mode === "curl") {
      const parsed = parseCurl(curlInput)
      url = parsed.url
      parsed.headers.forEach(h => { headers[h.key] = h.value })
    } else {
      url = manualUrl
      manualHeaders.forEach(h => { if (h.key) headers[h.key] = h.value })
    }

    if (!url) {
      setFetchError("No URL provided")
      setFetching(false)
      return
    }

    try {
      const proxyRes = await fetch("/api/proxy-fetch", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ url, headers }),
      })
      if (!proxyRes.ok) {
        const errText = await proxyRes.text()
        throw new Error(`Proxy fetch failed: ${errText}`)
      }
      const proxyJson = await proxyRes.json()
      if (proxyJson.status >= 400) throw new Error(`Remote returned HTTP ${proxyJson.status}`)
      const json = proxyJson.body

      const data: RawProvider[] = json.data || json
      if (!Array.isArray(data)) throw new Error("Response data is not an array")

      setRawProviders(data)
      const mappedList = data.map(mapProvider)
      setMapped(mappedList)
      setSelected(new Set(mappedList.map((_, i) => i)))
      setStep("preview")
    } catch (err: unknown) {
      setFetchError(err instanceof Error ? err.message : "Fetch failed")
    } finally {
      setFetching(false)
    }
  }

  const toggleSelect = (i: number) => {
    setSelected(prev => {
      const next = new Set(prev)
      next.has(i) ? next.delete(i) : next.add(i)
      return next
    })
  }

  const selectAll = () => setSelected(new Set(mapped.map((_, i) => i)))
  const deselectAll = () => setSelected(new Set())

  const handleImport = async () => {
    setStep("importing")
    const importResults: ImportResult[] = []

    for (const i of Array.from(selected)) {
      const m = mapped[i]

      // Upsert provider
      try {
        const res = await fetch(`/api/providers/${m.provider.id}`, {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            base_url: m.provider.base_url,
            protocols: m.provider.protocols,
            circuit_breaker_threshold: m.provider.circuit_breaker_threshold,
            circuit_breaker_recovery_secs: m.provider.circuit_breaker_recovery_secs,
          }),
        })
        if (!res.ok) throw new Error(`HTTP ${res.status}`)
        importResults.push({ id: m.provider.id, type: "provider", status: "success" })
      } catch (err: unknown) {
        importResults.push({ id: m.provider.id, type: "provider", status: "error", error: err instanceof Error ? err.message : "Failed" })
      }

      // Create accounts
      for (const acct of m.accounts) {
        try {
          const res = await fetch("/api/accounts", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(acct),
          })
          if (!res.ok) throw new Error(`HTTP ${res.status}`)
          importResults.push({ id: acct.id, type: "account", status: "success" })
        } catch (err: unknown) {
          importResults.push({ id: acct.id, type: "account", status: "error", error: err instanceof Error ? err.message : "Failed" })
        }
      }
    }

    setResults(importResults)
    setStep("done")
  }

  const reset = () => {
    setStep("input")
    setCurlInput("")
    setManualUrl("")
    setManualHeaders([])
    setRawProviders([])
    setMapped([])
    setSelected(new Set())
    setResults([])
    setFetchError("")
  }

  const addManualHeader = () => setManualHeaders(h => [...h, { key: "", value: "" }])
  const removeManualHeader = (i: number) => setManualHeaders(h => h.filter((_, idx) => idx !== i))
  const updateManualHeader = (i: number, field: "key" | "value", val: string) =>
    setManualHeaders(h => h.map((hdr, idx) => idx === i ? { ...hdr, [field]: val } : hdr))

  const successCount = results.filter(r => r.status === "success").length
  const errorCount = results.filter(r => r.status === "error").length

  return (
    <div>
      <div className="flex items-center justify-between mb-4 sm:mb-6">
        <h1 className="text-xl sm:text-2xl font-bold">Tools</h1>
      </div>

      <Card>
        <CardContent className="p-4 sm:p-6">
          <div className="flex items-center gap-2 mb-4">
            <Download className="h-5 w-5" />
            <h2 className="text-lg font-semibold">Import from LLM Gateway</h2>
          </div>

          {/* Step indicator */}
          <div className="flex items-center gap-2 mb-6 text-sm">
            {(["input", "preview", "done"] as const).map((s, i) => (
              <div key={s} className="flex items-center gap-2">
                {i > 0 && <div className="w-8 h-px bg-[hsl(var(--border))]" />}
                <div className={`px-2.5 py-1 rounded-full text-xs font-medium ${
                  step === s || (step === "importing" && s === "done")
                    ? "bg-[hsl(var(--primary))] text-[hsl(var(--primary-foreground))]"
                    : (["input", "preview", "importing", "done"].indexOf(step) > ["input", "preview", "importing", "done"].indexOf(s))
                      ? "bg-[hsl(var(--secondary))] text-[hsl(var(--secondary-foreground))]"
                      : "bg-[hsl(var(--muted))] text-[hsl(var(--muted-foreground))]"
                }`}>
                  {i + 1}. {s === "input" ? "Fetch" : s === "preview" ? "Preview" : "Results"}
                </div>
              </div>
            ))}
          </div>

          {/* Step 1: Input */}
          {step === "input" && (
            <div className="space-y-4">
              <div className="flex gap-2">
                <Button
                  size="sm"
                  variant={mode === "curl" ? "default" : "outline"}
                  onClick={() => setMode("curl")}
                >
                  Paste cURL
                </Button>
                <Button
                  size="sm"
                  variant={mode === "manual" ? "default" : "outline"}
                  onClick={() => setMode("manual")}
                >
                  Manual
                </Button>
              </div>

              {mode === "curl" ? (
                <div>
                  <Label>cURL Command</Label>
                  <Textarea
                    value={curlInput}
                    onChange={e => setCurlInput(e.target.value)}
                    placeholder={"curl 'https://api.example.com/providers' \\\n  -H 'Authorization: Bearer token' \\\n  -b 'session=abc123'"}
                    className="font-mono text-xs min-h-[120px]"
                  />
                </div>
              ) : (
                <div className="space-y-3">
                  <div>
                    <Label>URL</Label>
                    <Input
                      value={manualUrl}
                      onChange={e => setManualUrl(e.target.value)}
                      placeholder="https://api.example.com/providers"
                    />
                  </div>
                  <div>
                    <div className="flex items-center justify-between mb-2">
                      <Label>Headers</Label>
                      <Button variant="ghost" size="sm" onClick={addManualHeader}>
                        <Plus className="h-3 w-3 mr-1" /> Add
                      </Button>
                    </div>
                    {manualHeaders.map((h, i) => (
                      <div key={i} className="flex gap-2 mb-2">
                        <Input placeholder="Key" value={h.key} onChange={e => updateManualHeader(i, "key", e.target.value)} className="flex-1" />
                        <Input placeholder="Value" value={h.value} onChange={e => updateManualHeader(i, "value", e.target.value)} className="flex-1" />
                        <Button variant="ghost" size="icon" onClick={() => removeManualHeader(i)}>
                          <X className="h-4 w-4" />
                        </Button>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {fetchError && (
                <div className="text-sm text-red-400 bg-red-400/10 border border-red-400/20 rounded-md px-3 py-2">
                  {fetchError}
                </div>
              )}

              <Button onClick={handleFetch} disabled={fetching}>
                {fetching ? <><Loader2 className="h-4 w-4 mr-2 animate-spin" /> Fetching...</> : <><Upload className="h-4 w-4 mr-2" /> Fetch Providers</>}
              </Button>
            </div>
          )}

          {/* Step 2: Preview */}
          {step === "preview" && (
            <div className="space-y-4">
              <div className="flex items-center justify-between">
                <span className="text-sm text-[hsl(var(--muted-foreground))]">
                  {rawProviders.length} providers found, {selected.size} selected
                </span>
                <div className="flex gap-2">
                  <Button size="sm" variant="outline" onClick={selectAll}>Select All</Button>
                  <Button size="sm" variant="outline" onClick={deselectAll}>Deselect All</Button>
                </div>
              </div>

              <div className="overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-[hsl(var(--border))]">
                      <th className="text-left py-2 px-2 w-8">
                        <input
                          type="checkbox"
                          checked={selected.size === mapped.length && mapped.length > 0}
                          onChange={() => selected.size === mapped.length ? deselectAll() : selectAll()}
                          className="rounded"
                        />
                      </th>
                      <th className="text-left py-2 px-2">Name</th>
                      <th className="text-left py-2 px-2">Type</th>
                      <th className="text-left py-2 px-2">Base URL</th>
                      <th className="text-left py-2 px-2">Tokens</th>
                    </tr>
                  </thead>
                  <tbody>
                    {mapped.map((m, i) => (
                      <tr key={i} className="border-b border-[hsl(var(--border))] hover:bg-[hsl(var(--accent)/0.5)]">
                        <td className="py-2 px-2">
                          <input
                            type="checkbox"
                            checked={selected.has(i)}
                            onChange={() => toggleSelect(i)}
                            className="rounded"
                          />
                        </td>
                        <td className="py-2 px-2 font-medium">{m.provider.id}</td>
                        <td className="py-2 px-2">
                          {m.provider.protocols.map(p => (
                            <Badge key={p} variant="secondary" className="mr-1">{p}</Badge>
                          ))}
                        </td>
                        <td className="py-2 px-2 text-[hsl(var(--muted-foreground))] truncate max-w-[200px]">
                          {m.provider.base_url || "—"}
                        </td>
                        <td className="py-2 px-2">
                          <Badge variant="outline">{m.accounts.length}</Badge>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>

              {/* Mapping details */}
              {selected.size > 0 && (
                <details className="text-sm">
                  <summary className="cursor-pointer text-[hsl(var(--muted-foreground))] hover:text-[hsl(var(--foreground))]">
                    Mapping details ({selected.size} selected)
                  </summary>
                  <div className="mt-2 space-y-2 max-h-64 overflow-y-auto">
                    {Array.from(selected).map(i => {
                      const m = mapped[i]
                      const raw = rawProviders[i]
                      return (
                        <div key={i} className="p-3 rounded border border-[hsl(var(--border))] bg-[hsl(var(--muted)/0.3)]">
                          <div className="font-medium mb-1">{raw.name}</div>
                          <div className="grid grid-cols-1 sm:grid-cols-2 gap-x-4 gap-y-1 text-xs text-[hsl(var(--muted-foreground))]">
                            <div>Provider ID: <span className="text-[hsl(var(--foreground))]">{m.provider.id}</span></div>
                            <div>Protocols: <span className="text-[hsl(var(--foreground))]">{m.provider.protocols.join(", ")}</span></div>
                            <div>Base URL: <span className="text-[hsl(var(--foreground))]">{m.provider.base_url || "—"}</span></div>
                            <div>Accounts: <span className="text-[hsl(var(--foreground))]">{m.accounts.length} ({m.accounts.map(a => a.id).join(", ")})</span></div>
                          </div>
                        </div>
                      )
                    })}
                  </div>
                </details>
              )}

              <div className="flex gap-2">
                <Button variant="outline" onClick={reset}>Back</Button>
                <Button onClick={handleImport} disabled={selected.size === 0}>
                  <Upload className="h-4 w-4 mr-2" /> Import {selected.size} Selected
                </Button>
              </div>
            </div>
          )}

          {/* Step 3: Importing */}
          {step === "importing" && (
            <div className="flex items-center gap-3 py-8 justify-center">
              <Loader2 className="h-5 w-5 animate-spin" />
              <span className="text-[hsl(var(--muted-foreground))]">Importing providers and accounts...</span>
            </div>
          )}

          {/* Step 4: Done */}
          {step === "done" && (
            <div className="space-y-4">
              <div className="flex items-center gap-4">
                {errorCount === 0 ? (
                  <div className="flex items-center gap-2 text-green-500">
                    <CheckCircle2 className="h-5 w-5" />
                    <span className="font-medium">All {successCount} operations succeeded</span>
                  </div>
                ) : (
                  <div className="flex items-center gap-4">
                    <div className="flex items-center gap-2 text-green-500">
                      <CheckCircle2 className="h-5 w-5" />
                      <span>{successCount} succeeded</span>
                    </div>
                    <div className="flex items-center gap-2 text-red-400">
                      <XCircle className="h-5 w-5" />
                      <span>{errorCount} failed</span>
                    </div>
                  </div>
                )}
              </div>

              <div className="overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-[hsl(var(--border))]">
                      <th className="text-left py-2 px-2">ID</th>
                      <th className="text-left py-2 px-2">Type</th>
                      <th className="text-left py-2 px-2">Status</th>
                      <th className="text-left py-2 px-2">Details</th>
                    </tr>
                  </thead>
                  <tbody>
                    {results.map((r, i) => (
                      <tr key={i} className="border-b border-[hsl(var(--border))]">
                        <td className="py-2 px-2 font-medium">{r.id}</td>
                        <td className="py-2 px-2">
                          <Badge variant="secondary">{r.type}</Badge>
                        </td>
                        <td className="py-2 px-2">
                          {r.status === "success" ? (
                            <Badge variant="default" className="bg-green-600">success</Badge>
                          ) : (
                            <Badge variant="destructive">error</Badge>
                          )}
                        </td>
                        <td className="py-2 px-2 text-[hsl(var(--muted-foreground))]">{r.error || "—"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>

              <Button onClick={reset}>
                <Trash2 className="h-4 w-4 mr-2" /> Start New Import
              </Button>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
