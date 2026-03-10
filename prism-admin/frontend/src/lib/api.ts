const TOKEN_KEY = "prism-admin-token"

export function getToken(): string | null {
  return localStorage.getItem(TOKEN_KEY)
}

export function setToken(token: string) {
  localStorage.setItem(TOKEN_KEY, token)
}

export function clearToken() {
  localStorage.removeItem(TOKEN_KEY)
}

export async function api(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const token = getToken()
  const headers = new Headers(init?.headers)
  if (token && !headers.has("Authorization")) {
    headers.set("Authorization", `Bearer ${token}`)
  }
  const res = await fetch(input, { ...init, headers })
  if (res.status === 401) {
    window.dispatchEvent(new CustomEvent("prism:auth-required"))
  }
  return res
}
