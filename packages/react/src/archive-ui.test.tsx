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

describe('archive row action', () => {
  test('archiving removes the item from the default view', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'stays' } });
    // Newest first: this row renders at index 0.
    const item = stub.addNotification({ payload: { title: 'to archive' } });
    await renderInbox(stub);

    fireEvent.click(screen.getAllByRole('button', { name: 'Archive' })[0] as HTMLElement);
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/archive`)).toHaveLength(1);
    });
    expect(screen.queryByText('to archive')).toBeNull();
    expect(screen.getByText('stays')).toBeDefined();
  });

  test('the archived view offers unarchive and localizes', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'boxed' }, archived: true });
    await renderInbox(stub, {
      localization: { unarchiveAction: 'Wiederherstellen' },
    });

    fireEvent.change(screen.getByRole('combobox', { name: 'View' }), {
      target: { value: 'archived' },
    });
    await waitFor(() => {
      expect(screen.getByText('boxed')).toBeDefined();
    });
    const action = screen.getByRole('button', { name: 'Wiederherstellen' });
    fireEvent.click(action);
    await waitFor(() => {
      expect(screen.queryByText('boxed')).toBeNull();
    });
  });
});

describe('more-actions menu', () => {
  test('archive-all empties the list from the menu', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'gone soon' } });
    await renderInbox(stub);

    fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Archive all' }));
    await waitFor(() => {
      expect(stub.requestsFor('/v1/inbox/archive-all')).toHaveLength(1);
    });
    expect(screen.queryByText('gone soon')).toBeNull();
    expect(screen.queryByRole('menu')).toBeNull();
  });

  test('archive-read fires the async endpoint', async () => {
    const stub = createStubServer();
    stub.addNotification({ read: true });
    stub.addNotification();
    await renderInbox(stub);

    fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Archive read' }));
    await waitFor(() => {
      expect(stub.requestsFor('/v1/inbox/archive-read')).toHaveLength(1);
    });
  });

  test('outside pointerdown closes the menu', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub);

    fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
    expect(screen.getByRole('menu')).toBeDefined();
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole('menu')).toBeNull();
  });

  test('opens with the first item focused, arrows wrap, Home/End jump', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub);

    fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
    const items = screen.getAllByRole('menuitem');
    expect(items).toHaveLength(3);
    expect(document.activeElement).toBe(items[0]);

    fireEvent.keyDown(items[0] as HTMLElement, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items[1]);
    fireEvent.keyDown(items[1] as HTMLElement, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items[2]);
    fireEvent.keyDown(items[2] as HTMLElement, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items[0]);

    fireEvent.keyDown(items[0] as HTMLElement, { key: 'ArrowUp' });
    expect(document.activeElement).toBe(items[2]);

    fireEvent.keyDown(items[2] as HTMLElement, { key: 'Home' });
    expect(document.activeElement).toBe(items[0]);
    fireEvent.keyDown(items[0] as HTMLElement, { key: 'End' });
    expect(document.activeElement).toBe(items[2]);
  });

  test('escape closes only the menu and restores trigger focus', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await renderInbox(stub);

    const trigger = screen.getByRole('button', { name: 'More actions' });
    fireEvent.click(trigger);
    expect(screen.getByRole('menu')).toBeDefined();

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(screen.getByRole('dialog')).toBeDefined();
    expect(document.activeElement).toBe(trigger);

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});
