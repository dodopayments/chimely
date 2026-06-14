import { useQuery } from '@tanstack/react-query';
import { useParams } from '@tanstack/react-router';
import { Search } from 'lucide-react';
import { useState } from 'react';
import { EnvNav } from '@/components/env-nav';
import { ReadBadge } from '@/components/status-badge';
import { ErrorState } from '@/components/states';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Skeleton } from '@/components/ui/skeleton';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { ApiRequestError, api } from '@/lib/api';
import { formatTs } from '@/lib/utils';

export function SubscribersRoute() {
  const { envId } = useParams({ strict: false }) as { envId: string };
  const [input, setInput] = useState('');
  const [query, setQuery] = useState('');

  const view = useQuery({
    queryKey: ['subscriber', envId, query],
    queryFn: () => api.getSubscriber(envId, query),
    enabled: query.length > 0,
    retry: false,
  });

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Subscriber lookup</h1>
        <p className="text-sm text-muted-foreground">
          Counters, watermarks, preferences, and the merged inbox exactly as the subscriber sees it.
        </p>
      </div>
      <EnvNav envId={envId} />

      <form
        className="flex max-w-md gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          setQuery(input.trim());
        }}
      >
        <Input value={input} onChange={(e) => setInput(e.target.value)} placeholder="Customer subscriber id (usr_42)" />
        <Button type="submit">
          <Search /> Look up
        </Button>
      </form>

      {query && view.isLoading && <Skeleton className="h-40 w-full" />}
      {query && view.isError && (
        <ErrorState
          message={
            view.error instanceof ApiRequestError && view.error.status === 404
              ? `No subscriber “${query}” in this environment.`
              : 'Lookup failed.'
          }
        />
      )}
      {view.data && (
        <div className="flex flex-col gap-6">
          <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <Stat label="Unread" value={view.data.counters.unread} accent />
            <Stat label="Unseen" value={view.data.counters.unseen} />
            <Stat label="Read watermark" text={formatTs(view.data.read_watermark)} />
            <Stat label="Seen watermark" text={formatTs(view.data.seen_watermark)} />
          </div>

          <Card>
            <CardHeader>
              <CardTitle>Identity</CardTitle>
              <CardDescription>
                Broadcast visibility window: sees broadcasts created at or after{' '}
                <span className="font-mono">{formatTs(view.data.created_at)}</span>.
              </CardDescription>
            </CardHeader>
          </Card>

          <section className="flex flex-col gap-2">
            <h2 className="text-sm font-semibold">Preferences</h2>
            {view.data.preferences.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No explicit preferences — every category is enabled by default.
              </p>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Category</TableHead>
                    <TableHead>Channel</TableHead>
                    <TableHead>State</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {view.data.preferences.map((p) => (
                    <TableRow key={`${p.category}:${p.channel}`}>
                      <TableCell>{p.category}</TableCell>
                      <TableCell className="font-mono">{p.channel}</TableCell>
                      <TableCell>
                        {p.enabled ? (
                          <Badge variant="success">enabled</Badge>
                        ) : (
                          <Badge variant="neutral">muted</Badge>
                        )}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </section>

          <section className="flex flex-col gap-2">
            <h2 className="text-sm font-semibold">Recent inbox (merged)</h2>
            {view.data.inbox.length === 0 ? (
              <p className="text-sm text-muted-foreground">Empty inbox.</p>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Source</TableHead>
                    <TableHead>Category</TableHead>
                    <TableHead>Occurred</TableHead>
                    <TableHead>Read</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {view.data.inbox.map((item) => (
                    <TableRow key={item.id}>
                      <TableCell>
                        <Badge variant={item.source === 'broadcast' ? 'default' : 'neutral'}>
                          {item.source}
                        </Badge>
                      </TableCell>
                      <TableCell>{item.category}</TableCell>
                      <TableCell className="text-muted-foreground">{formatTs(item.occurred_at)}</TableCell>
                      <TableCell>
                        <ReadBadge read={item.read} />
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </section>
        </div>
      )}
    </div>
  );
}

function Stat({
  label,
  value,
  text,
  accent,
}: {
  label: string;
  value?: number;
  text?: string;
  accent?: boolean;
}) {
  return (
    <Card>
      <CardContent className="p-5">
        <p className="text-sm text-muted-foreground">{label}</p>
        {text ? (
          <p className="mt-1 font-mono text-sm">{text}</p>
        ) : (
          <p className={`text-3xl font-semibold tabular-nums ${accent ? 'text-primary' : ''}`}>{value}</p>
        )}
      </CardContent>
    </Card>
  );
}
