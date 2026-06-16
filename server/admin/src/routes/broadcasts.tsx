import { useMutation } from '@tanstack/react-query';
import { useParams } from '@tanstack/react-router';
import { Radio } from 'lucide-react';
import { useState } from 'react';
import { toast } from 'sonner';
import { EnvNav } from '@/components/env-nav';
import { EmptyState } from '@/components/states';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Textarea } from '@/components/ui/textarea';
import { api, ApiRequestError } from '@/lib/api';
import { CAP, useAuth } from '@/lib/auth';

export function BroadcastsRoute() {
  const { envId } = useParams({ strict: false }) as { envId: string };
  const { has } = useAuth();
  const [category, setCategory] = useState('');
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [actionUrl, setActionUrl] = useState('');
  const [iconUrl, setIconUrl] = useState('');
  const [extraJson, setExtraJson] = useState('');
  const [idemKey, setIdemKey] = useState('');
  const [lastId, setLastId] = useState<string | null>(null);

  const compose = useMutation({
    mutationFn: () => {
      const payload: Record<string, unknown> = {};
      if (title) payload.title = title;
      if (body) payload.body = body;
      if (actionUrl) payload.action_url = actionUrl;
      if (iconUrl) payload.icon_url = iconUrl;
      if (extraJson.trim()) {
        const extra = JSON.parse(extraJson) as Record<string, unknown>;
        Object.assign(payload, extra);
      }
      return api.createBroadcast(envId, {
        category,
        payload,
        idempotency_key: idemKey || undefined,
      });
    },
    onSuccess: (b) => {
      setLastId(b.id);
      toast.success(`Broadcast ${b.id} composed`);
    },
    onError: (e) => {
      if (e instanceof SyntaxError) toast.error('Extra JSON is not valid JSON');
      else toast.error(e instanceof ApiRequestError ? e.message : 'Failed to compose');
    },
  });

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Compose broadcast</h1>
        <p className="text-sm text-muted-foreground">
          One row per announcement, fanned out on read — never materialized per subscriber.
        </p>
      </div>
      <EnvNav envId={envId} />

      {has(CAP.broadcastCompose) ? (
        <Card className="max-w-2xl">
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Radio className="size-4 text-primary" /> New broadcast
          </CardTitle>
          <CardDescription>
            Subscribers see it only if it was created at or after their own creation time.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form
            className="flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              compose.mutate();
            }}
          >
            <Field label="Category" required>
              <Input value={category} onChange={(e) => setCategory(e.target.value)} placeholder="product.update" required />
            </Field>
            <div className="rounded-md border border-border p-4">
              <p className="mb-3 text-sm font-medium">Well-known payload fields</p>
              <div className="flex flex-col gap-4">
                <Field label="Title">
                  <Input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="We shipped X" />
                </Field>
                <Field label="Body">
                  <Textarea value={body} onChange={(e) => setBody(e.target.value)} className="font-sans" placeholder="Plain text, never HTML." />
                </Field>
                <Field label="Action URL">
                  <Input value={actionUrl} onChange={(e) => setActionUrl(e.target.value)} placeholder="https://example.com/changelog" />
                </Field>
                <Field label="Icon URL">
                  <Input value={iconUrl} onChange={(e) => setIconUrl(e.target.value)} placeholder="https://example.com/icon.png" />
                </Field>
              </div>
            </div>
            <Field label="Extra payload (JSON, merged into the well-known fields)">
              <Textarea
                value={extraJson}
                onChange={(e) => setExtraJson(e.target.value)}
                placeholder={'{\n  "feature_flag": "beta"\n}'}
              />
            </Field>
            <Field label="Idempotency key (optional)">
              <Input value={idemKey} onChange={(e) => setIdemKey(e.target.value)} placeholder="auto-generated if blank" />
            </Field>
            <div className="flex items-center gap-3">
              <Button type="submit" disabled={compose.isPending}>
                {compose.isPending ? 'Composing…' : 'Compose broadcast'}
              </Button>
              {lastId && <span className="font-mono text-sm text-success-foreground dark:text-success">{lastId}</span>}
            </div>
          </form>
        </CardContent>
        </Card>
      ) : (
        <EmptyState
          title="Not authorized"
          hint="Composing broadcasts requires the operator or admin role."
        />
      )}
    </div>
  );
}

function Field({
  label,
  required,
  children,
}: {
  label: string;
  required?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label>
        {label}
        {required && <span className="text-danger"> *</span>}
      </Label>
      {children}
    </div>
  );
}
