import { useEffect, useState } from "react"
import { BrowserRouter, Routes, Route, NavLink, useLocation } from "react-router-dom"
import { LayoutDashboard, Box, Brain, GitBranch, Gauge, Layers, Puzzle, Shield, ScrollText, FileCode, Menu, Sun, Moon, Monitor, Users } from "lucide-react"
import { Button } from "@/components/ui/button"
import { useTheme } from "@/hooks/useTheme"
import { Sheet, SheetTrigger, SheetContent } from "@/components/ui/sheet"
import Dashboard from "./pages/Dashboard"
import Providers from "./pages/Providers"
import Models from "./pages/Models"
import Variants from "./pages/Variants"
import RateLimits from "./pages/RateLimits"
import Pools from "./pages/Pools"
import Plugins from "./pages/Plugins"
import Auth from "./pages/Auth"
import AccessLog from "./pages/AccessLog"
import ConfigEditor from "./pages/ConfigEditor"
import Accounts from "./pages/Accounts"

const resourceNav = [
  { to: "/", label: "Dashboard", icon: LayoutDashboard },
  { to: "/providers", label: "Providers", icon: Box },
  { to: "/accounts", label: "Accounts", icon: Users },
  { to: "/models", label: "Models", icon: Brain },
  { to: "/variants", label: "Variants", icon: GitBranch },
  { to: "/rate-limits", label: "Rate Limits", icon: Gauge },
  { to: "/pools", label: "Pools", icon: Layers },
  { to: "/plugins", label: "Plugins", icon: Puzzle },
]

const settingsNav = [
  { to: "/auth", label: "Auth", icon: Shield },
  { to: "/access-log", label: "Access Log", icon: ScrollText },
  { to: "/config", label: "Config", icon: FileCode },
]

function NavItems({ onClick }: { onClick?: () => void }) {
  return (
    <>
      <div className="px-3 py-2 text-xs font-semibold text-[hsl(var(--muted-foreground))] uppercase tracking-wider">Resources</div>
      {resourceNav.map(item => (
        <NavLink
          key={item.to}
          to={item.to}
          end={item.to === "/"}
          onClick={onClick}
          className={({ isActive }) =>
            `flex items-center gap-2 px-3 py-2 rounded-md text-sm transition-colors ${
              isActive
                ? "bg-[hsl(var(--accent))] text-[hsl(var(--accent-foreground))]"
                : "text-[hsl(var(--muted-foreground))] hover:bg-[hsl(var(--accent))] hover:text-[hsl(var(--accent-foreground))]"
            }`
          }
        >
          <item.icon className="h-4 w-4" />
          {item.label}
        </NavLink>
      ))}
      <div className="px-3 pt-4 pb-2 text-xs font-semibold text-[hsl(var(--muted-foreground))] uppercase tracking-wider">Settings</div>
      {settingsNav.map(item => (
        <NavLink
          key={item.to}
          to={item.to}
          onClick={onClick}
          className={({ isActive }) =>
            `flex items-center gap-2 px-3 py-2 rounded-md text-sm transition-colors ${
              isActive
                ? "bg-[hsl(var(--accent))] text-[hsl(var(--accent-foreground))]"
                : "text-[hsl(var(--muted-foreground))] hover:bg-[hsl(var(--accent))] hover:text-[hsl(var(--accent-foreground))]"
            }`
          }
        >
          <item.icon className="h-4 w-4" />
          {item.label}
        </NavLink>
      ))}
    </>
  )
}

function AppLayout() {
  const [open, setOpen] = useState(false)
  const { theme, cycleTheme } = useTheme()
  const location = useLocation()

  const ThemeIcon = theme === "dark" ? Moon : theme === "light" ? Sun : Monitor
  const themeLabel = theme === "dark" ? "Dark" : theme === "light" ? "Light" : "System"

  useEffect(() => {
    setOpen(false)
  }, [location.pathname])

  return (
    <div className="flex h-screen">
      {/* Desktop sidebar */}
      <nav className="hidden md:flex w-56 border-r border-[hsl(var(--border))] bg-[hsl(var(--card))] flex-col">
        <div className="p-4 border-b border-[hsl(var(--border))]">
          <h1 className="text-lg font-bold bg-gradient-to-r from-blue-400 to-purple-400 bg-clip-text text-transparent">
            Prism Admin
          </h1>
          <p className="text-xs text-[hsl(var(--muted-foreground))]">LLM Router Management</p>
        </div>
        <div className="flex-1 p-2 space-y-1 overflow-y-auto">
          <NavItems />
        </div>
        <div className="p-3 border-t border-[hsl(var(--border))] flex items-center justify-between">
          <span className="text-xs text-[hsl(var(--muted-foreground))]">Prism Router v0.1.0</span>
          <Button variant="ghost" size="icon" className="h-7 w-7" onClick={cycleTheme} title={themeLabel}>
            <ThemeIcon className="h-3.5 w-3.5 text-[hsl(var(--muted-foreground))]" />
          </Button>
        </div>
      </nav>

      {/* Mobile header + sheet */}
      <div className="flex-1 flex flex-col min-w-0">
        <header className="md:hidden flex items-center gap-3 p-3 border-b border-[hsl(var(--border))] bg-[hsl(var(--card))]">
          <Sheet open={open} onOpenChange={setOpen}>
            <SheetTrigger asChild>
              <Button variant="ghost" size="icon">
                <Menu className="h-5 w-5" />
              </Button>
            </SheetTrigger>
            <SheetContent side="left">
              <div className="mb-4">
                <h1 className="text-lg font-bold bg-gradient-to-r from-blue-400 to-purple-400 bg-clip-text text-transparent">
                  Prism Admin
                </h1>
                <p className="text-xs text-[hsl(var(--muted-foreground))]">LLM Router Management</p>
              </div>
              <div className="space-y-1">
                <NavItems onClick={() => setOpen(false)} />
              </div>
            </SheetContent>
          </Sheet>
          <h1 className="text-sm font-semibold bg-gradient-to-r from-blue-400 to-purple-400 bg-clip-text text-transparent flex-1">Prism Admin</h1>
          <Button variant="ghost" size="icon" className="h-8 w-8" onClick={cycleTheme} title={themeLabel}>
            <ThemeIcon className="h-4 w-4 text-[hsl(var(--muted-foreground))]" />
          </Button>
        </header>
        <main className="flex-1 overflow-auto p-4 sm:p-6 lg:p-8">
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/providers" element={<Providers />} />
            <Route path="/accounts" element={<Accounts />} />
            <Route path="/models" element={<Models />} />
            <Route path="/variants" element={<Variants />} />
            <Route path="/rate-limits" element={<RateLimits />} />
            <Route path="/pools" element={<Pools />} />
            <Route path="/plugins" element={<Plugins />} />
            <Route path="/auth" element={<Auth />} />
            <Route path="/access-log" element={<AccessLog />} />
            <Route path="/config" element={<ConfigEditor />} />
          </Routes>
        </main>
      </div>
    </div>
  )
}

export default function App() {
  return (
    <BrowserRouter>
      <AppLayout />
    </BrowserRouter>
  )
}
