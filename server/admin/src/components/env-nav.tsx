import { Link } from '@tanstack/react-router';
import { CAP, useAuth } from '@/lib/auth';
import { cn } from '@/lib/utils';

const TABS = [
  { to: '/environments/$envId', label: 'Overview', exact: true, cap: CAP.read },
  { to: '/environments/$envId/notifications', label: 'Notifications', exact: false, cap: CAP.read },
  { to: '/environments/$envId/broadcasts', label: 'Broadcast', exact: false, cap: CAP.broadcastCompose },
  { to: '/environments/$envId/subscribers', label: 'Subscriber lookup', exact: false, cap: CAP.read },
] as const;

export function EnvNav({ envId }: { envId: string }) {
  const { has } = useAuth();
  return (
    <nav className="flex flex-wrap gap-1 border-b border-border pb-2">
      {TABS.filter((t) => has(t.cap)).map((t) => (
        <Link
          key={t.to}
          to={t.to}
          params={{ envId }}
          activeOptions={{ exact: t.exact }}
          className="rounded-md px-3 py-1.5 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          activeProps={{
            className: cn('rounded-md px-3 py-1.5 text-sm font-medium', 'bg-primary/10 text-primary'),
          }}
        >
          {t.label}
        </Link>
      ))}
    </nav>
  );
}
