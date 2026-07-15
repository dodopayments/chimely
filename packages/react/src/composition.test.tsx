import type { ChimelyClient } from '@chimely/client';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { createRef } from 'react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { Bell } from './components/Bell';
import { InboxContent } from './components/InboxContent';
import { Preferences } from './components/Preferences';
import { ChimelyProvider } from './context';
import type { InboxProps } from './Inbox';
import { Inbox } from './Inbox';
import { navigation } from './navigation';
import type { StubServer } from './test-support/setup';
import { createStubServer, loadClient, makeClient } from './test-support/setup';

async function provided(
  stub: StubServer,
  children: (client: ChimelyClient) => ReactNode,
): Promise<ChimelyClient> {
  const client = makeClient(stub);
  await loadClient(client, stub);
  render(<ChimelyProvider client={client}>{children(client)}</ChimelyProvider>);
  return client;
}

async function renderInbox(stub: StubServer, props: InboxProps = {}): Promise<void> {
  await provided(stub, () => <Inbox {...props} />);
  fireEvent.click(screen.getByRole('button', { name: /^Notifications/ }));
}

afterEach(() => {
  vi.restoreAllMocks();
  document.querySelector('style[data-chimely]')?.remove();
});

describe('standalone InboxContent', () => {
  test('renders header, list, and preferences without a bell or popover', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'inline item' }, category: 'billing.alerts' });
    await provided(stub, () => (
      <InboxContent appearance={{ variables: { colorPrimary: '#123456' } }} />
    ));

    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.querySelector('.chimely-bell')).toBeNull();
    const content = document.querySelector('.chimely-content') as HTMLElement;
    expect(content.style.getPropertyValue('--chimely-colorPrimary')).toBe('#123456');
    expect(document.querySelector('.chimely-header-title')?.textContent).toBe('Notifications');
    expect(screen.getByText('inline item')).toBeDefined();

    fireEvent.click(screen.getByRole('button', { name: 'Notification preferences' }));
    expect(screen.getByRole('checkbox')).toBeDefined();
    fireEvent.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByText('inline item')).toBeDefined();
  });

  test('item clicks keep the default markRead + navigate wiring', async () => {
    const assign = vi.spyOn(navigation, 'assign').mockImplementation(() => {});
    const stub = createStubServer();
    const item = stub.addNotification({
      payload: { title: 'go', action_url: 'https://app.test/x' },
    });
    await provided(stub, () => <InboxContent />);

    fireEvent.click(screen.getByText('go'));
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)).toHaveLength(1);
    });
    expect(assign).toHaveBeenCalledWith('https://app.test/x');
  });
});

describe('standalone Bell', () => {
  test('shows the badge, forwards ref and clicks, never calls seen-all', async () => {
    const stub = createStubServer();
    stub.addNotification();
    stub.addNotification();
    const onClick = vi.fn();
    const ref = createRef<HTMLButtonElement>();
    await provided(stub, () => <Bell ref={ref} onClick={onClick} />);

    const button = screen.getByRole('button', { name: /^Notifications/ });
    expect(ref.current).toBe(button);
    expect(screen.getByText('2')).toBeDefined();

    fireEvent.click(button);
    expect(onClick).toHaveBeenCalledTimes(1);
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(stub.requestsFor('/v1/inbox/seen-all')).toHaveLength(0);
  });

  test('renderBell replaces the contents', async () => {
    const stub = createStubServer();
    stub.addNotification();
    await provided(stub, () => (
      <Bell
        open={false}
        renderBell={({ unseenCount, open }) => <i>{`${unseenCount}:${open}`}</i>}
      />
    ));
    expect(screen.getByText('1:false')).toBeDefined();
    expect(document.querySelector('.chimely-badge')).toBeNull();
  });
});

describe('standalone Preferences', () => {
  test('toggles a category', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'billing.alerts' });
    await provided(stub, () => (
      <Preferences localization={{ categoryLabels: { 'billing.alerts': 'Billing' } }} />
    ));

    expect(screen.getByText('Billing')).toBeDefined();
    fireEvent.click(screen.getByRole('checkbox'));
    await waitFor(() => {
      expect(stub.requestsFor('/v1/inbox/preferences').some((r) => r.method === 'PUT')).toBe(true);
    });
  });
});

describe('preferences grouping', () => {
  const rowLabels = (): Array<string | null> =>
    [...document.querySelectorAll('.chimely-preference span')].map((el) => el.textContent);

  test('preferencesFilter limits the shown categories', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'billing.invoice' });
    stub.addNotification({ category: 'security.alert' });
    await provided(stub, () => (
      <Preferences preferencesFilter={(category) => category.startsWith('billing')} />
    ));
    expect(rowLabels()).toEqual(['billing.invoice']);
  });

  test('preferencesSort orders the category rows', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'a.one' });
    stub.addNotification({ category: 'b.two' });
    await provided(stub, () => <Preferences preferencesSort={(a, b) => b.localeCompare(a)} />);
    expect(rowLabels()).toEqual(['b.two', 'a.one']);
  });

  test('preferenceGroups renders labeled groups with an ungrouped tail', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'billing.invoice' });
    stub.addNotification({ category: 'security.alert' });
    stub.addNotification({ category: 'social.mention' });
    await provided(stub, () => (
      <Preferences preferenceGroups={[{ label: 'Money', categories: ['billing.invoice'] }]} />
    ));
    expect(screen.getByText('Money')).toBeDefined();
    // Grouped first (group order), then the ungrouped tail (sorted).
    expect(rowLabels()).toEqual(['billing.invoice', 'security.alert', 'social.mention']);
  });

  test('a group with no visible category renders no heading', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'social.mention' });
    await provided(stub, () => (
      <Preferences preferenceGroups={[{ label: 'Money', categories: ['billing.invoice'] }]} />
    ));
    expect(screen.queryByText('Money')).toBeNull();
    expect(rowLabels()).toEqual(['social.mention']);
  });

  test('the props thread through <Inbox> to the panel', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'billing.invoice' });
    await renderInbox(stub, {
      preferenceGroups: [{ label: 'Money', categories: ['billing.invoice'] }],
    });
    fireEvent.click(screen.getByRole('button', { name: 'Notification preferences' }));
    expect(screen.getByText('Money')).toBeDefined();
  });
});

describe('granular render props', () => {
  test('renderSubject replaces only the subject', async () => {
    const assign = vi.spyOn(navigation, 'assign').mockImplementation(() => {});
    const stub = createStubServer();
    const item = stub.addNotification({
      payload: { title: 'plain title', body: 'default body', action_url: 'https://app.test/s' },
    });
    await renderInbox(stub, {
      renderSubject: ({ item: it }) => <mark data-testid="subject">{String(it.category)}</mark>,
    });

    expect(screen.queryByText('plain title')).toBeNull();
    expect(screen.getByTestId('subject')).toBeDefined();
    expect(screen.getByText('default body')).toBeDefined();

    fireEvent.click(screen.getByTestId('subject'));
    await waitFor(() => {
      expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)).toHaveLength(1);
    });
    expect(assign).toHaveBeenCalledWith('https://app.test/s');
  });

  test('renderBody replaces only the body', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'kept title', body: 'hidden body' } });
    await renderInbox(stub, {
      renderBody: () => <em data-testid="body">custom body</em>,
    });
    expect(screen.getByText('kept title')).toBeDefined();
    expect(screen.queryByText('hidden body')).toBeNull();
    expect(screen.getByTestId('body')).toBeDefined();
  });

  test('renderAvatar replaces the default icon', async () => {
    const stub = createStubServer();
    stub.addNotification({
      payload: { title: 'with avatar', icon_url: 'https://app.test/icon.png' },
    });
    await renderInbox(stub, {
      renderAvatar: () => <span data-testid="avatar">A</span>,
    });
    expect(document.querySelector('.chimely-item-icon')).toBeNull();
    expect(screen.getByTestId('avatar')).toBeDefined();
  });

  test('renderItem still overrides the granular render props', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'gone' } });
    await renderInbox(stub, {
      renderItem: () => <div data-testid="whole-item">everything</div>,
      renderSubject: () => <span data-testid="subject">unused</span>,
    });
    expect(screen.getByTestId('whole-item')).toBeDefined();
    expect(screen.queryByTestId('subject')).toBeNull();
  });
});
