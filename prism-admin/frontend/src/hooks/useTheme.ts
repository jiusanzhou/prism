import { useEffect, useState } from "react"

type Theme = "light" | "dark" | "system"

function getSystemTheme(): "light" | "dark" {
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"
}

function applyTheme(theme: Theme) {
  const resolved = theme === "system" ? getSystemTheme() : theme
  document.documentElement.classList.toggle("dark", resolved === "dark")
}

export function useTheme() {
  const [theme, setThemeState] = useState<Theme>(() => {
    return (localStorage.getItem("prism-theme") as Theme) || "dark"
  })

  useEffect(() => {
    applyTheme(theme)
    localStorage.setItem("prism-theme", theme)
  }, [theme])

  useEffect(() => {
    if (theme !== "system") return
    const mq = window.matchMedia("(prefers-color-scheme: dark)")
    const handler = () => applyTheme("system")
    mq.addEventListener("change", handler)
    return () => mq.removeEventListener("change", handler)
  }, [theme])

  const setTheme = (t: Theme) => setThemeState(t)

  const cycleTheme = () => {
    const order: Theme[] = ["light", "dark", "system"]
    const next = order[(order.indexOf(theme) + 1) % order.length]
    setThemeState(next)
  }

  return { theme, setTheme, cycleTheme }
}
