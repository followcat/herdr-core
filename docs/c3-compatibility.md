# C3 compatibility and consumer boundaries

Protocol generation stays 1. The released schema is unchanged; all new facts and attach fields
are optional. The compact CLI list keeps its established projection. Full terminal RPCs,
subscriptions and event patches are the fact API.

| Client / path | Facts and behavior | Regression |
|---|---|---|
| Generation-1 client → fork core | Ignore optional hostname/repository/activity; DATA remains human activity; no unsolicited new stream kinds | `c3_compat`, released-schema `compat`, existing m0–m7 |
| Fork client → older core | Missing hostname/facts/feature/attach confirmation means unknown or existing DATA fallback | `c3_compat`, quiet negotiation matrix, identity refresh tests |
| Fork → fork relays → fork | Preserve origin core/epoch/scope/keys and complete patches; quiet release requires every hop | `c3_compat`, `repository_publish`, `quiet_release_relay` |
| Fork → old hop → fork | Remove unsupported incidental facts from records/events and immediately on downgraded hello, before inventory; quiet confirmation false; never retag kind 7 | remote compatibility and quiet negotiation tests |
| Capable stream reconnects over a downgraded path | Close before mapping resumed output; reattach to negotiate fallback | quiet/resize pre-mapping regressions |
| Core restarts or crash-replays | New activity epoch, saved counter/guards, no reconstructed prehistory; repository facts revalidate | `activity`, activity guard tests, existing C2 recovery suite |

`repository_facts` publishes machine-side cached identity/labels/branch/local divergence and a
freshness state. `quiet_human_release` changes input ownership only when requested and confirmed.
`record_activity` supplies chronology only. `os_hostname` supplies an optional observed OS hostname, distinct
from the configured display name. Unknown counts are null, not zero. Windows intentionally
omits Git children/counts until resource containment exists there.

Appearance is outside C3: no Ghostty pin/parser/paint changes, tracker, snapshot envelope or reset
parameter. hc-tui's downstream LRU attached panes can preserve routine switching; first attach
and reconnect appearance remain limitations. Activity has no acknowledgement API, state labels,
session token or ordered output fences. Clients acknowledge locally; inventory ahead of consumed
output can acknowledge too early. Same-process session replacement without an observed owner/app
boundary is unclaimed. Labels remain uninterpreted.

The matrix uses isolated scratch cores and framed protocol fixtures, including old-feature peers.
It establishes protocol compatibility, not rendering acceptance in hc-tui, wherdr or herdr-app.
Those consumers still need their own application-level acceptance checks. Optional legacy resize
tests additionally accept `HC_OLD_CORE` for an actual released binary; without it they stay ignored.
