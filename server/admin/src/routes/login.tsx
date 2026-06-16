import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { type AdminMe, api, ApiRequestError } from '@/lib/api';

// Branded login screen. Shown by AuthGate whenever there is no live session.
// On success it hands the resolved user up so the app renders without an
// extra /me round-trip.
export function LoginRoute({ onAuthenticated }: { onAuthenticated: (me: AdminMe) => void }) {
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const doLogin = async () => {
    setPending(true);
    setError(null);
    try {
      const me = await api.login(email.trim(), password);
      onAuthenticated(me);
    } catch (err) {
      setError(err instanceof ApiRequestError ? err.message : 'Login failed');
      setPending(false);
    }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-6">
      <div className="w-full max-w-sm">
        <div className="mb-8 flex items-center gap-2">
          <span className="inline-block size-3 rounded-full bg-primary" />
          <span className="text-2xl font-semibold tracking-tight">Dronte</span>
          <span className="text-sm text-muted-foreground">admin</span>
        </div>
        <div className="rounded-xl border border-border bg-card p-6 shadow-sm">
          <h1 className="text-lg font-semibold">Sign in</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Operator access to this Dronte instance.
          </p>
          <form
            className="mt-6 flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              void doLogin();
            }}
          >
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="email">Email</Label>
              <Input
                id="email"
                type="email"
                autoComplete="username"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                required
                // biome-ignore lint/a11y/noAutofocus: the login form is the only field on the page
                autoFocus
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="password">Password</Label>
              <Input
                id="password"
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                required
              />
            </div>
            {error && (
              <div className="rounded-md border border-danger/40 bg-danger/10 p-3 text-sm text-danger">
                {error}
              </div>
            )}
            <Button type="submit" disabled={pending} className="mt-2">
              {pending ? 'Signing in…' : 'Sign in'}
            </Button>
          </form>
        </div>
      </div>
    </div>
  );
}
