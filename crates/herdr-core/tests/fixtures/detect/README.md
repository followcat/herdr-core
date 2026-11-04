# Detect fixture provenance

All fixtures in this directory are imported from
[wherdr](https://github.com/afloury/wherdr), revision `d6f81665` on the
followcat/wherdr branch `mobile`. wherdr is MIT-licensed (Copyright (c) 2026
Antoine Floury); its license and a scope note live in
`distribution/notices/wherdr.txt` and are folded into
`crates/herdr-core/THIRD-PARTY-NOTICES` by `scripts/notices`.

| Fixture here | Source within wherdr | Conversion |
|---|---|---|
| `claude-{approve-bash,bash-starting,bash-running,bash-finished,done,idle,working}.txt` | same names under `tests/fixtures/` | byte-identical |
| `cmd-{composer,draft,slash-menu,trust-dialog}.txt` | same names under `tests/fixtures/` | byte-identical |
| `codex-{approve-command,approve-edits,idle,trust}.txt` | same names under `tests/fixtures/` | byte-identical |
| `devin-guide-prompt.txt` | `tests/fixtures/devin-idle.txt` | byte-identical |
| `devin-welcome.txt` | `tests/captures/devin-idle.ansi` | ANSI escape sequences removed, CRLF normalized, environment and path text neutralized |
| `devin-draft-box.txt` | `tests/captures/devin-draft.ansi` | ANSI escape sequences removed, CRLF normalized, environment and path text neutralized |
| `opencode-permission-{edit,narrow,shell}.txt` | same names under `tests/captures/` | byte-identical |

Neutralization on the two `.ansi` conversions replaced real user/host names,
env values and absolute paths with neutral stand-ins; everything else is the
captured screen text. Keep it that way: no real names, paths or hosts.
