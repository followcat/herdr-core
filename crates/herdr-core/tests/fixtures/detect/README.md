# Detect fixture provenance

The original fixtures in this directory are imported from
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

## Command Code live validation (2026-11-05)

The following full `hc read` screens come from the local C2 scratch-session
validation of Command Code 1.88.2–1.88.3, rather than wherdr:

| Fixture | Source capture | Expected |
|---|---|---|
| `cmd2-turn-1.txt` | `C2-cmd-caps/cmd2-turn-1.txt` | working: Harmonizing footer |
| `cmd-after-yes-5.txt` | `C2-cmd-caps/cmd-after-yes-5.txt` | working: Pondering footer plus Tip |
| `cmd2-run-8.txt` | original live captures, `cmd2-run-8.txt` | working: Shell command allowed footer plus Tip |
| `cmd-tool-permission.txt` | `C2-cmd-caps/cmd-tool-permission.txt` | blocked / permission |
| `cmd2-turn-11.txt` | `C2-cmd-caps/cmd2-turn-11.txt` | blocked / permission |
| `cmd2-run-10.txt` | `C2-cmd-caps/cmd2-run-10.txt` | blocked / permission: retry dialog |
| `cmd-after-yes-15.txt` | `C2-cmd-caps/cmd-after-yes-15.txt` | idle: DONE and Worked summary, no turn footer |

The brief's `cmd2-run-10.txt` label says working, but that supplied file is
actually a permission dialog. The original `cmd2-run-8.txt` supplies the
stated working footer; neither screen was rewritten to fit the label.
Each import replaces the private working-directory path with
`/home/user/acme-shop`; there are no user/host names in these captures.
All other bytes, including line breaks, indentation and horizontal rules,
are preserved. Draft/control and retained-footer negatives are explicitly
composed from these fixtures in the tests.

## Command Code narrow captures (2026-11-05)

All fourteen files under `cmd-narrow/` are full `hc read` screens of Command
Code 1.88.3 held at the stated widths, from `C2-cmd-caps/narrow/`:

| Files | Expected |
|---|---|
| `draft60.txt` | idle: multiline unsubmitted draft, including a short pasted divider; no hint row |
| `work{32,45,60,66,72,80}-01.txt` (66 uses `work66-04.txt`) | working: bare, compact counter or full interrupt footer |
| `w60-04.txt` | working: compact counter plus Tip |
| `work32-04.txt`, `work45-03.txt`, `work80-17.txt` | idle: completed turns |
| `w32-02-dialog.txt`, `w45-10-dialog.txt`, `w60-02-dialog.txt` | blocked / permission |

Only the private working-directory path is replaced with
`/home/user/acme-shop`; other bytes and physical rows are preserved. No
user or host names occur. The tests separately label derived wraps at 25
columns, wrapped full-footer/Tip cases, and multiline pasted drafts; these
are not presented as live captures.

## Command Code renderer audit (2026-11-05)

`cmd-r3/edit-dialog-w80.txt` and `cmd-r3/rm-dialog-w80.txt` come from
`C2-cmd-caps/r3/`, full `hc read` screens at 80 columns. Only the private
working-directory path is replaced with `/home/user/acme-shop`; all other
bytes and physical rows are preserved. No user/host names occur.

The edit capture expects blocked/permission. The rm capture is a control:
it is the ordinary three-choice shell dialog, not risk-marked. Risk-filtered
Yes/No choices, other file actions and generic tool dialogs are explicit
synthetic tests built from the installed cmd 1.88.3 renderer's English
strings. The bundle is read-only; no Command Code instance was run. The
report records its SHA-256 and zero-based UTF-8 byte offsets for provenance.
