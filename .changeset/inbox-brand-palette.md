---
'@dronte/react': minor
---

Default `<Inbox />` theme now ships the Dronte brand palette: primary actions,
links, the unread badge, the unread dot, and focus rings use the accent
`#1264FF`; hover/section accents use the deep accent `#004F32`. Adds a
`colorPrimaryHover` appearance variable and accent focus-visible outlines.
All values remain overridable through the existing `--dronte-*` custom
properties — no new styling dependency.
