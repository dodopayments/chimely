# @chimely/react

Drop-in `<Inbox />`, hooks, and composable components for
[Chimely](https://github.com/dodopayments/chimely), the open-source,
self-hostable in-app notification inbox.

```bash
npm install @chimely/react
```

Requires `react` and `react-dom` 18 or 19 as peer dependencies.

## The `<Inbox />`

```tsx
import { Inbox } from '@chimely/react';

<Inbox
  serverUrl="https://chimely.example.com"
  environment="production"
  subscriberId="usr_123"
  subscriberHash={subscriberHash}
/>
```

That is a live bell with an unseen badge, a popover inbox with unread state,
tabs, filters, archive, infinite scroll, per-category preferences, and SSE
live updates. `subscriberHash` is `hex(HMAC-SHA256(environment_secret,
subscriberId))`, computed by **your backend**, never in the browser. See
[Auth and the subscriber hash](https://chimely.dev/docs/auth).

## Hooks and composables

For custom UIs, skip the popover and compose:

- `ChimelyProvider` plus `useNotifications`, `useUnreadCount`,
  `useUnseenCount`, `usePreferences`
- `<Bell />`, `<InboxContent />`, `<Preferences />` as standalone components
- Render props (`renderItem`, `renderSubject`, `renderBody`, `renderAvatar`,
  `renderBell`, `renderFooter`) to replace one fragment while keeping the
  rest

## Theming

Plain CSS with custom properties. `appearance.variables` sets tokens
(`colorPrimary`, `colorBackground`, `shadow`, ...), `appearance.classNames`
and `appearance.styles` target each named slot, and a `darkTheme` preset
ships with the package:

```tsx
import { darkTheme } from '@chimely/react';

<Inbox appearance={{ variables: { ...darkTheme, colorPrimary: '#8b5cf6' } }} />
```

The full prop, slot, and localization tables are in the
[SDK reference](https://chimely.dev/docs/sdk-reference).

## Versioning

Pre-1.0: the component and hook surface may change on minor bumps. Pin your
versions.

## Links

- [Quickstart](https://chimely.dev/docs/quickstart) (five minutes, nothing to clone)
- [Documentation](https://chimely.dev/docs)
- [GitHub](https://github.com/dodopayments/chimely)

## License

MIT. The Chimely server is AGPL-3.0, which does not affect applications that
talk to it over HTTP through this SDK. See the
[License FAQ](https://github.com/dodopayments/chimely#license-faq).
