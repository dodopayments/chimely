import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useParams } from '@tanstack/react-router';
import { KeyRound, Plus, RefreshCw } from 'lucide-react';
import { useState } from 'react';
import { toast } from 'sonner';
import { CopyField } from '@/components/copy-field';
import { EnvNav } from '@/components/env-nav';
import { Async } from '@/components/states';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
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
import { Separator } from '@/components/ui/separator';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { api, type AdminApiKeyCreated, ApiRequestError } from '@/lib/api';
import { formatTs } from '@/lib/utils';

export function EnvironmentDetailRoute() {
  const { envId } = useParams({ strict: false }) as { envId: string };
  const env = useQuery({ queryKey: ['environment', envId], queryFn: () => api.getEnvironment(envId) });

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">
          {env.data?.name ?? 'Environment'}
        </h1>
        <p className="font-mono text-sm text-muted-foreground">{env.data?.slug ?? envId}</p>
      </div>
      <EnvNav envId={envId} />

      <Tabs defaultValue="keys">
        <TabsList>
          <TabsTrigger value="keys">API keys</TabsTrigger>
          <TabsTrigger value="hmac">Subscriber HMAC</TabsTrigger>
        </TabsList>
        <TabsContent value="keys">
          <ApiKeysTab envId={envId} />
        </TabsContent>
        <TabsContent value="hmac">
          <HmacTab envId={envId} />
        </TabsContent>
      </Tabs>
    </div>
  );
}

function ApiKeysTab({ envId }: { envId: string }) {
  const keys = useQuery({ queryKey: ['api-keys', envId], queryFn: () => api.listApiKeys(envId) });
  return (
    <div className="flex flex-col gap-4">
      <div className="flex justify-end">
        <CreateApiKeyDialog envId={envId} />
      </div>
      <Async query={keys} emptyTitle="No API keys">
        {(rows) =>
          rows.length === 0 ? (
            <p className="text-sm text-muted-foreground">No API keys yet.</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Prefix</TableHead>
                  <TableHead>Name</TableHead>
                  <TableHead>Created</TableHead>
                  <TableHead>Last used</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead />
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((k) => (
                  <TableRow key={k.id}>
                    <TableCell className="font-mono">{k.key_prefix}…</TableCell>
                    <TableCell>{k.name}</TableCell>
                    <TableCell className="text-muted-foreground">{formatTs(k.created_at)}</TableCell>
                    <TableCell className="text-muted-foreground">{formatTs(k.last_used_at)}</TableCell>
                    <TableCell>
                      {k.revoked_at ? (
                        <Badge variant="danger">revoked</Badge>
                      ) : (
                        <Badge variant="success">active</Badge>
                      )}
                    </TableCell>
                    <TableCell className="text-right">
                      {!k.revoked_at && <RevokeKeyButton envId={envId} keyId={k.id} name={k.name} />}
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

function CreateApiKeyDialog({ envId }: { envId: string }) {
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const [created, setCreated] = useState<AdminApiKeyCreated | null>(null);

  const create = useMutation({
    mutationFn: () => api.createApiKey(envId, name),
    onSuccess: (key) => {
      setCreated(key);
      setName('');
      toast.success('API key created — copy it now');
      qc.invalidateQueries({ queryKey: ['api-keys', envId] });
    },
    onError: (e) => toast.error(e instanceof ApiRequestError ? e.message : 'Failed'),
  });

  const close = (o: boolean) => {
    setOpen(o);
    if (!o) setCreated(null);
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogTrigger asChild>
        <Button>
          <Plus /> Create key
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Create API key</DialogTitle>
          <DialogDescription>
            The full key is shown once and never again. Only its hash is stored.
          </DialogDescription>
        </DialogHeader>
        {created ? (
          <div className="flex flex-col gap-3">
            <div className="rounded-md border border-warning-hi/40 bg-warning-hi/10 p-3 text-sm">
              Store this key now. You will not be able to see it again.
            </div>
            <CopyField value={created.key} />
            <DialogFooter>
              <Button onClick={() => close(false)}>Done</Button>
            </DialogFooter>
          </div>
        ) : (
          <form
            className="flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              create.mutate();
            }}
          >
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="key-name">Name</Label>
              <Input
                id="key-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="ci, backend, …"
                required
              />
            </div>
            <DialogFooter>
              <Button type="submit" disabled={create.isPending}>
                {create.isPending ? 'Creating…' : 'Create key'}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

function RevokeKeyButton({ envId, keyId, name }: { envId: string; keyId: string; name: string }) {
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const revoke = useMutation({
    mutationFn: () => api.revokeApiKey(envId, keyId),
    onSuccess: () => {
      toast.success('Key revoked');
      qc.invalidateQueries({ queryKey: ['api-keys', envId] });
      setOpen(false);
    },
    onError: (e) => toast.error(e instanceof ApiRequestError ? e.message : 'Failed'),
  });
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button variant="ghost" size="sm" className="text-danger hover:text-danger">
          Revoke
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>Revoke “{name}”?</DialogTitle>
          <DialogDescription>
            The key stops authenticating immediately. The row is kept for audit.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={() => revoke.mutate()} disabled={revoke.isPending}>
            Revoke key
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function HmacTab({ envId }: { envId: string }) {
  const qc = useQueryClient();
  const env = useQuery({ queryKey: ['environment', envId], queryFn: () => api.getEnvironment(envId) });
  const [rotatedSecret, setRotatedSecret] = useState<string | null>(null);

  const rotate = useMutation({
    mutationFn: () => api.rotateHmac(envId),
    onSuccess: (r) => {
      setRotatedSecret(r.subscriber_hmac_secret);
      toast.success('Rotated — both secrets verify during the overlap');
      qc.invalidateQueries({ queryKey: ['environment', envId] });
    },
    onError: (e) => toast.error(e instanceof ApiRequestError ? e.message : 'Failed'),
  });

  const complete = useMutation({
    mutationFn: () => api.completeHmacRotation(envId),
    onSuccess: () => {
      setRotatedSecret(null);
      toast.success('Rotation completed — previous secret cleared');
      qc.invalidateQueries({ queryKey: ['environment', envId] });
    },
    onError: (e) => toast.error(e instanceof ApiRequestError ? e.message : 'Failed'),
  });

  return (
    <Async query={env} emptyTitle="Environment not found">
      {(detail) => (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <KeyRound className="size-4 text-primary" /> Subscriber HMAC secret
            </CardTitle>
            <CardDescription>
              The customer backend computes <code>HMAC-SHA256(secret, subscriber_id)</code> with
              this. Rotation uses two slots so live widget sessions never break.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-5">
            <div className="flex flex-col gap-1.5">
              <Label>Current secret</Label>
              <CopyField value={detail.subscriber_hmac_secret} maskable />
            </div>

            {detail.has_previous_secret && (
              <div className="rounded-md border border-warning/40 bg-warning/10 p-3 text-sm">
                Rotation in progress — the previous secret still verifies. Update every backend,
                then complete the rotation.
                {detail.subscriber_hmac_rotated_at && (
                  <span className="block text-muted-foreground">
                    Rotated {formatTs(detail.subscriber_hmac_rotated_at)}
                  </span>
                )}
              </div>
            )}

            {rotatedSecret && (
              <div className="flex flex-col gap-2 rounded-md border border-warning-hi/40 bg-warning-hi/10 p-3">
                <p className="text-sm font-medium">New secret — copy it into your backend now.</p>
                <CopyField value={rotatedSecret} />
              </div>
            )}

            <Separator />

            <div className="flex flex-wrap gap-2">
              <Button onClick={() => rotate.mutate()} disabled={rotate.isPending}>
                <RefreshCw className={rotate.isPending ? 'animate-spin' : ''} /> Rotate secret
              </Button>
              <Button
                variant="outline"
                onClick={() => complete.mutate()}
                disabled={!detail.has_previous_secret || complete.isPending}
              >
                Complete rotation
              </Button>
            </div>
          </CardContent>
        </Card>
      )}
    </Async>
  );
}
