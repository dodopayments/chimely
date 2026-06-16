/**
 * Plain CSS, injected once on first <Inbox /> mount. Theming happens through
 * the --dronte-* custom properties (set via InboxAppearance.variables) and
 * the per-slot class hooks. No styling dependency, no Tailwind.
 */
export const INBOX_CSS = `
.dronte-root {
  position: relative;
  display: inline-block;
  font-family: var(--dronte-fontFamily, system-ui, -apple-system, sans-serif);
  font-size: var(--dronte-fontSize, 14px);
  color: var(--dronte-colorForeground, #111827);
}
.dronte-bell {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 36px;
  height: 36px;
  padding: 0;
  border: none;
  border-radius: var(--dronte-borderRadius, 8px);
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.dronte-bell:hover {
  background: var(--dronte-colorMuted, #f3f4f6);
}
.dronte-badge {
  position: absolute;
  top: 2px;
  right: 2px;
  min-width: 16px;
  height: 16px;
  padding: 0 4px;
  border-radius: 999px;
  background: var(--dronte-colorBadge, #1264FF);
  color: #ffffff;
  font-size: 11px;
  font-weight: 600;
  line-height: 16px;
  text-align: center;
}
.dronte-popover {
  position: absolute;
  top: 0;
  left: 0;
  display: flex;
  flex-direction: column;
  width: 360px;
  max-height: 480px;
  overflow: hidden;
  background: var(--dronte-colorBackground, #ffffff);
  border: 1px solid var(--dronte-colorMuted, #e5e7eb);
  border-radius: var(--dronte-borderRadius, 8px);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
  z-index: 1000;
}
.dronte-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 10px 14px;
  border-bottom: 1px solid var(--dronte-colorMuted, #e5e7eb);
}
.dronte-header-title {
  font-weight: 600;
}
.dronte-header-action {
  border: none;
  background: transparent;
  color: var(--dronte-colorPrimary, #1264FF);
  font: inherit;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: var(--dronte-borderRadius, 8px);
}
.dronte-header-action:hover {
  background: var(--dronte-colorMuted, #f3f4f6);
  color: var(--dronte-colorPrimaryHover, #004F32);
}
.dronte-bell:focus-visible,
.dronte-header-action:focus-visible,
.dronte-item:focus-visible {
  outline: 2px solid var(--dronte-colorPrimary, #1264FF);
  outline-offset: 2px;
}
.dronte-list {
  flex: 1;
  overflow-y: auto;
  margin: 0;
  padding: 0;
  list-style: none;
}
.dronte-sentinel {
  height: 1px;
}
.dronte-item {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  width: 100%;
  padding: 12px 14px;
  border: none;
  border-bottom: 1px solid var(--dronte-colorMuted, #f3f4f6);
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
}
.dronte-item:hover {
  background: var(--dronte-colorMuted, #f9fafb);
}
.dronte-item-icon {
  width: 28px;
  height: 28px;
  border-radius: 50%;
  flex: none;
}
.dronte-item-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.dronte-item-title {
  font-weight: 500;
}
.dronte-item-unread .dronte-item-title {
  font-weight: 700;
}
.dronte-item-body {
  color: var(--dronte-colorForeground, #374151);
  opacity: 0.8;
}
.dronte-item-time {
  font-size: 0.85em;
  opacity: 0.6;
}
.dronte-item-dot {
  width: 8px;
  height: 8px;
  margin-top: 6px;
  margin-left: auto;
  border-radius: 50%;
  background: var(--dronte-colorPrimary, #1264FF);
  flex: none;
}
.dronte-empty {
  padding: 32px 16px;
  text-align: center;
}
.dronte-empty-title {
  margin: 0 0 4px;
  font-weight: 600;
}
.dronte-empty-body {
  margin: 0;
  opacity: 0.7;
}
.dronte-footer {
  flex: none;
  border-top: 1px solid var(--dronte-colorMuted, #f3f4f6);
  min-height: 4px;
}
.dronte-preferences {
  flex: 1;
  overflow-y: auto;
  padding: 8px 0;
}
.dronte-preference {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 10px 14px;
}
.dronte-preference input {
  accent-color: var(--dronte-colorPrimary, #1264FF);
}
`;

let injected = false;

/** Idempotent, SSR-safe. Called on <Inbox /> mount, never at import time. */
export function ensureStyles(): void {
  if (injected || typeof document === 'undefined') {
    return;
  }
  if (document.querySelector('style[data-dronte]')) {
    injected = true;
    return;
  }
  const element = document.createElement('style');
  element.setAttribute('data-dronte', '');
  element.textContent = INBOX_CSS;
  document.head.appendChild(element);
  injected = true;
}
