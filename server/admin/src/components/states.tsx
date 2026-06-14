import type { ReactNode } from 'react';
import { Skeleton } from '@/components/ui/skeleton';

export function EmptyState({ title, hint }: { title: string; hint?: string }) {
  return (
    <div className="flex flex-col items-center justify-center gap-1 rounded-lg border border-dashed border-border py-12 text-center">
      <p className="font-medium">{title}</p>
      {hint && <p className="text-sm text-muted-foreground">{hint}</p>}
    </div>
  );
}

export function ErrorState({ message }: { message: string }) {
  return (
    <div className="rounded-lg border border-danger/40 bg-danger/10 p-4 text-sm text-danger">
      {message}
    </div>
  );
}

export function TableSkeleton({ rows = 5, cols = 4 }: { rows?: number; cols?: number }) {
  return (
    <div className="flex flex-col gap-2">
      {Array.from({ length: rows }).map((_, r) => (
        <div key={r} className="flex gap-3">
          {Array.from({ length: cols }).map((_, c) => (
            <Skeleton key={c} className="h-8 flex-1" />
          ))}
        </div>
      ))}
    </div>
  );
}

export function Async<T>({
  query,
  children,
  emptyTitle,
}: {
  query: { isLoading: boolean; isError: boolean; error: unknown; data: T | undefined };
  children: (data: T) => ReactNode;
  emptyTitle?: string;
}) {
  if (query.isLoading) return <TableSkeleton />;
  if (query.isError) {
    const message = query.error instanceof Error ? query.error.message : 'Request failed';
    return <ErrorState message={message} />;
  }
  if (query.data === undefined) return <EmptyState title={emptyTitle ?? 'No data'} />;
  return <>{children(query.data)}</>;
}
