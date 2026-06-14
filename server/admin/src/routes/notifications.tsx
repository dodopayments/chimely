import { useQuery } from '@tanstack/react-query';
import { useParams } from '@tanstack/react-router';
import { useState } from 'react';
import { EnvNav } from '@/components/env-nav';
import { ReadBadge, StatusBadge } from '@/components/status-badge';
import { Async } from '@/components/states';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api, type AdminNotification, type NotificationFilter } from '@/lib/api';
import { formatTs } from '@/lib/utils';

export function NotificationsRoute() {
  const { envId } = useParams({ strict: false }) as { envId: string };
  const [draft, setDraft] = useState<NotificationFilter>({ limit: 50 });
  const [applied, setApplied] = useState<NotificationFilter>({ limit: 50 });
  const [open, setOpen] = useState<AdminNotification | null>(null);

  const page = useQuery({
    queryKey: ['notifications', envId, applied],
    queryFn: () => api.listNotifications(envId, applied),
  });

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Notifications</h1>
        <p className="text-sm text-muted-foreground">
          Browse direct notifications and inspect the “did it send?” timeline.
        </p>
      </div>
      <EnvNav envId={envId} />

      <form
        className="grid items-end gap-3 sm:grid-cols-2 lg:grid-cols-4"
        onSubmit={(e) => {
          e.preventDefault();
          setApplied(draft);
        }}
      >
        <Field label="Subscriber id">
          <Input
            value={draft.subscriber_id ?? ''}
            onChange={(e) => setDraft((d) => ({ ...d, subscriber_id: e.target.value || undefined }))}
            placeholder="usr_42"
          />
        </Field>
        <Field label="Category">
          <Input
            value={draft.category ?? ''}
            onChange={(e) => setDraft((d) => ({ ...d, category: e.target.value || undefined }))}
            placeholder="payment.failed"
          />
        </Field>
        <Field label="After">
          <Input
            type="datetime-local"
            value={draft.after ? toLocalInput(draft.after) : ''}
            onChange={(e) => setDraft((d) => ({ ...d, after: fromLocalInput(e.target.value) }))}
          />
        </Field>
        <Field label="Before">
          <Input
            type="datetime-local"
            value={draft.before ? toLocalInput(draft.before) : ''}
            onChange={(e) => setDraft((d) => ({ ...d, before: fromLocalInput(e.target.value) }))}
          />
        </Field>
        <div className="flex gap-2">
          <Button type="submit">Apply filters</Button>
          <Button
            type="button"
            variant="outline"
            onClick={() => {
              setDraft({ limit: 50 });
              setApplied({ limit: 50 });
            }}
          >
            Reset
          </Button>
        </div>
      </form>

      <Async query={page} emptyTitle="No notifications">
        {(data) =>
          data.items.length === 0 ? (
            <p className="text-sm text-muted-foreground">No notifications match these filters.</p>
          ) : (
            <>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Subscriber</TableHead>
                    <TableHead>Category</TableHead>
                    <TableHead>Visible at</TableHead>
                    <TableHead>Read</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {data.items.map((n) => (
                    <TableRow key={n.id} className="cursor-pointer" onClick={() => setOpen(n)}>
                      <TableCell className="font-mono">{n.subscriber_id}</TableCell>
                      <TableCell>{n.category}</TableCell>
                      <TableCell className="text-muted-foreground">{formatTs(n.visible_at)}</TableCell>
                      <TableCell>
                        <ReadBadge read={n.read_at != null} />
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
              {data.next_cursor && (
                <div>
                  <Button
                    variant="outline"
                    onClick={() => setApplied((a) => ({ ...a, cursor: data.next_cursor ?? undefined }))}
                  >
                    Load more
                  </Button>
                </div>
              )}
            </>
          )
        }
      </Async>

      {open && (
        <TimelineDialog envId={envId} notif={open} onClose={() => setOpen(null)} />
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label>{label}</Label>
      {children}
    </div>
  );
}

function TimelineDialog({
  envId,
  notif,
  onClose,
}: {
  envId: string;
  notif: AdminNotification;
  onClose: () => void;
}) {
  const timeline = useQuery({
    queryKey: ['timeline', envId, notif.id],
    queryFn: () => api.notificationTimeline(envId, notif.id),
  });

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Status timeline</DialogTitle>
          <DialogDescription className="font-mono break-all">{notif.id}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <div className="grid grid-cols-2 gap-2 text-sm">
            <Meta label="Subscriber" value={notif.subscriber_id} />
            <Meta label="Category" value={notif.category} />
            <Meta label="Created" value={formatTs(notif.created_at)} />
            <Meta label="Visible" value={formatTs(notif.visible_at)} />
          </div>
          <Async query={timeline} emptyTitle="No timeline">
            {(t) => (
              <ol className="flex flex-col gap-3 border-l border-border pl-4">
                {t.timeline.map((entry) => (
                  <li key={entry.status} className="relative">
                    <span className="absolute -left-[1.32rem] top-1 size-2.5 rounded-full bg-primary" />
                    <div className="flex items-center gap-2">
                      <StatusBadge status={entry.status} />
                      <span className="text-sm text-muted-foreground">
                        {formatTs(entry.occurred_at)}
                      </span>
                    </div>
                  </li>
                ))}
              </ol>
            )}
          </Async>
          {Object.keys(notif.payload).length > 0 && (
            <pre className="max-h-48 overflow-auto rounded-md bg-muted p-3 text-xs">
              {JSON.stringify(notif.payload, null, 2)}
            </pre>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function Meta({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <p className="text-xs text-muted-foreground">{label}</p>
      <p className="font-mono">{value}</p>
    </div>
  );
}

function toLocalInput(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function fromLocalInput(value: string): string | undefined {
  if (!value) return undefined;
  const d = new Date(value);
  return Number.isNaN(d.getTime()) ? undefined : d.toISOString();
}
