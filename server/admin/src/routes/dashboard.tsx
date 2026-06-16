import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { AlertTriangle, Boxes } from 'lucide-react';
import { Bar, BarChart, CartesianGrid, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';
import { Async } from '@/components/states';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { api, type AdminDeadLetter } from '@/lib/api';

export function DashboardRoute() {
  const envs = useQuery({ queryKey: ['environments'], queryFn: api.listEnvironments });
  const dlq = useQuery({ queryKey: ['dlq'], queryFn: api.listDlq });

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Overview</h1>
        <p className="text-sm text-muted-foreground">Operational status across the instance.</p>
      </div>

      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <StatCard
          icon={<Boxes className="size-4 text-primary" />}
          label="Environments"
          value={envs.data?.length}
          to="/environments"
        />
        <ParkedCard data={dlq.data} />
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Parked jobs by environment</CardTitle>
          <CardDescription>Dead-letter backlog awaiting replay.</CardDescription>
        </CardHeader>
        <CardContent>
          <Async query={dlq} emptyTitle="No parked jobs">
            {(letters) => <DlqChart letters={letters} />}
          </Async>
        </CardContent>
      </Card>
    </div>
  );
}

function StatCard({
  icon,
  label,
  value,
  to,
}: {
  icon: React.ReactNode;
  label: string;
  value: number | undefined;
  to: string;
}) {
  return (
    <Link to={to} className="block">
      <Card className="transition-colors hover:border-primary/50">
        <CardContent className="flex items-center justify-between p-6">
          <div>
            <p className="text-sm text-muted-foreground">{label}</p>
            <p className="text-3xl font-semibold tabular-nums">{value ?? '—'}</p>
          </div>
          {icon}
        </CardContent>
      </Card>
    </Link>
  );
}

function ParkedCard({ data }: { data: AdminDeadLetter[] | undefined }) {
  const count = data?.length ?? 0;
  const danger = count > 0;
  return (
    <Link to="/dlq" className="block">
      <Card className="transition-colors hover:border-primary/50">
        <CardContent className="flex items-center justify-between p-6">
          <div>
            <p className="text-sm text-muted-foreground">Parked jobs</p>
            <p className={`text-3xl font-semibold tabular-nums ${danger ? 'text-danger' : ''}`}>
              {data ? count : '—'}
            </p>
          </div>
          <AlertTriangle className={`size-4 ${danger ? 'text-danger' : 'text-muted-foreground'}`} />
        </CardContent>
      </Card>
    </Link>
  );
}

function DlqChart({ letters }: { letters: AdminDeadLetter[] }) {
  if (letters.length === 0) {
    return <p className="py-8 text-center text-sm text-muted-foreground">No parked jobs — the queue is clear.</p>;
  }
  const byEnv = new Map<string, number>();
  for (const l of letters) byEnv.set(l.environment_slug, (byEnv.get(l.environment_slug) ?? 0) + 1);
  const data = [...byEnv.entries()].map(([slug, count]) => ({ slug, count }));

  return (
    <ResponsiveContainer width="100%" height={260}>
      <BarChart data={data}>
        <CartesianGrid strokeDasharray="3 3" stroke="var(--color-border)" vertical={false} />
        <XAxis dataKey="slug" tick={{ fill: 'var(--color-muted-foreground)', fontSize: 12 }} />
        <YAxis allowDecimals={false} tick={{ fill: 'var(--color-muted-foreground)', fontSize: 12 }} />
        <Tooltip
          contentStyle={{
            background: 'var(--color-popover)',
            border: '1px solid var(--color-border)',
            borderRadius: 8,
            color: 'var(--color-popover-foreground)',
          }}
        />
        {/* Parked jobs are a failure state → danger. */}
        <Bar dataKey="count" fill="var(--color-chart-5)" radius={[4, 4, 0, 0]} />
      </BarChart>
    </ResponsiveContainer>
  );
}
