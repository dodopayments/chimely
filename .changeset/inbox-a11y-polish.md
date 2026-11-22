---
"@chimely/react": patch
---

Inbox accessibility and robustness polish from the hostile-frontend review: move focus into the popover dialog on open (APG dialog pattern) so keyboard users reach a portaled popover directly, add arrow-key/Home/End navigation to the more-actions menu (honoring its `role=menu` semantics), cap the popover width at the viewport with `min(360px, calc(100vw - 16px))`, and re-probe the DOM in `ensureStyles` each call so head-replacing navigation (Turbo/PJAX) cannot suppress style re-injection.
