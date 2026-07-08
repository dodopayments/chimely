import type { ChimelyClient } from '@chimely/client';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { darkTheme } from './appearance';
import { ChimelyProvider } from './context';
import type { InboxProps } from './Inbox';
import { Inbox } from './Inbox';
import type { StubServer } from './test-support/setup';
import { createStubServer, loadClient, makeClient } from './test-support/setup';

async function renderInbox(
  stub: StubServer,
  props: InboxProps = {},
  clientConfig: { pageSize?: number } = {},
): Promise<ChimelyClient> {
  const client = makeClient(stub, clientConfig);
  await loadClient(client, stub);
  render(
    <ChimelyProvider client={client}>
      <Inbox {...props} />
    </ChimelyProvider>,
  );
  return client;
}

function bell(): HTMLElement {
  return screen.getByRole('button', { name: /^Notifications/ });
}

afterEach(() => {
  vi.restoreAllMocks();
  document.querySelector('style[data-chimely]')?.remove();
});

describe('appearance.styles', () => {
  test('slot styles land inline alongside the default classes', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'styled' } });
    await renderInbox(stub, {
      appearance: {
        classNames: { item: 'my-item' },
        styles: {
          item: { backgroundColor: 'rgb(1, 2, 3)' },
          header: { paddingTop: '20px' },
        },
      },
    });
    fireEvent.click(bell());

    const item = document.querySelector('.chimely-item.my-item') as HTMLElement;
    expect(item.style.backgroundColor).toBe('rgb(1, 2, 3)');
    const header = document.querySelector('.chimely-header') as HTMLElement;
    expect(header.style.paddingTop).toBe('20px');
  });

  test('itemUnread styles merge over item styles for unread items', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'unread' } });
    stub.addNotification({ payload: { title: 'read' }, read: true });
    await renderInbox(stub, {
      appearance: {
        styles: {
          item: { color: 'rgb(10, 10, 10)' },
          itemUnread: { color: 'rgb(200, 0, 0)' },
        },
      },
    });
    fireEvent.click(bell());

    const unread = document.querySelector('.chimely-item-unread') as HTMLElement;
    expect(unread.style.color).toBe('rgb(200, 0, 0)');
    const all = [...document.querySelectorAll('.chimely-item')] as HTMLElement[];
    const read = all.find((el) => !el.classList.contains('chimely-item-unread'));
    expect(read?.style.color).toBe('rgb(10, 10, 10)');
  });
});

describe('appearance.icons', () => {
  test('icons.bell and icons.gear replace the built-in SVGs', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub, {
      appearance: {
        icons: {
          bell: () => <span data-testid="my-bell-icon" />,
          gear: () => <span data-testid="my-gear-icon" />,
        },
      },
    });

    expect(screen.getByTestId('my-bell-icon')).toBeDefined();
    expect(document.querySelector('.chimely-bell svg')).toBeNull();

    fireEvent.click(bell());
    expect(screen.getByTestId('my-gear-icon')).toBeDefined();
  });

  test('renderBell wins over icons.bell', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub, {
      appearance: { icons: { bell: () => <span data-testid="icon-bell" /> } },
      renderBell: () => <span data-testid="render-bell" />,
    });
    expect(screen.getByTestId('render-bell')).toBeDefined();
    expect(screen.queryByTestId('icon-bell')).toBeNull();
  });
});

describe('darkTheme', () => {
  test('spreads into variables and lands as custom properties', async () => {
    const stub = createStubServer();
    await renderInbox(stub, {
      appearance: { variables: { ...darkTheme, colorPrimary: '#123123' } },
    });
    const root = document.querySelector('.chimely-root') as HTMLElement;
    expect(root.style.getPropertyValue('--chimely-colorBackground')).toBe('#111827');
    expect(root.style.getPropertyValue('--chimely-colorBadgeForeground')).toBe('#0b1220');
    expect(root.style.getPropertyValue('--chimely-colorPrimary')).toBe('#123123');
  });
});

describe('new-notification pill', () => {
  test('appears for arrivals while scrolled down, clears on click', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 6; i += 1) {
      stub.addNotification();
    }
    await renderInbox(stub, {
      localization: { newNotifications: (count) => `${count} fresh` },
    });
    fireEvent.click(bell());

    const list = document.querySelector('.chimely-list') as HTMLElement;
    list.scrollTop = 200;

    stub.addNotification({ payload: { title: 'late arrival' } });
    stub.emitHint();
    const pill = await screen.findByRole('button', { name: '1 fresh' });
    expect(pill.classList.contains('chimely-pill')).toBe(true);

    fireEvent.click(pill);
    await waitFor(() => {
      expect(screen.queryByRole('button', { name: '1 fresh' })).toBeNull();
    });
    expect(list.scrollTop).toBe(0);
  });

  test('does not appear when the list is at the top', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 3; i += 1) {
      stub.addNotification();
    }
    await renderInbox(stub);
    fireEvent.click(bell());

    stub.addNotification({ payload: { title: 'visible immediately' } });
    stub.emitHint();
    await screen.findByText('visible immediately');
    expect(document.querySelector('.chimely-pill')).toBeNull();
  });

  test('prunes ids evicted by a non-contiguous reset', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 3; i += 1) {
      stub.addNotification();
    }
    await renderInbox(
      stub,
      { localization: { newNotifications: (count) => `${count} fresh` } },
      { pageSize: 3 },
    );
    fireEvent.click(bell());

    const list = document.querySelector('.chimely-list') as HTMLElement;
    list.scrollTop = 200;

    stub.addNotification({ payload: { title: 'kept behind the pill' } });
    stub.emitHint();
    await screen.findByRole('button', { name: '1 fresh' });

    // More than a page of arrivals with zero overlap resets the list
    // wholesale, evicting the earlier arrival. The pill counts only the
    // ids still present.
    for (let i = 0; i < 4; i += 1) {
      stub.addNotification();
    }
    stub.emitHint();
    await screen.findByRole('button', { name: '3 fresh' });
  });

  test('arrivals outside the active tab do not bump the pill', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 5; i += 1) {
      stub.addNotification({ category: 'billing.alerts' });
    }
    const client = await renderInbox(stub, {
      tabs: [
        { label: 'All' },
        { label: 'Billing', filter: (item) => item.category === 'billing.alerts' },
      ],
      localization: { newNotifications: (count) => `${count} fresh` },
    });
    fireEvent.click(bell());
    fireEvent.click(screen.getByRole('tab', { name: /Billing/ }));

    const list = document.querySelector('.chimely-list') as HTMLElement;
    list.scrollTop = 200;

    stub.addNotification({ category: 'system' });
    stub.emitHint();
    await waitFor(() => {
      expect(client.getSnapshot().items).toHaveLength(6);
    });
    expect(document.querySelector('.chimely-pill')).toBeNull();

    stub.addNotification({ category: 'billing.alerts' });
    stub.emitHint();
    await screen.findByRole('button', { name: '1 fresh' });
  });
});
