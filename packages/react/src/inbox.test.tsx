import type { ChimelyClient } from '@chimely/client';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { ChimelyProvider } from './context';
import type { InboxProps } from './Inbox';
import { Inbox } from './Inbox';
import { navigation } from './navigation';
import type { StubServer } from './test-support/setup';
import { createStubServer, loadClient, makeClient } from './test-support/setup';

type IntersectionCallback = (entries: Array<{ isIntersecting: boolean }>) => void;

class MockIntersectionObserver {
  static instances: MockIntersectionObserver[] = [];
  readonly callback: IntersectionCallback;

  constructor(callback: IntersectionCallback) {
    this.callback = callback;
    MockIntersectionObserver.instances.push(this);
  }

  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}

  static intersect(): void {
    for (const instance of MockIntersectionObserver.instances) {
      instance.callback([{ isIntersecting: true }]);
    }
  }
}

async function renderInbox(
  stub: StubServer,
  props: InboxProps = {},
  clientConfig: { pageSize?: number } = {},
): Promise<{ client: ChimelyClient; unmount: () => void }> {
  const client = makeClient(stub, clientConfig);
  await loadClient(client, stub);
  const { unmount } = render(
    <ChimelyProvider client={client}>
      <Inbox {...props} />
    </ChimelyProvider>,
  );
  return { client, unmount };
}

function bell(): HTMLElement {
  return screen.getByRole('button', { name: 'Notifications' });
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  MockIntersectionObserver.instances = [];
  document.querySelector('style[data-chimely]')?.remove();
});

describe('bell and badge', () => {
  test('renders the bell with the unseen badge and injects the stylesheet once', async () => {
    const stub = createStubServer();
    stub.addNotification();
    stub.addNotification();
    await renderInbox(stub);

    expect(bell()).toBeDefined();
    expect(screen.getByText('2')).toBeDefined();
    expect(document.querySelectorAll('style[data-chimely]')).toHaveLength(1);
  });

  test('no badge when nothing is unseen', async () => {
    const stub = createStubServer();
    stub.addNotification({ seen: true });
    await renderInbox(stub);
    expect(document.querySelector('.chimely-badge')).toBeNull();
  });

  test('renderBell fully replaces the bell contents', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub, {
      renderBell: ({ unseenCount, open }) => (
        <span data-testid="custom-bell">{`${unseenCount}:${open ? 'open' : 'closed'}`}</span>
      ),
    });
    expect(screen.getByTestId('custom-bell').textContent).toBe('1:closed');
    expect(document.querySelector('.chimely-badge')).toBeNull();
  });
});

describe('popover', () => {
  test('opening calls markAllSeen and clears the badge without touching read state', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const { client } = await renderInbox(stub);

    fireEvent.click(bell());
    expect(screen.getByRole('dialog')).toBeDefined();
    await waitFor(() => {
      expect(stub.requestsFor('/v1/inbox/seen-all')).toHaveLength(1);
    });
    expect(client.getSnapshot().counts.unseen).toBe(0);
    expect(client.getSnapshot().counts.unread).toBe(1);
    expect(document.querySelector('.chimely-badge')).toBeNull();
  });

  test('escape closes the popover', async () => {
    const stub = createStubServer();
    await renderInbox(stub);
    fireEvent.click(bell());
    expect(screen.getByRole('dialog')).toBeDefined();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  test('renders items newest first with the unread slot class', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'older' }, read: true });
    stub.addBroadcast({ payload: { title: 'newer' } });
    await renderInbox(stub);
    fireEvent.click(bell());

    const titles = [...document.querySelectorAll('.chimely-item-title')].map(
      (node) => node.textContent,
    );
    expect(titles).toEqual(['newer', 'older']);
    const unreadItems = document.querySelectorAll('.chimely-item-unread');
    expect(unreadItems).toHaveLength(1);
    expect(unreadItems[0]?.textContent).toContain('newer');
  });

  test('shows the localized empty state', async () => {
    const stub = createStubServer();
    await renderInbox(stub, {
      localization: { emptyTitle: 'Rien', emptyBody: 'Tout est lu.' },
    });
    fireEvent.click(bell());
    expect(screen.getByText('Rien')).toBeDefined();
    expect(screen.getByText('Tout est lu.')).toBeDefined();
  });

  test('renderEmpty fully replaces the empty slot', async () => {
    const stub = createStubServer();
    await renderInbox(stub, { renderEmpty: () => <span data-testid="custom-empty">nada</span> });
    fireEvent.click(bell());
    expect(screen.getByTestId('custom-empty')).toBeDefined();
    expect(screen.queryByText('No notifications')).toBeNull();
  });
});

describe('item click behavior', () => {
  test('default click marks read then follows payload.action_url', async () => {
    const assign = vi.spyOn(navigation, 'assign').mockImplementation(() => {});
    const stub = createStubServer();
    const item = stub.addNotification({
      payload: { title: 'pay up', action_url: 'https://app.test/invoices/42' },
    });
    await renderInbox(stub);
    fireEvent.click(bell());

    fireEvent.click(screen.getByText('pay up'));
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)).toHaveLength(1);
    });
    expect(assign).toHaveBeenCalledWith('https://app.test/invoices/42');
    const readRequest = stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)[0];
    expect(readRequest?.status).toBe(204);
  });

  test('a javascript: action_url marks read but never navigates', async () => {
    const assign = vi.spyOn(navigation, 'assign').mockImplementation(() => {});
    const stub = createStubServer();
    const item = stub.addNotification({
      payload: { title: 'hostile', action_url: 'javascript:alert(1)' },
    });
    await renderInbox(stub);
    fireEvent.click(bell());

    fireEvent.click(screen.getByText('hostile'));
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)).toHaveLength(1);
    });
    expect(assign).not.toHaveBeenCalled();
  });

  test('a relative action_url navigates (it resolves same-origin)', async () => {
    const assign = vi.spyOn(navigation, 'assign').mockImplementation(() => {});
    const stub = createStubServer();
    const item = stub.addNotification({
      payload: { title: 'relative', action_url: '/invoices/42' },
    });
    await renderInbox(stub);
    fireEvent.click(bell());

    fireEvent.click(screen.getByText('relative'));
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)).toHaveLength(1);
    });
    expect(assign).toHaveBeenCalledWith('/invoices/42');
  });

  test('onItemClick returning false suppresses the default behavior', async () => {
    const assign = vi.spyOn(navigation, 'assign').mockImplementation(() => {});
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'silent', action_url: 'https://app.test/x' } });
    const onItemClick = vi.fn(() => false);
    await renderInbox(stub, { onItemClick });
    fireEvent.click(bell());

    fireEvent.click(screen.getByText('silent'));
    expect(onItemClick).toHaveBeenCalledTimes(1);
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(stub.requestsFor('/read')).toHaveLength(0);
    expect(assign).not.toHaveBeenCalled();
  });

  test('onItemClick returning nothing lets the default run', async () => {
    vi.spyOn(navigation, 'assign').mockImplementation(() => {});
    const stub = createStubServer();
    const item = stub.addNotification({ payload: { title: 'observed' } });
    const onItemClick = vi.fn();
    await renderInbox(stub, { onItemClick });
    fireEvent.click(bell());

    fireEvent.click(screen.getByText('observed'));
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)).toHaveLength(1);
    });
  });

  test('renderItem fully replaces the item slot including click wiring', async () => {
    const stub = createStubServer();
    const item = stub.addNotification({ payload: { title: 'hidden default' } });
    await renderInbox(stub, {
      renderItem: ({ item: rendered, markRead }) => (
        <button type="button" data-testid="custom-item" onClick={() => void markRead()}>
          {rendered.category}
        </button>
      ),
    });
    fireEvent.click(bell());

    expect(screen.queryByText('hidden default')).toBeNull();
    fireEvent.click(screen.getByTestId('custom-item'));
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)).toHaveLength(1);
    });
  });
});

describe('portal', () => {
  test('portal renders the popover under document.body with theme variables', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub, {
      portal: true,
      appearance: { variables: { colorPrimary: '#ff0000' } },
    });
    fireEvent.click(bell());

    const dialog = screen.getByRole('dialog');
    expect(dialog.parentElement).toBe(document.body);
    expect(dialog.classList.contains('chimely-popover-portal')).toBe(true);
    expect((dialog as HTMLElement).style.getPropertyValue('--chimely-colorPrimary')).toBe(
      '#ff0000',
    );
  });

  test('outside click closes a portaled popover, inside click does not', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub, { portal: true });
    fireEvent.click(bell());

    fireEvent.pointerDown(screen.getByRole('dialog'));
    expect(screen.getByRole('dialog')).toBeDefined();

    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  test('escape closes and refocuses the bell', async () => {
    const stub = createStubServer();
    await renderInbox(stub);
    fireEvent.click(bell());
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.activeElement).toBe(bell());
  });
});

describe('controlled open', () => {
  function controlledView(
    client: ChimelyClient,
    open: boolean,
    onOpenChange: (o: boolean) => void,
  ) {
    return (
      <ChimelyProvider client={client}>
        <Inbox open={open} onOpenChange={onOpenChange} />
      </ChimelyProvider>
    );
  }

  test('open intents surface via onOpenChange, state stays with the parent', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const client = makeClient(stub);
    await loadClient(client, stub);
    const onOpenChange = vi.fn();
    const { rerender } = render(controlledView(client, false, onOpenChange));

    fireEvent.click(bell());
    expect(onOpenChange).toHaveBeenCalledWith(true);
    expect(screen.queryByRole('dialog')).toBeNull();

    rerender(controlledView(client, true, onOpenChange));
    expect(screen.getByRole('dialog')).toBeDefined();
    await waitFor(() => {
      expect(stub.requestsFor('/v1/inbox/seen-all')).toHaveLength(1);
    });

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(screen.getByRole('dialog')).toBeDefined();

    rerender(controlledView(client, false, onOpenChange));
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  test('mounting with open already true fires markAllSeen once', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const client = makeClient(stub);
    await loadClient(client, stub);
    render(controlledView(client, true, () => {}));

    expect(screen.getByRole('dialog')).toBeDefined();
    await waitFor(() => {
      expect(stub.requestsFor('/v1/inbox/seen-all')).toHaveLength(1);
    });
  });

  test('uncontrolled mode still reports transitions through onOpenChange', async () => {
    const stub = createStubServer();
    const onOpenChange = vi.fn();
    await renderInbox(stub, { onOpenChange });
    fireEvent.click(bell());
    expect(onOpenChange).toHaveBeenCalledWith(true);
    expect(screen.getByRole('dialog')).toBeDefined();
  });
});

describe('routerPush', () => {
  test('same-origin action URLs go through routerPush in path form', async () => {
    const assign = vi.spyOn(navigation, 'assign').mockImplementation(() => {});
    const routerPush = vi.fn();
    const stub = createStubServer();
    const item = stub.addNotification({
      payload: { title: 'spa', action_url: '/invoices/42?x=1#y' },
    });
    await renderInbox(stub, { routerPush });
    fireEvent.click(bell());

    fireEvent.click(screen.getByText('spa'));
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)).toHaveLength(1);
    });
    expect(routerPush).toHaveBeenCalledWith('/invoices/42?x=1#y');
    expect(assign).not.toHaveBeenCalled();
  });

  test('absolute same-origin URLs are normalized before routerPush', async () => {
    vi.spyOn(navigation, 'assign').mockImplementation(() => {});
    const routerPush = vi.fn();
    const stub = createStubServer();
    stub.addNotification({
      payload: { title: 'absolute', action_url: `${window.location.origin}/invoices/9` },
    });
    await renderInbox(stub, { routerPush });
    fireEvent.click(bell());

    fireEvent.click(screen.getByText('absolute'));
    await waitFor(() => {
      expect(routerPush).toHaveBeenCalledWith('/invoices/9');
    });
  });

  test('cross-origin URLs bypass routerPush and use full navigation', async () => {
    const assign = vi.spyOn(navigation, 'assign').mockImplementation(() => {});
    const routerPush = vi.fn();
    const stub = createStubServer();
    stub.addNotification({
      payload: { title: 'external', action_url: 'https://elsewhere.test/x' },
    });
    await renderInbox(stub, { routerPush });
    fireEvent.click(bell());

    fireEvent.click(screen.getByText('external'));
    await waitFor(() => {
      expect(assign).toHaveBeenCalledWith('https://elsewhere.test/x');
    });
    expect(routerPush).not.toHaveBeenCalled();
  });

  test('unsafe URLs navigate through neither path', async () => {
    const assign = vi.spyOn(navigation, 'assign').mockImplementation(() => {});
    const routerPush = vi.fn();
    const stub = createStubServer();
    const item = stub.addNotification({
      payload: { title: 'hostile', action_url: 'javascript:alert(1)' },
    });
    await renderInbox(stub, { routerPush });
    fireEvent.click(bell());

    fireEvent.click(screen.getByText('hostile'));
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)).toHaveLength(1);
    });
    expect(assign).not.toHaveBeenCalled();
    expect(routerPush).not.toHaveBeenCalled();
  });
});

describe('header title and footer', () => {
  test('renders the default header title and renderFooter content', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub, {
      renderFooter: () => <span data-testid="footer-brand">Acme</span>,
    });
    fireEvent.click(bell());
    expect(document.querySelector('.chimely-header-title')?.textContent).toBe('Notifications');
    expect(screen.getByTestId('footer-brand').closest('.chimely-footer')).not.toBeNull();
  });

  test('inboxTitle overrides the header title and the dialog label', async () => {
    const stub = createStubServer();
    await renderInbox(stub, { localization: { inboxTitle: 'Meldungen' } });
    fireEvent.click(bell());
    expect(screen.getByRole('dialog', { name: 'Meldungen' })).toBeDefined();
    expect(document.querySelector('.chimely-header-title')?.textContent).toBe('Meldungen');
  });
});

describe('localized labels', () => {
  test('bellLabel and backLabel drive the aria labels', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'billing.alerts' });
    await renderInbox(stub, {
      localization: { bellLabel: 'Meine Glocke', backLabel: 'Zurueck' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Meine Glocke' }));
    fireEvent.click(screen.getByRole('button', { name: 'Notification preferences' }));
    expect(screen.getByRole('button', { name: 'Zurueck' })).toBeDefined();
  });

  test('categoryLabels rename preference rows, unknown keys fall back', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'billing.alerts' });
    stub.addNotification({ category: 'security.alerts' });
    await renderInbox(stub, {
      localization: { categoryLabels: { 'billing.alerts': 'Billing' } },
    });
    fireEvent.click(bell());
    fireEvent.click(screen.getByRole('button', { name: 'Notification preferences' }));
    expect(screen.getByText('Billing')).toBeDefined();
    expect(screen.queryByText('billing.alerts')).toBeNull();
    expect(screen.getByText('security.alerts')).toBeDefined();
  });
});

describe('timestamps', () => {
  test('recent items read as relative time with absolute dateTime and title', async () => {
    const stub = createStubServer();
    stub.addNotification({
      payload: { title: 'fresh' },
      occurred_at: new Date(Date.now() - 30_000).toISOString(),
    });
    const { client } = await renderInbox(stub);
    fireEvent.click(bell());

    const time = document.querySelector('.chimely-item-time') as HTMLTimeElement;
    const occurredAt = client.getSnapshot().items[0]?.occurredAt as string;
    expect(time.textContent).toBe('now');
    expect(time.getAttribute('datetime')).toBe(occurredAt);
    expect(time.getAttribute('title')).toBe(new Date(occurredAt).toLocaleString());
  });

  test('old items fall back to the locale date', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'ancient' } });
    const { client } = await renderInbox(stub);
    fireEvent.click(bell());

    const occurredAt = client.getSnapshot().items[0]?.occurredAt as string;
    expect(document.querySelector('.chimely-item-time')?.textContent).toBe(
      new Date(occurredAt).toLocaleDateString(),
    );
  });

  test('formatTimestamp override wins', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub, {
      localization: { formatTimestamp: (iso) => `ts:${iso}` },
    });
    fireEvent.click(bell());
    expect(document.querySelector('.chimely-item-time')?.textContent).toMatch(/^ts:/);
  });
});

describe('header actions', () => {
  test('mark all read uses the localized label and hits read-all', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const { client } = await renderInbox(stub, { localization: { markAllRead: 'Tout lire' } });
    fireEvent.click(bell());

    fireEvent.click(screen.getByText('Tout lire'));
    await waitFor(() => {
      expect(stub.requestsFor('/v1/inbox/read-all')).toHaveLength(1);
    });
    expect(client.getSnapshot().counts.unread).toBe(0);
  });
});

describe('preferences panel', () => {
  test('toggles per-category in_app preferences', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'billing.alerts' });
    await renderInbox(stub);
    fireEvent.click(bell());

    fireEvent.click(screen.getByRole('button', { name: 'Notification preferences' }));
    expect(screen.getByText('Notification preferences')).toBeDefined();

    const checkbox = screen.getByRole('checkbox');
    expect((checkbox as HTMLInputElement).checked).toBe(true);

    fireEvent.click(checkbox);
    await waitFor(() => {
      expect(stub.requestsFor('/v1/inbox/preferences').some((r) => r.method === 'PUT')).toBe(true);
    });
    const write = stub.requestsFor('/v1/inbox/preferences').find((r) => r.method === 'PUT');
    expect(write?.body).toEqual({
      preferences: [{ category: 'billing.alerts', channel: 'in_app', enabled: false }],
    });
    expect((screen.getByRole('checkbox') as HTMLInputElement).checked).toBe(false);
  });

  test('preferencesPanel={false} hides the panel toggle', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub, { preferencesPanel: false });
    fireEvent.click(bell());
    expect(screen.queryByRole('button', { name: 'Notification preferences' })).toBeNull();
  });
});

describe('appearance', () => {
  test('variables land on the root as --chimely-* custom properties, forwarded verbatim', async () => {
    const stub = createStubServer();
    await renderInbox(stub, {
      appearance: {
        variables: {
          colorPrimary: '#ff0000',
          fontSize: '16px',
          customThing: '4px',
        },
      },
    });
    const root = document.querySelector('.chimely-root') as HTMLElement;
    expect(root.style.getPropertyValue('--chimely-colorPrimary')).toBe('#ff0000');
    expect(root.style.getPropertyValue('--chimely-fontSize')).toBe('16px');
    expect(root.style.getPropertyValue('--chimely-customThing')).toBe('4px');
  });

  test('slot classNames apply alongside the default classes', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub, {
      appearance: {
        classNames: {
          root: 'my-root',
          bell: 'my-bell',
          badge: 'my-badge',
          popover: 'my-popover',
          item: 'my-item',
          itemUnread: 'my-unread',
        },
      },
    });
    expect(document.querySelector('.chimely-root.my-root')).not.toBeNull();
    expect(document.querySelector('.chimely-bell.my-bell')).not.toBeNull();
    expect(document.querySelector('.chimely-badge.my-badge')).not.toBeNull();

    fireEvent.click(bell());
    expect(document.querySelector('.chimely-popover.my-popover')).not.toBeNull();
    expect(document.querySelector('.chimely-item.my-item')).not.toBeNull();
    expect(document.querySelector('.chimely-item-unread.my-unread')).not.toBeNull();
  });
});

describe('infinite scroll', () => {
  test('a visible end sentinel drains pages until the inbox is exhausted', async () => {
    vi.stubGlobal('IntersectionObserver', MockIntersectionObserver);
    const stub = createStubServer();
    for (let i = 0; i < 5; i += 1) {
      stub.addNotification();
    }
    const { client } = await renderInbox(stub, {}, { pageSize: 2 });
    fireEvent.click(bell());
    expect(client.getSnapshot().items).toHaveLength(2);
    expect(MockIntersectionObserver.instances.length).toBeGreaterThan(0);

    // The mock never reports the sentinel leaving, so one intersection
    // drives the fill loop through every remaining page.
    MockIntersectionObserver.intersect();
    await waitFor(() => {
      expect(client.getSnapshot().items).toHaveLength(5);
    });
    expect(client.getSnapshot().hasMore).toBe(false);
  });
});

describe('tabs', () => {
  const TABS = [
    { label: 'All' },
    {
      label: 'Billing',
      filter: (item: { category: string }) => item.category === 'billing.alerts',
    },
  ];

  test('renders the strip, filters the list, and counts unread per tab', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'billing.alerts', payload: { title: 'invoice' } });
    stub.addNotification({ category: 'system', payload: { title: 'maintenance' } });
    stub.addNotification({ category: 'system', payload: { title: 'seen one' }, read: true });
    await renderInbox(stub, { tabs: TABS });
    fireEvent.click(bell());

    const tabButtons = screen.getAllByRole('tab');
    expect(tabButtons.map((t) => t.textContent)).toEqual(['All2', 'Billing1']);
    expect(tabButtons[0]?.getAttribute('aria-selected')).toBe('true');
    expect(screen.getByText('maintenance')).toBeDefined();

    fireEvent.click(screen.getByRole('tab', { name: /Billing/ }));
    expect(screen.getByText('invoice')).toBeDefined();
    expect(screen.queryByText('maintenance')).toBeNull();
    expect(screen.getByRole('tab', { name: /Billing/ }).getAttribute('aria-selected')).toBe('true');
  });

  test('a sparse tab keeps fetching until its item appears', async () => {
    vi.stubGlobal('IntersectionObserver', MockIntersectionObserver);
    const stub = createStubServer();
    // Newest first: five non-matching items, then the single billing item.
    stub.addNotification({ category: 'billing.alerts', payload: { title: 'deep invoice' } });
    for (let i = 0; i < 5; i += 1) {
      stub.addNotification({ category: 'system' });
    }
    await renderInbox(stub, { tabs: TABS }, { pageSize: 2 });
    fireEvent.click(bell());

    fireEvent.click(screen.getByRole('tab', { name: /Billing/ }));
    MockIntersectionObserver.intersect();
    await waitFor(() => {
      expect(screen.getByText('deep invoice')).toBeDefined();
    });
  });

  test('a tab matching nothing drains the inbox then shows the empty state', async () => {
    vi.stubGlobal('IntersectionObserver', MockIntersectionObserver);
    const stub = createStubServer();
    for (let i = 0; i < 5; i += 1) {
      stub.addNotification({ category: 'system' });
    }
    const { client } = await renderInbox(
      stub,
      { tabs: [{ label: 'None', filter: () => false }] },
      { pageSize: 2 },
    );
    fireEvent.click(bell());

    // While pages remain the empty state stays hidden.
    expect(screen.queryByText('No notifications')).toBeNull();
    MockIntersectionObserver.intersect();
    await waitFor(() => {
      expect(client.getSnapshot().hasMore).toBe(false);
    });
    const requests = stub.requestsFor('/v1/inbox/items').length;
    expect(screen.getByText('No notifications')).toBeDefined();

    // Exhausted: no further requests are issued.
    MockIntersectionObserver.intersect();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(stub.requestsFor('/v1/inbox/items')).toHaveLength(requests);
  });

  test('without tabs there is no tablist', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub);
    fireEvent.click(bell());
    expect(screen.queryByRole('tablist')).toBeNull();
  });
});

describe('standalone mode', () => {
  test('connection props construct an owned client that closes on unmount', async () => {
    const stub = createStubServer({ requireHash: 'cafe' });
    stub.addNotification({ payload: { title: 'standalone works' } });
    vi.stubGlobal('fetch', stub.fetchFn);
    // A function constructor may return an object, which replaces `this`.
    function EventSourceStub(this: unknown, url: string) {
      return stub.createEventSource(url);
    }
    vi.stubGlobal('EventSource', EventSourceStub);

    const { unmount } = render(
      <Inbox
        serverUrl="https://chimely.test"
        environment={stub.environment}
        subscriberId={stub.subscriberId}
        subscriberHash="cafe"
        backoff={{ initialDelayMs: 5 }}
      />,
    );

    const streamUrl = new URL(stub.stream().url);
    expect(streamUrl.searchParams.get('environment')).toBe(stub.environment);
    expect(streamUrl.searchParams.get('subscriber_id')).toBe(stub.subscriberId);
    expect(streamUrl.searchParams.get('subscriber_hash')).toBe('cafe');

    stub.openStream();
    fireEvent.click(bell());
    await waitFor(() => {
      expect(screen.getByText('standalone works')).toBeDefined();
    });

    unmount();
    expect(stub.stream().closed).toBe(true);
  });
});
