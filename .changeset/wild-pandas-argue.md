---
"@chimely/client": minor
---

`InboxSnapshot` gains `lastRefreshNewItemIds`: the ids the last first-page merge added that were not already loaded, with fresh array identity per merge. 304 refreshes and fetchMore leave it untouched. Powers the new-notification indicator in @chimely/react.
