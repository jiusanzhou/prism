import { useEffect } from "react"

export function usePageTitle(title: string) {
  useEffect(() => {
    document.title = `${title} - Prism Admin`
    return () => { document.title = "Prism Admin" }
  }, [title])
}
