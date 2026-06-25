import { Link, Outlet } from '@tanstack/react-router';
import {
  AlertTriangle,
  Boxes,
  LayoutDashboard,
  LogOut,
  Monitor,
  Moon,
  Sun,
  Users,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Select } from '@/components/ui/select';
import { CAP, useAuth } from '@/lib/auth';
import { useTheme } from '@/lib/theme';
import { cn } from '@/lib/utils';

const NAV = [
  { to: '/', label: 'Dashboard', icon: LayoutDashboard, exact: true, cap: CAP.read },
  { to: '/environments', label: 'Environments', icon: Boxes, exact: false, cap: CAP.read },
  { to: '/dlq', label: 'Dead-letter queue', icon: AlertTriangle, exact: true, cap: CAP.read },
  { to: '/users', label: 'Users', icon: Users, exact: false, cap: CAP.userManage },
] as const;

function ThemeToggle() {
  const { theme, setTheme } = useTheme();
  const icon =
    theme === 'dark' ? <Moon className="size-4" /> : theme === 'light' ? <Sun className="size-4" /> : <Monitor className="size-4" />;
  return (
    <div className="flex items-center gap-2">
      <span className="text-muted-foreground">{icon}</span>
      <Select
        aria-label="Theme"
        value={theme}
        onChange={(e) => setTheme(e.target.value as 'light' | 'dark' | 'system')}
        className="h-8 w-28"
      >
        <option value="system">System</option>
        <option value="light">Light</option>
        <option value="dark">Dark</option>
      </Select>
    </div>
  );
}

export function Layout() {
  const { user, has, logout } = useAuth();
  const items = NAV.filter((n) => has(n.cap));

  return (
    <div className="grid min-h-screen grid-cols-[15rem_1fr] max-md:grid-cols-1">
      <aside className="flex flex-col border-r border-border bg-card max-md:hidden">
        <div className="flex h-14 items-center gap-2 px-5">
          <span className="inline-block size-2.5 rounded-full bg-primary" />
          <span className="text-lg font-semibold tracking-tight">Chimely</span>
          <span className="text-xs text-muted-foreground">admin</span>
        </div>
        <nav className="flex flex-1 flex-col gap-1 p-3">
          {items.map(({ to, label, icon: Icon, exact }) => (
            <Link
              key={to}
              to={to}
              activeOptions={{ exact }}
              className="flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              activeProps={{
                className: cn(
                  'flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors',
                  'bg-primary/10 text-primary',
                ),
              }}
            >
              <Icon className="size-4" />
              {label}
            </Link>
          ))}
        </nav>
        <div className="flex flex-col gap-3 border-t border-border p-3">
          <div className="px-1">
            <p className="truncate text-sm font-medium" title={user.name}>
              {user.name}
            </p>
            <p className="truncate text-xs text-muted-foreground" title={user.email}>
              {user.email}
            </p>
            <span className="mt-1.5 inline-block rounded bg-primary/10 px-1.5 py-0.5 text-xs font-medium capitalize text-primary">
              {user.role}
            </span>
          </div>
          <Button variant="outline" size="sm" onClick={() => void logout()}>
            <LogOut className="size-4" /> Sign out
          </Button>
          <ThemeToggle />
        </div>
      </aside>
      <main className="min-w-0 overflow-x-hidden">
        <header className="flex h-14 items-center justify-between gap-4 border-b border-border px-6 md:hidden">
          <span className="font-semibold">Chimely admin</span>
          <div className="flex items-center gap-2">
            <ThemeToggle />
            <Button variant="outline" size="icon" aria-label="Sign out" onClick={() => void logout()}>
              <LogOut className="size-4" />
            </Button>
          </div>
        </header>
        <div className="mx-auto max-w-6xl p-6">
          <Outlet />
        </div>
      </main>
    </div>
  );
}
