import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Plus } from 'lucide-react';
import { useState } from 'react';
import { toast } from 'sonner';
import { CopyField } from '@/components/copy-field';
import { Async, EmptyState } from '@/components/states';
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
import { Select } from '@/components/ui/select';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { ADMIN_ROLES, type AdminRole, type AdminUserView, api, ApiRequestError } from '@/lib/api';
import { CAP, useAuth } from '@/lib/auth';
import { formatTs } from '@/lib/utils';

// A strong, human-shareable temporary password (>= 12 chars). Shown once.
function generatePassword(): string {
  const bytes = new Uint8Array(18);
  crypto.getRandomValues(bytes);
  return btoa(String.fromCharCode(...bytes)).replace(/[+/=]/g, '').slice(0, 20);
}

const failMessage = (e: unknown) => (e instanceof ApiRequestError ? e.message : 'Request failed');

export function UsersRoute() {
  const { user, has } = useAuth();
  const enabled = has(CAP.userManage);
  const users = useQuery({ queryKey: ['admin-users'], queryFn: api.listUsers, enabled });

  if (!enabled) {
    return <EmptyState title="Not authorized" hint="Managing users requires the admin role." />;
  }

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Users</h1>
          <p className="text-sm text-muted-foreground">
            Instance-wide admin accounts. Roles are fixed presets; one per user.
          </p>
        </div>
        <NewUserDialog />
      </div>

      <Async query={users} emptyTitle="No admin users">
        {(rows) => (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Email</TableHead>
                <TableHead>Name</TableHead>
                <TableHead>Role</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Created</TableHead>
                <TableHead />
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((row) => (
                <UserRow key={row.id} row={row} selfId={user.id} />
              ))}
            </TableBody>
          </Table>
        )}
      </Async>
    </div>
  );
}

function UserRow({ row, selfId }: { row: AdminUserView; selfId: string }) {
  const qc = useQueryClient();
  const isSelf = row.id === selfId;
  const invalidate = () => qc.invalidateQueries({ queryKey: ['admin-users'] });

  const setRole = useMutation({
    mutationFn: (role: AdminRole) => api.updateUser(row.id, { role }),
    onSuccess: () => {
      toast.success('Role updated');
      invalidate();
    },
    onError: (e) => {
      toast.error(failMessage(e));
      invalidate();
    },
  });

  const setDisabled = useMutation({
    mutationFn: (disabled: boolean) => api.updateUser(row.id, { disabled }),
    onSuccess: (u) => {
      toast.success(u.disabled ? 'User disabled' : 'User enabled');
      invalidate();
    },
    onError: (e) => toast.error(failMessage(e)),
  });

  return (
    <TableRow>
      <TableCell className="font-mono">{row.email}</TableCell>
      <TableCell>{row.name}</TableCell>
      <TableCell>
        <Select
          aria-label={`Role for ${row.email}`}
          value={row.role}
          disabled={setRole.isPending}
          onChange={(e) => setRole.mutate(e.target.value as AdminRole)}
          className="h-8 w-32 capitalize"
        >
          {ADMIN_ROLES.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </Select>
      </TableCell>
      <TableCell>
        {row.disabled ? (
          <Badge variant="danger">disabled</Badge>
        ) : (
          <Badge variant="success">active</Badge>
        )}
      </TableCell>
      <TableCell className="text-muted-foreground">{formatTs(row.created_at)}</TableCell>
      <TableCell className="text-right">
        <div className="flex justify-end gap-1">
          <ResetPasswordDialog user={row} />
          <Button
            variant="ghost"
            size="sm"
            disabled={isSelf || setDisabled.isPending}
            title={isSelf ? 'You cannot disable your own account' : undefined}
            onClick={() => setDisabled.mutate(!row.disabled)}
          >
            {row.disabled ? 'Enable' : 'Disable'}
          </Button>
          <DeleteUserDialog user={row} disabled={isSelf} />
        </div>
      </TableCell>
    </TableRow>
  );
}

function NewUserDialog() {
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [email, setEmail] = useState('');
  const [name, setName] = useState('');
  const [role, setRole] = useState<AdminRole>('viewer');
  const [password, setPassword] = useState(generatePassword());
  const [created, setCreated] = useState<{ email: string; password: string } | null>(null);

  const reset = () => {
    setEmail('');
    setName('');
    setRole('viewer');
    setPassword(generatePassword());
    setCreated(null);
  };

  const create = useMutation({
    mutationFn: () => api.createUser({ email, name, role, password }),
    onSuccess: () => {
      setCreated({ email, password });
      toast.success('User created — share the temporary password now');
      qc.invalidateQueries({ queryKey: ['admin-users'] });
    },
    onError: (e) => toast.error(failMessage(e)),
  });

  const close = (o: boolean) => {
    setOpen(o);
    if (!o) reset();
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogTrigger asChild>
        <Button>
          <Plus /> New user
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New admin user</DialogTitle>
          <DialogDescription>
            They sign in with this email and the temporary password, then change it.
          </DialogDescription>
        </DialogHeader>
        {created ? (
          <div className="flex flex-col gap-3">
            <div className="rounded-md border border-warning-hi/40 bg-warning-hi/10 p-3 text-sm">
              Share this temporary password with <span className="font-mono">{created.email}</span>{' '}
              now. It is shown only once.
            </div>
            <CopyField value={created.password} />
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
              <Label htmlFor="user-email">Email</Label>
              <Input
                id="user-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="operator@example.com"
                required
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="user-name">Name</Label>
              <Input
                id="user-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Alex Operator"
                required
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="user-role">Role</Label>
              <Select
                id="user-role"
                value={role}
                onChange={(e) => setRole(e.target.value as AdminRole)}
                className="capitalize"
              >
                {ADMIN_ROLES.map((r) => (
                  <option key={r} value={r}>
                    {r}
                  </option>
                ))}
              </Select>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>Temporary password (shown once)</Label>
              <CopyField value={password} />
            </div>
            <DialogFooter>
              <Button type="submit" disabled={create.isPending}>
                {create.isPending ? 'Creating…' : 'Create user'}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

function ResetPasswordDialog({ user }: { user: AdminUserView }) {
  const [open, setOpen] = useState(false);
  const [password, setPassword] = useState(generatePassword());
  const [done, setDone] = useState(false);

  const reset = useMutation({
    mutationFn: () => api.setUserPassword(user.id, password),
    onSuccess: () => {
      setDone(true);
      toast.success('Password reset — share it now');
    },
    onError: (e) => toast.error(failMessage(e)),
  });

  const close = (o: boolean) => {
    setOpen(o);
    if (!o) {
      setPassword(generatePassword());
      setDone(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogTrigger asChild>
        <Button variant="ghost" size="sm">
          Reset password
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>Reset password</DialogTitle>
          <DialogDescription>
            Set a new temporary password for <span className="font-mono">{user.email}</span>.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <CopyField value={password} />
          {done ? (
            <div className="rounded-md border border-success/40 bg-success/10 p-3 text-sm">
              Password updated. Share it with the user now.
            </div>
          ) : null}
          <DialogFooter>
            {done ? (
              <Button onClick={() => close(false)}>Done</Button>
            ) : (
              <Button onClick={() => reset.mutate()} disabled={reset.isPending}>
                {reset.isPending ? 'Resetting…' : 'Set password'}
              </Button>
            )}
          </DialogFooter>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function DeleteUserDialog({ user, disabled }: { user: AdminUserView; disabled: boolean }) {
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const del = useMutation({
    mutationFn: () => api.deleteUser(user.id),
    onSuccess: () => {
      toast.success('User deleted');
      qc.invalidateQueries({ queryKey: ['admin-users'] });
      setOpen(false);
    },
    onError: (e) => toast.error(failMessage(e)),
  });

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="text-danger hover:text-danger"
          disabled={disabled}
          title={disabled ? 'You cannot delete your own account' : undefined}
        >
          Delete
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>Delete “{user.email}”?</DialogTitle>
          <DialogDescription>
            Their sessions end immediately and the account is removed. This cannot be undone.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={() => del.mutate()} disabled={del.isPending}>
            Delete user
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
