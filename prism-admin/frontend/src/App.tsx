import { BrowserRouter, Routes, Route, NavLink } from "react-router-dom"
import { LayoutDashboard, Box, Brain, GitBranch, Gauge } from "lucide-react"
import Dashboard from "./pages/Dashboard"
import Providers from "./pages/Providers"
import Models from "./pages/Models"
import Variants from "./pages/Variants"
import RateLimits from "./pages/RateLimits"

const navItems = [
  { to: "/", label: "Dashboard", icon: LayoutDashboard },
  { to: "/providers", label: "Providers", icon: Box },
  { to: "/models", label: "Models", icon: Brain },
  { to: "/variants", label: "Variants", icon: GitBranch },
  { to: "/rate-limits", label: "Rate Limits", icon: Gauge },
]

export default function App() {
  return (
    <BrowserRouter>
      <div className="flex h-screen">
        {/* Sidebar */}
        <nav className="w-56 border-r border-[hsl(var(--border))] bg-[hsl(var(--card))] flex flex-col">
          <div className="p-4 border-b border-[hsl(var(--border))]">
            <h1 className="text-lg font-bold bg-gradient-to-r from-blue-400 to-purple-400 bg-clip-text text-transparent">
              Prism Admin
            </h1>
            <p className="text-xs text-[hsl(var(--muted-foreground))]">LLM Router Management</p>
          </div>
          <div className="flex-1 p-2 space-y-1">
            {navItems.map(item => (
              <NavLink
                key={item.to}
                to={item.to}
                end={item.to === "/"}
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
          </div>
          <div className="p-3 border-t border-[hsl(var(--border))] text-xs text-[hsl(var(--muted-foreground))]">
            Prism Router v0.1.0
          </div>
        </nav>

        {/* Main */}
        <main className="flex-1 overflow-auto p-8">
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/providers" element={<Providers />} />
            <Route path="/models" element={<Models />} />
            <Route path="/variants" element={<Variants />} />
            <Route path="/rate-limits" element={<RateLimits />} />
          </Routes>
        </main>
      </div>
    </BrowserRouter>
  )
}
