import { Badge } from '@/components/ui/badge';

// Maps notification/job states to the semantic palette. State is the ONLY
// thing color encodes here.
const STATUS_VARIANT: Record<
  string,
  'neutral' | 'default' | 'success' | 'warning' | 'warningHi' | 'danger'
> = {
  created: 'neutral',
  delivered_hint: 'default',
  seen: 'default',
  read: 'success',
  retrying: 'warning',
  near_dlq: 'warningHi',
  failed: 'danger',
  parked: 'danger',
};

const STATUS_LABEL: Record<string, string> = {
  created: 'created',
  delivered_hint: 'delivered',
  seen: 'seen',
  read: 'read',
};

export function StatusBadge({ status }: { status: string }) {
  const variant = STATUS_VARIANT[status] ?? 'neutral';
  return <Badge variant={variant}>{STATUS_LABEL[status] ?? status}</Badge>;
}

export function ReadBadge({ read }: { read: boolean }) {
  return read ? (
    <Badge variant="success">read</Badge>
  ) : (
    <Badge variant="default">unread</Badge>
  );
}
