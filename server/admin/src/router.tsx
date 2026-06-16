import { createRootRoute, createRoute, createRouter } from '@tanstack/react-router';
import { Layout } from '@/components/layout';
import { BroadcastsRoute } from '@/routes/broadcasts';
import { DashboardRoute } from '@/routes/dashboard';
import { DlqRoute } from '@/routes/dlq';
import { EnvironmentDetailRoute } from '@/routes/environment-detail';
import { EnvironmentsRoute } from '@/routes/environments';
import { NotificationsRoute } from '@/routes/notifications';
import { SubscribersRoute } from '@/routes/subscribers';
import { UsersRoute } from '@/routes/users';

const rootRoute = createRootRoute({ component: Layout });

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  component: DashboardRoute,
});

const environmentsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'environments',
  component: EnvironmentsRoute,
});

const environmentDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'environments/$envId',
  component: EnvironmentDetailRoute,
});

const notificationsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'environments/$envId/notifications',
  component: NotificationsRoute,
});

const broadcastsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'environments/$envId/broadcasts',
  component: BroadcastsRoute,
});

const subscribersRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'environments/$envId/subscribers',
  component: SubscribersRoute,
});

const dlqRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'dlq',
  component: DlqRoute,
});

const usersRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'users',
  component: UsersRoute,
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  environmentsRoute,
  environmentDetailRoute,
  notificationsRoute,
  broadcastsRoute,
  subscribersRoute,
  dlqRoute,
  usersRoute,
]);

export const router = createRouter({
  routeTree,
  basepath: '/admin',
  defaultPreload: 'intent',
});

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router;
  }
}
