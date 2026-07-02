/**
 * Plain CSS, injected once on first <Inbox /> mount. Theming happens through
 * the --chimely-* custom properties (set via InboxAppearance.variables) and
 * the per-slot class hooks. No styling dependency, no Tailwind.
 */
export const INBOX_CSS = `
.chimely-root {
  position: relative;
  display: inline-block;
  font-family: var(--chimely-fontFamily, system-ui, -apple-system, sans-serif);
  font-size: var(--chimely-fontSize, 14px);
  color: var(--chimely-colorForeground, #111827);
}
.chimely-bell {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 36px;
  height: 36px;
  padding: 0;
  border: none;
  border-radius: var(--chimely-borderRadius, 8px);
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.chimely-bell:hover {
  background: var(--chimely-colorMuted, #f3f4f6);
}
.chimely-badge {
  position: absolute;
  top: 2px;
  right: 2px;
  min-width: 16px;
  height: 16px;
  padding: 0 4px;
  border-radius: 999px;
  background: var(--chimely-colorBadge, #1264FF);
  color: var(--chimely-colorBadgeForeground, #ffffff);
  font-size: 11px;
  font-weight: 600;
  line-height: 16px;
  text-align: center;
}
.chimely-popover {
  position: absolute;
  top: 0;
  left: 0;
  display: flex;
  flex-direction: column;
  width: 360px;
  max-height: 480px;
  overflow: hidden;
  background: var(--chimely-colorBackground, #ffffff);
  border: 1px solid var(--chimely-colorMuted, #e5e7eb);
  border-radius: var(--chimely-borderRadius, 8px);
  box-shadow: var(--chimely-shadow, 0 8px 24px rgba(0, 0, 0, 0.12));
  z-index: 1000;
}
.chimely-popover-portal {
  position: fixed;
}
.chimely-content {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  font-family: var(--chimely-fontFamily, system-ui, -apple-system, sans-serif);
  font-size: var(--chimely-fontSize, 14px);
  color: var(--chimely-colorForeground, #111827);
}
.chimely-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 10px 14px;
  border-bottom: 1px solid var(--chimely-colorMuted, #e5e7eb);
}
.chimely-header-title {
  font-weight: 600;
}
.chimely-header-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  margin-left: auto;
}
.chimely-filter {
  border: none;
  background: transparent;
  color: var(--chimely-colorPrimary, #1264FF);
  font: inherit;
  padding: 2px 4px;
  border-radius: var(--chimely-borderRadius, 8px);
  cursor: pointer;
}
.chimely-header-action {
  border: none;
  background: transparent;
  color: var(--chimely-colorPrimary, #1264FF);
  font: inherit;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: var(--chimely-borderRadius, 8px);
}
.chimely-header-action:hover {
  background: var(--chimely-colorMuted, #f3f4f6);
  color: var(--chimely-colorPrimaryHover, #004F32);
}
.chimely-bell:focus-visible,
.chimely-header-action:focus-visible,
.chimely-item:focus-visible {
  outline: 2px solid var(--chimely-colorPrimary, #1264FF);
  outline-offset: 2px;
}
.chimely-tabs {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 6px 10px;
  border-bottom: 1px solid var(--chimely-colorMuted, #e5e7eb);
  overflow-x: auto;
}
.chimely-tab {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 8px;
  border: none;
  border-radius: var(--chimely-borderRadius, 8px);
  background: transparent;
  color: inherit;
  font: inherit;
  cursor: pointer;
  white-space: nowrap;
}
.chimely-tab:hover {
  background: var(--chimely-colorMuted, #f3f4f6);
}
.chimely-tab-active {
  color: var(--chimely-colorPrimary, #1264FF);
  box-shadow: inset 0 -2px 0 var(--chimely-colorPrimary, #1264FF);
}
.chimely-tab-count {
  min-width: 16px;
  height: 16px;
  padding: 0 4px;
  border-radius: 999px;
  background: var(--chimely-colorBadge, #1264FF);
  color: var(--chimely-colorBadgeForeground, #ffffff);
  font-size: 11px;
  font-weight: 600;
  line-height: 16px;
  text-align: center;
}
.chimely-list-container {
  position: relative;
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
}
.chimely-pill {
  position: absolute;
  top: 8px;
  left: 50%;
  transform: translateX(-50%);
  padding: 4px 12px;
  border: none;
  border-radius: 999px;
  background: var(--chimely-colorPrimary, #1264FF);
  color: var(--chimely-colorBadgeForeground, #ffffff);
  font: inherit;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  box-shadow: var(--chimely-shadow, 0 8px 24px rgba(0, 0, 0, 0.12));
  z-index: 1;
}
.chimely-list {
  flex: 1;
  overflow-y: auto;
  margin: 0;
  padding: 0;
  list-style: none;
}
.chimely-sentinel {
  height: 1px;
}
.chimely-list-row {
  position: relative;
}
.chimely-item-actions {
  position: absolute;
  top: 8px;
  right: 10px;
  display: flex;
  gap: 4px;
  opacity: 0;
  pointer-events: none;
}
.chimely-list-row:hover .chimely-item-actions,
.chimely-list-row:focus-within .chimely-item-actions {
  opacity: 1;
  pointer-events: auto;
}
.chimely-item-action {
  width: 24px;
  height: 24px;
  padding: 0;
  border: 1px solid var(--chimely-colorMuted, #e5e7eb);
  border-radius: var(--chimely-borderRadius, 8px);
  background: var(--chimely-colorBackground, #ffffff);
  color: var(--chimely-colorPrimary, #1264FF);
  font-size: 12px;
  line-height: 1;
  cursor: pointer;
}
.chimely-item-action:hover {
  background: var(--chimely-colorMuted, #f3f4f6);
}
.chimely-item {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  width: 100%;
  padding: 12px 14px;
  border: none;
  border-bottom: 1px solid var(--chimely-colorMuted, #f3f4f6);
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
}
.chimely-item:hover {
  background: var(--chimely-colorMuted, #f9fafb);
}
.chimely-item-icon {
  width: 28px;
  height: 28px;
  border-radius: 50%;
  flex: none;
}
.chimely-item-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.chimely-item-title {
  font-weight: 500;
}
.chimely-item-unread .chimely-item-title {
  font-weight: 700;
}
.chimely-item-body {
  color: var(--chimely-colorForeground, #374151);
  opacity: 0.8;
}
.chimely-item-time {
  font-size: 0.85em;
  opacity: 0.6;
}
.chimely-item-dot {
  width: 8px;
  height: 8px;
  margin-top: 6px;
  margin-left: auto;
  border-radius: 50%;
  background: var(--chimely-colorPrimary, #1264FF);
  flex: none;
}
.chimely-empty {
  padding: 32px 16px;
  text-align: center;
}
.chimely-empty-title {
  margin: 0 0 4px;
  font-weight: 600;
}
.chimely-empty-body {
  margin: 0;
  opacity: 0.7;
}
.chimely-footer {
  flex: none;
  border-top: 1px solid var(--chimely-colorMuted, #f3f4f6);
  min-height: 4px;
}
.chimely-footer:not(:empty) {
  padding: 8px 14px;
}
.chimely-preferences {
  flex: 1;
  overflow-y: auto;
  padding: 8px 0;
}
.chimely-preference {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 10px 14px;
}
.chimely-preference input {
  accent-color: var(--chimely-colorPrimary, #1264FF);
}
`;

let injected = false;

/** Idempotent, SSR-safe. Called on <Inbox /> mount, never at import time. */
export function ensureStyles(): void {
  if (injected || typeof document === 'undefined') {
    return;
  }
  if (document.querySelector('style[data-chimely]')) {
    injected = true;
    return;
  }
  const element = document.createElement('style');
  element.setAttribute('data-chimely', '');
  element.textContent = INBOX_CSS;
  document.head.appendChild(element);
  injected = true;
}
