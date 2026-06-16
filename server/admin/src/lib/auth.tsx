import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from 'react';
import { LoginRoute } from '@/routes/login';
import { type AdminMe, api } from '@/lib/api';

// Capability strings (mirror server/src/roles.rs). The UI gates on these for
// convenience; the server enforces them for real.
export const CAP = {
  read: 'read',
  dlqReplay: 'dlq:replay',
  broadcastCompose: 'broadcast:compose',
  apikeyRead: 'apikey:read',
  apikeyManage: 'apikey:manage',
  envReadSecret: 'env:read_secret',
  envCreate: 'env:create',
  hmacRotate: 'hmac:rotate',
  userManage: 'user:manage',
} as const;

interface AuthContextValue {
  user: AdminMe;
  has: (capability: string) => boolean;
  refresh: () => Promise<void>;
  logout: () => Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

function FullScreenSpinner() {
  return (
    <div className="flex min-h-screen items-center justify-center">
      <div className="flex items-center gap-2 text-muted-foreground">
        <span className="size-2.5 animate-pulse rounded-full bg-primary" />
        <span className="text-sm">Loading…</span>
      </div>
    </div>
  );
}

// Resolves the signed-in admin before rendering the app. While loading shows a
// spinner, unauthenticated renders the login screen, authenticated provides
// the user + capability checks to the tree. A 401 from any API call routes
// back to login via the `dronte-admin-unauthorized` event.
export function AuthGate({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<AdminMe | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    try {
      setUser(await api.me());
    } catch {
      setUser(null);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    const onUnauthorized = () => setUser(null);
    window.addEventListener('dronte-admin-unauthorized', onUnauthorized);
    return () => window.removeEventListener('dronte-admin-unauthorized', onUnauthorized);
  }, []);

  const logout = useCallback(async () => {
    try {
      await api.logout();
    } catch {
      // Even if the network call fails, drop the local session view.
    }
    setUser(null);
  }, []);

  if (loading) return <FullScreenSpinner />;
  if (!user) return <LoginRoute onAuthenticated={setUser} />;

  const value: AuthContextValue = {
    user,
    has: (capability) => user.capabilities.includes(capability),
    refresh: load,
    logout,
  };
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error('useAuth must be used within AuthGate');
  return ctx;
}
