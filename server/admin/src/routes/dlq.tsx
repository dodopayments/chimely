import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { toast } from 'sonner';
import { Async } from '@/components/states';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api, type AdminDeadLetter, ApiRequestError } from '@/lib/api';
import { CAP, useAuth } from '@/lib/auth';
import { formatTs } from '@/lib/utils';

export function DlqRoute() {
  const qc = useQueryClient();
  const { has } = useAuth();
  const canReplay = has(CAP.dlqReplay);
  const dlq = useQuery({ queryKey: ['dlq'], queryFn: api.listDlq });
  const [expanded, setExpanded] = useState<string | null>(null);

  const invalidate = () => {
    qc.invalidateQueries({ queryKey: ['dlq'] });
  };

  const replayOne = useMutation({
    mutationFn: (jobId: string) => api.replayDeadLetter(jobId),
    onSuccess: () => {
      toast.success('Replayed — re-entered the normal claim path');
      invalidate();
    },
    onError: (e) => toast.error(e instanceof ApiRequestError ? e.message : 'Replay failed'),
  });

  const replayAll = useMutation({
    mutationFn: () => api.replayAllDeadLetters(),
    onSuccess: (r) => {
      toast.success(`Replayed ${r.replayed} job${r.replayed === 1 ? '' : 's'}`);
      invalidate();
    },
    onError: (e) => toast.error(e instanceof ApiRequestError ? e.message : 'Replay failed'),
  });

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Dead-letter queue</h1>
          <p className="text-sm text-muted-foreground">
            Jobs that exhausted their retries, across all environments. Replay re-enqueues through
            the normal worker loop.
          </p>
        </div>
        {canReplay && (
          <Button
            variant="destructive"
            disabled={replayAll.isPending || (dlq.data?.length ?? 0) === 0}
            onClick={() => replayAll.mutate()}
          >
            Replay all
          </Button>
        )}
      </div>

      <Async query={dlq} emptyTitle="Dead-letter queue is empty">
        {(letters: AdminDeadLetter[]) =>
          letters.length === 0 ? (
            <p className="text-sm text-muted-foreground">No parked jobs. The queue is clear.</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Job</TableHead>
                  <TableHead>Environment</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead>Attempts</TableHead>
                  <TableHead>Parked</TableHead>
                  <TableHead>Error</TableHead>
                  <TableHead />
                </TableRow>
              </TableHeader>
              <TableBody>
                {letters.map((l) => (
                  <TableRow key={l.id}>
                    <TableCell className="font-mono text-xs">{l.id}</TableCell>
                    <TableCell className="font-mono">{l.environment_slug}</TableCell>
                    <TableCell>{l.job_type}</TableCell>
                    <TableCell>
                      <Badge variant="warningHi">{l.attempts}</Badge>
                    </TableCell>
                    <TableCell className="text-muted-foreground">{formatTs(l.parked_at)}</TableCell>
                    <TableCell className="max-w-xs">
                      <button
                        type="button"
                        className="truncate text-left text-danger hover:underline"
                        title="Click to expand"
                        onClick={() => setExpanded(expanded === l.id ? null : l.id)}
                      >
                        {expanded === l.id ? l.last_error : truncate(l.last_error)}
                      </button>
                    </TableCell>
                    <TableCell className="text-right">
                      {canReplay && (
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={replayOne.isPending}
                          onClick={() => replayOne.mutate(l.id)}
                        >
                          Replay
                        </Button>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )
        }
      </Async>
    </div>
  );
}

function truncate(s: string, n = 60): string {
  const firstLine = s.split('\n')[0] ?? s;
  return firstLine.length > n ? `${firstLine.slice(0, n)}…` : firstLine;
}
