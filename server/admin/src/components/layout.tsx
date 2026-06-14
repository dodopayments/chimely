import { Link, Outlet } from '@tanstack/react-router';
import { AlertTriangle, Boxes, LayoutDashboard, Monitor, Moon, Sun } from 'lucide-react';
import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Select } from '@/components/ui/select';
import { useTheme } from '@/lib/theme';
import { cn } from '@/lib/utils';

const NAV = [
  { to: '/', label: 'Dashboard', icon: LayoutDashboard, exact: true },
  { to: '/environments', label: 'Environments', icon: Boxes, exact: false },
  { to: '/dlq', label: 'Dead-letter queue', icon: AlertTriangle, exact: true },
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

function Unauthorized() {
  return (
    <div className="flex min-h-screen flex-col items-center justify-center gap-4 p-6 text-center">
      <h1 className="text-xl font-semibold">Session expired</h1>
      <p className="max-w-sm text-muted-foreground">
        Your admin credential is no longer valid. Reload the page to re-authenticate.
      </p>
      <Button onClick={() => window.location.reload()}>Reload</Button>
    </div>
  );
}

export function Layout() {
  const [unauthorized, setUnauthorized] = useState(false);

  useEffect(() => {
    const handler = () => setUnauthorized(true);
    window.addEventListener('dronte-admin-unauthorized', handler);
    return () => window.removeEventListener('dronte-admin-unauthorized', handler);
  }, []);

  if (unauthorized) return <Unauthorized />;

  return (
    <div className="grid min-h-screen grid-cols-[15rem_1fr] max-md:grid-cols-1">
      <aside className="flex flex-col border-r border-border bg-card max-md:hidden">
        <div className="flex h-14 items-center gap-2 px-5">
          <span className="inline-block size-2.5 rounded-full bg-primary" />
          <span className="text-lg font-semibold tracking-tight">Dronte</span>
          <span className="text-xs text-muted-foreground">admin</span>
        </div>
        <nav className="flex flex-1 flex-col gap-1 p-3">
          {NAV.map(({ to, label, icon: Icon, exact }) => (
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
        <div className="border-t border-border p-3">
          <ThemeToggle />
        </div>
      </aside>
      <main className="min-w-0 overflow-x-hidden">
        <header className="flex h-14 items-center justify-between gap-4 border-b border-border px-6 md:hidden">
          <span className="font-semibold">Dronte admin</span>
          <ThemeToggle />
        </header>
        <div className="mx-auto max-w-6xl p-6">
          <Outlet />
        </div>
      </main>
    </div>
  );
}
