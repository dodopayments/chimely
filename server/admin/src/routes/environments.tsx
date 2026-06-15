import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { Plus } from 'lucide-react';
import { useState } from 'react';
import { toast } from 'sonner';
import { Async } from '@/components/states';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api, ApiRequestError } from '@/lib/api';
import { CAP, useAuth } from '@/lib/auth';
import { formatTs } from '@/lib/utils';

export function EnvironmentsRoute() {
  const { has } = useAuth();
  const envs = useQuery({ queryKey: ['environments'], queryFn: api.listEnvironments });
  const navigate = useNavigate();

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Environments</h1>
          <p className="text-sm text-muted-foreground">
            The isolation unit. Each has its own API keys and subscriber HMAC secret.
          </p>
        </div>
        {has(CAP.envCreate) && <NewEnvironmentDialog />}
      </div>

      <Async query={envs} emptyTitle="No environments yet">
        {(rows) =>
          rows.length === 0 ? (
            <p className="text-sm text-muted-foreground">Create your first environment to begin.</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Slug</TableHead>
                  <TableHead>Name</TableHead>
                  <TableHead>Subscriber hash</TableHead>
                  <TableHead>Created</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((env) => (
                  <TableRow
                    key={env.id}
                    className="cursor-pointer"
                    onClick={() => navigate({ to: '/environments/$envId', params: { envId: env.id } })}
                  >
                    <TableCell className="font-mono">{env.slug}</TableCell>
                    <TableCell>{env.name}</TableCell>
                    <TableCell>
                      {env.require_subscriber_hash ? (
                        <Badge variant="default">required</Badge>
                      ) : (
                        <Badge variant="neutral">optional (dev)</Badge>
                      )}
                    </TableCell>
                    <TableCell className="text-muted-foreground">{formatTs(env.created_at)}</TableCell>
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

function NewEnvironmentDialog() {
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [slug, setSlug] = useState('');
  const [name, setName] = useState('');
  const [requireHash, setRequireHash] = useState(true);

  const create = useMutation({
    mutationFn: () => api.createEnvironment({ slug, name, require_subscriber_hash: requireHash }),
    onSuccess: (env) => {
      toast.success(`Environment ${env.slug} created`);
      qc.invalidateQueries({ queryKey: ['environments'] });
      setOpen(false);
      setSlug('');
      setName('');
      setRequireHash(true);
    },
    onError: (e) => toast.error(e instanceof ApiRequestError ? e.message : 'Failed to create'),
  });

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button>
          <Plus /> New environment
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New environment</DialogTitle>
          <DialogDescription>
            Slugs are URL-safe handles the widget sends to scope the subscriber plane.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            create.mutate();
          }}
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="slug">Slug</Label>
            <Input
              id="slug"
              value={slug}
              onChange={(e) => setSlug(e.target.value)}
              placeholder="dashboard-prod"
              required
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="name">Name</Label>
            <Input
              id="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Dashboard (production)"
              required
            />
          </div>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={requireHash}
              onChange={(e) => setRequireHash(e.target.checked)}
              className="accent-primary"
            />
            Require subscriber HMAC hash (recommended for production)
          </label>
          <DialogFooter>
            <Button type="submit" disabled={create.isPending}>
              {create.isPending ? 'Creating…' : 'Create environment'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
