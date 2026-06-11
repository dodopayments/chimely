import type { DronteClient } from '@dronte/client';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { DronteProvider } from './context';
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
): Promise<{ client: DronteClient; unmount: () => void }> {
  const client = makeClient(stub, clientConfig);
  await loadClient(client, stub);
  const { unmount } = render(
    <DronteProvider client={client}>
      <Inbox {...props} />
    </DronteProvider>,
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
  document.querySelector('style[data-dronte]')?.remove();
});

describe('bell and badge', () => {
  test('renders the bell with the unseen badge and injects the stylesheet once', async () => {
    const stub = createStubServer();
    stub.addNotification();
    stub.addNotification();
    await renderInbox(stub);

    expect(bell()).toBeDefined();
    expect(screen.getByText('2')).toBeDefined();
    expect(document.querySelectorAll('style[data-dronte]')).toHaveLength(1);
  });

  test('no badge when nothing is unseen', async () => {
    const stub = createStubServer();
    stub.addNotification({ seen: true });
    await renderInbox(stub);
    expect(document.querySelector('.dronte-badge')).toBeNull();
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
    expect(document.querySelector('.dronte-badge')).toBeNull();
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
    expect(document.querySelector('.dronte-badge')).toBeNull();
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

    const titles = [...document.querySelectorAll('.dronte-item-title')].map(
      (node) => node.textContent,
    );
    expect(titles).toEqual(['newer', 'older']);
    const unreadItems = document.querySelectorAll('.dronte-item-unread');
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
  test('variables land on the root as --dronte-* custom properties, forwarded verbatim', async () => {
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
    const root = document.querySelector('.dronte-root') as HTMLElement;
    expect(root.style.getPropertyValue('--dronte-colorPrimary')).toBe('#ff0000');
    expect(root.style.getPropertyValue('--dronte-fontSize')).toBe('16px');
    expect(root.style.getPropertyValue('--dronte-customThing')).toBe('4px');
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
    expect(document.querySelector('.dronte-root.my-root')).not.toBeNull();
    expect(document.querySelector('.dronte-bell.my-bell')).not.toBeNull();
    expect(document.querySelector('.dronte-badge.my-badge')).not.toBeNull();

    fireEvent.click(bell());
    expect(document.querySelector('.dronte-popover.my-popover')).not.toBeNull();
    expect(document.querySelector('.dronte-item.my-item')).not.toBeNull();
    expect(document.querySelector('.dronte-item-unread.my-unread')).not.toBeNull();
  });
});

describe('infinite scroll', () => {
  test('the end sentinel pages in more items via fetchMore', async () => {
    vi.stubGlobal('IntersectionObserver', MockIntersectionObserver);
    const stub = createStubServer();
    for (let i = 0; i < 5; i += 1) {
      stub.addNotification();
    }
    const { client } = await renderInbox(stub, {}, { pageSize: 2 });
    fireEvent.click(bell());
    expect(client.getSnapshot().items).toHaveLength(2);
    expect(MockIntersectionObserver.instances.length).toBeGreaterThan(0);

    MockIntersectionObserver.intersect();
    await waitFor(() => {
      expect(client.getSnapshot().items).toHaveLength(4);
    });

    MockIntersectionObserver.intersect();
    await waitFor(() => {
      expect(client.getSnapshot().items).toHaveLength(5);
    });
    expect(client.getSnapshot().hasMore).toBe(false);
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
        serverUrl="https://dronte.test"
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
