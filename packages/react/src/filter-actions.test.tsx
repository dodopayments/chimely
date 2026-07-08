import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { ChimelyProvider } from './context';
import type { InboxProps } from './Inbox';
import { Inbox } from './Inbox';
import type { StubServer } from './test-support/setup';
import { createStubServer, loadClient, makeClient } from './test-support/setup';

async function renderInbox(stub: StubServer, props: InboxProps = {}): Promise<void> {
  const client = makeClient(stub);
  await loadClient(client, stub);
  render(
    <ChimelyProvider client={client}>
      <Inbox {...props} />
    </ChimelyProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: /^Notifications/ }));
}

afterEach(() => {
  vi.restoreAllMocks();
  document.querySelector('style[data-chimely]')?.remove();
});

describe('per-item read/unread actions', () => {
  test('a read item offers mark-unread and flips optimistically', async () => {
    const stub = createStubServer();
    const item = stub.addNotification({ payload: { title: 'was read' }, read: true });
    await renderInbox(stub);

    const action = screen.getByRole('button', { name: 'Mark as unread' });
    fireEvent.click(action);
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/unread`)).toHaveLength(1);
    });
    // The flip is optimistic and the action relabels.
    expect(document.querySelector('.chimely-item-unread')).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Mark as read' })).toBeDefined();
  });

  test('an unread item offers mark-read without navigating', async () => {
    const stub = createStubServer();
    const item = stub.addNotification({
      payload: { title: 'unread', action_url: 'https://app.test/x' },
    });
    await renderInbox(stub);

    fireEvent.click(screen.getByRole('button', { name: 'Mark as read' }));
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)).toHaveLength(1);
    });
    expect(document.querySelector('.chimely-item-unread')).toBeNull();
  });

  test('action tooltips localize', async () => {
    const stub = createStubServer();
    stub.addNotification({ read: true });
    await renderInbox(stub, {
      localization: { markUnreadAction: 'Als ungelesen markieren' },
    });
    expect(screen.getByRole('button', { name: 'Als ungelesen markieren' })).toBeDefined();
  });
});

describe('filter view select', () => {
  test('switching to Unread refetches with the filter and narrows the list', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'read one' }, read: true });
    stub.addNotification({ payload: { title: 'unread one' } });
    await renderInbox(stub);
    expect(screen.getByText('read one')).toBeDefined();

    fireEvent.change(screen.getByRole('combobox', { name: 'View' }), {
      target: { value: 'unread' },
    });
    await waitFor(() => {
      expect(screen.queryByText('read one')).toBeNull();
    });
    expect(screen.getByText('unread one')).toBeDefined();
    const filtered = stub
      .requestsFor('/v1/inbox/items')
      .filter((r) => r.search.get('filter') === 'unread');
    expect(filtered.length).toBeGreaterThan(0);

    fireEvent.change(screen.getByRole('combobox', { name: 'View' }), {
      target: { value: 'default' },
    });
    await waitFor(() => {
      expect(screen.getByText('read one')).toBeDefined();
    });
  });

  test('the select localizes', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub, {
      localization: {
        filterLabel: 'Ansicht',
        filterInbox: 'Alle',
        filterUnread: 'Ungelesen',
        filterArchived: 'Archiviert',
      },
    });
    const select = screen.getByRole('combobox', { name: 'Ansicht' });
    expect([...select.querySelectorAll('option')].map((o) => o.textContent)).toEqual([
      'Alle',
      'Ungelesen',
      'Archiviert',
    ]);
  });
});
