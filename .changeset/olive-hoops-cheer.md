---
"@chimely/react": minor
---

Composable components: `<InboxContent />` (the popover interior for custom popovers, drawers, and full-page inboxes), `<Bell />` (forwardRef trigger with the unseen badge), and `<Preferences />` are now exported. Granular render props `renderSubject`, `renderBody`, and `renderAvatar` replace one fragment of the default item while keeping layout and click wiring. The popover interior is now wrapped in a `.chimely-content` div (new `content` slot).
