# Screen-detected status (fork)

**This is a followcat fork feature, not upstream.** Upstream takes status only
from OSC 7501 ([schema/README.md](../schema/README.md#status)). This fork adds
an opt-out inference path for agents that never speak 7501, so they can still
show `working`/`blocked`/`idle` and unblock waits.

Nothing here replaces 7501. A program that reports status keeps full control of
its own status; the screen is only a fallback.

> **Ships:** the bundled rule manifests, the pure evaluation engine
> (`detect::detect`), the foreground-agent identifier (`detect::identify`),
> the override catalog, the inferred/native arbitration (`status::apply_inferred`,
> `Status.source`), the detection worker (`watch_screen_status`) and runtime
> recovery for inferred state (adoption, handover, kept replacement).

## Three separate facts

Keep these apart; conflating them is where wrong behavior hides.

- **Native authority.** The first real 7501 report from a terminal's current
  program ends screen inference for that program's lifetime: the native root
  replaces any inferred root, pending inferred transitions are dropped, and
  detection stays off. Native `clear` reports win too — after a native clear
  the root stays empty rather than falling back to the screen. Authority is
  tracked by `Terminal::reported`, which means "the program itself sent at
  least one 7501 report". Inference never sets it. `status::reset` clears
  `status`/`records` but **preserves** `reported`; the flag is only cleared
  when the terminal's program is replaced (a kept terminal getting a new
  program) or at creation — and it is persisted in `terminals.json`, so a
  core restart never re-arms inference on a program that already reported.
- **Status availability.** Silent agents still need a readable status so
  `status`, `wait`, parent notes and notifications behave. That is the whole
  point of inference: the winning rule publishes a `Status` record as the
  terminal's **effective root record** — not a `"screen"`-named child —
  through the same `status_changed` path as a native report. Inferred
  records carry `"source": "screen"`; native records have no `source` field.
  The field is an additive generation-1 extension: old clients ignore it and
  `released.openrpc.json` stays compatible. The *availability* checks that
  used to read `reported` — input observation (`input.rs`), `wait`
  (`wait.rs`), note delivery (`notes.rs::can_take_note`) and template
  first-message readiness — now read "native or inferred status"
  (`Terminal::status_available`): a silent agent whose screen says it is
  working can observe its input and take a note just like a reporting one.
- **Interrupt-key selection.** `reported` stays what it means — "the program
  itself reported" — and keeps deciding which key interrupts a program and
  whether a program claims input. Inference never sets `reported`, so an
  inferred status can change what callers *see* without ever changing which
  keys a program gets.

## Inputs

The engine is pure. `detect::detect(&manifest, &Input)` reads:

- **`screen`**: the visible screen's plain text.
- **`osc_title`**: the terminal's OSC title (`Screen::title()`), since
  Claude and Codex rules read it.
- **`osc_progress`**: always empty; hc's wrapper does not capture OSC
  progress sequences. Rules on that region simply never match.

No match gives `unknown`. `unknown` never publishes, never times out into
`idle`, and only positive evidence ends an inferred `working`.

## State mapping

- `working` — the screen shows the agent mid-turn (spinners, tool footers,
  interrupt hints, a live `!` shell's background hint). Published promptly.
- `blocked` — the screen shows a prompt that needs a person (permission,
  question, trust dialog). Published promptly; `kind` is `permission` or
  `question` when the matched rule says so.
- `idle` — positive readiness evidence (a live prompt, a title naming the
  agent at rest). Inferred readiness is published as `idle`, never `done`: a
  visible prompt proves the agent is waiting, not that work completed.
- No rule match → **unknown**: nothing is published.

Rules marked `skip_state_update` (e.g. transcript viewers, model pickers)
leave the current inferred status untouched.

## The allowlist

Inference runs only for manifests validated against captured screens.
Coverage is honest: rows marked *needs capture* have no real screen for that
state yet, so the rule is unproven (or absent) until a capture exists.

| manifest | foreground match | coverage |
|---|---|---|
| `devin` | `devin`, `devin-cli`; node entrypoints under `devin*`/`@devin*` paths | working, idle (captured); **blocked: needs capture** |
| `claude` | `claude`, `claude-code`; `@anthropic-ai/claude-code` entrypoints | working incl. foreground shell, blocked, idle (captured); **transcript/menu skip: needs capture** |
| `codex` | `codex`; `@openai/codex` entrypoints | blocked (captured); **idle, working and real title transitions: needs capture** |
| `opencode` | `opencode`, `open-code`, `opencode2`, `opencode-ai` | blocked (captured); **working and evidence-backed idle: needs capture** |
| `command-code` | `command-code`; node entrypoints under `node_modules/command-code` (never bare `cmd`) | working, tool/trust permissions, idle (captured) |
| `cline` | `cline` (the npm shim), `.cline` (the package's own executable, also its `comm`); node entrypoints under `node_modules/cline` | working, permission and idle (captured); **a spinner-less think is unknown by design** |
| `cursor` | `cursor-agent` (and its path via argv0, which its bash wrapper keeps) | working, approvals, workspace trust, resume picker, sign-in/usage limits, idle (captured) |

Anything else — shells, editors, one-off tools — is not inferred at all.
A terminal whose foreground is a plain shell or an unrecognized program has
no status, exactly as upstream.

### Cline

Cline's composer is `❯ Ask anything...` between two full-width rules, with a
status footer under it (a context meter, then the workspace and
`Auto-approve all …`). The three rules read three different regions, and
nothing else on screen can stand in for them:

- **working**: a braille spinner with its interrupt hint
  (`⠏ Thinking... (esc to cancel)`) as the *last* content line above the
  composer. Braille inside quoted code or tool output, and a scrollbar column
  of `█`, sit further up and never count.
- **blocked**: `[y] Approve` and `[n] Deny` in the dialog area under the last
  rule, with no composer in that area — a box quoted above a live composer is
  a transcript, not a question.
- **idle**: the composer's own `❯ Ask anything...` placeholder, with neither a
  spinner nor an unanswered `❯ <message>` as the newest line above it. Cline
  can think for a minute with neither spinner nor hint on screen; that screen
  stays `unknown`, because only positive evidence may publish a state.

### Cursor (`cursor-agent`)

Cursor's composer is a `▄`/`▀` box around a single `→ ` input row, with the
model and folder under it. Each rule reads the region its evidence lives in:

- **working**: `ctrl+c to stop` on the composer's input row, or a braille frame
  leading an `-ing` activity word above the box (`bottom_non_empty_lines(10)`).
- **blocked / permission**: an approval picker below the last full-width `─`
  divider — the `→ ` option cursor, `Run Everything (shift+tab)`, and
  `allowlist` or the tool's own question line — or the rounded workspace-trust
  box (`[a] Trust this workspace`, `[q] Quit`).
- **blocked / question**: the resume/chat picker (`All chats • <scope>`,
  `↑/↓ to navigate`, `Enter to select`, `Esc to close`), which wants a
  selection rather than an approval.
- **blocked, no kind**: `please sign in to continue` (auth) and the terminal
  usage/spend limits (`usage_limit_exceeded`, `concurrent_limit_exceeded`).
- **idle**: the composer box's own `▀▀▀▀` and `→ ` rows, and never while
  `ctrl+c to stop` is on them. A picker or the resume list has no box, so it
  can never read as readiness.

Command Code's resume picker (opened with `cmd -r`) is inferred the same way
(blocked / question) instead of leaving the terminal `unknown`.

## Configuration

`~/.config/herdr-core/config.toml`:

```toml
[detect]
enabled = false            # default: true in this fork
agents = ["devin", "claude"]  # optional allowlist limit; unset = all bundled
```

Rule overrides live in `<config dir>/detect/<agent>.toml`, next to
`config.toml` — e.g. `~/.config/herdr-core/detect/devin.toml`. A file whose
`id` matches the agent replaces the bundled manifest; a non-regular file, a
file over the size cap, or an invalid file is ignored with a warning in
`core.log` and the bundled rules apply. Manifests load and compile once when
the core starts; `hc server restart` picks up changes without a rebuild and
keeps running terminals.

Manifests are bounded: ≤128 rules, gate depth ≤8, ≤512 gates and ≤1024
matchers total, ≤512 chars per pattern, ≤256 KiB per file, regexes compiled
once and shared. A manifest that exceeds the bounds fails validation and is
ignored. Engine 4 also allows a nested gate to select an absolute `region`
of the same input. Children without `region` inherit their parent's slice.
Command Code uses this to check the composer body for readiness while only
checking the area above it for a live busy footer; draft text is never busy
UI evidence. A complete composer box is readiness evidence even if its
shortcuts hint is off screen.

Command Code's `full_width_prompt_box_body` and
`above_full_width_prompt_box` require a pair of pure horizontal rules of the
longest pure rule's length, with a prompt row inside. The longest rule
supplies the width even when a dialog replaces the composer. This measures
only one-cell rule glyphs, not arbitrary Unicode text. Equal maxima select
the last rule, while short command/explanation dividers remain content.
Short pasted dividers remain draft content. `after_last_full_width_rule`
selects the dialog or hint area below the last maximum-length divider.
The older region names keep their existing behavior for other manifests. A pasted divider
exactly as wide as the terminal remains ambiguous and is an accepted limit.
Engine 5 adds `outside_full_width_prompt_box`: below the last full-width
rule when a verified composer exists. Engine 6 recognizes the renderer's
normal `❯`, shell-input `!` and ASCII `>` prefixes and returns no trust
evidence if an apparent composer cannot be verified (clipped or unequal
edges, an unknown prefix, or a remaining shortcuts hint). Without composer
evidence, the whole input remains eligible. The bundled Command Code
manifest requires engine 8, including logical-line selection below; older
engines reject it. Plan and accept-edits change hints,
not the prefix. Folder trust reads only this region.
A real unboxed TrustPrompt remains visible even without its top divider;
wrapped title/control phrases are compared across whitespace.

Engine 8 adds `logical_lines = true` to a rule or a nested gate with an
absolute `region`. It opts that subtree into Ghostty's logical view of the
same visible screen: soft-wrapped cells join without an added space,
explicit newlines stay separate, and wide-character wrap spacers disappear.
Other gates use the physical view, and descendants inherit their selected
view. Without VT metadata, no soft-wrap joining occurs. Both views retain
the input-size bound. Command Code opts in only for its footer and the
corresponding idle exclusion; its composer and permission checks stay
physical. The runtime captures the extra view only for opted-in manifests,
under the same snapshot lock and staleness token.

Engine 7 adds a `prefix` matcher: `{ regex, min_chars, patterns }`. The
layout regex must name a `prefix` capture; its whitespace is folded to
single spaces. A complete label matches a configured regular language, or
its clipped form must have at least `min_chars` Unicode characters and be
a prefix of a member of that language. Patterns are case-sensitive unless
they specify flags; positional assertions inside label patterns are
rejected. Named and unnamed captures are transparent in derived prefix
expressions; copying a subexpression cannot duplicate its capture name.
The shape regex and both full/prefix programs for each label pattern
count toward the existing matcher budgets and 512-character source limit. Derived prefix expressions
are bounded to 16 KiB and compiled programs to 1 MiB per expression.

Command Code 1.88.3's closed English activity inventory includes all 74
default verbs, permission results, execution, compaction, connection,
updater/session naming, settings, permission waits, submit errors and
harness retry messages. Its working rule and idle exclusion use identical
prefix matchers with a six-character minimum (excluding padding); complete
short labels such as `Error` remain valid. This accepts any word or
character cut, including compact-counter gaps of one space. Arbitrary
mod-provided labels remain outside the inventory. Built-in UI bytes can be
imitated exactly; screen text cannot recover their origin.

All selected Yes/No rows accept figures' complete pointer set: `❯` or
fallback `>` (including `TERM=linux`). This applies to trust, shell, file,
generic and risk-filtered dialogs, and to the check that No is the final
numbered choice. A later selected `> 4. Yes` cannot be a No continuation.

Command Code accepts the six captured spinner glyphs `· ☆ ⌘ ◇ ○ ✧`, a
built-in activity label, and either no suffix, a compact counter (including
decimals and `k`), or full interrupt controls. All permission allowed/denied
labels and `Executing: <command>` are supported. A full-footer remainder must
sit directly after that activity line. The footer starts at column 0 or 1: Ink can remove the leading cell
when Status overflows, irrespective of its label. Cloud
waits use ShimmerLine: its icon may shrink to one cell and its text may
wrap directly to column 2 or 3. AssistantMessage continuation text
starts at column 2. Turns below 1000ms omit WorkedDurationNote, so a final
two-space `· Working…` reply is idle. The captured `Use shift+tab to enable
accept edits` Tip can wrap between words; it does not admit arbitrary later
transcript rows. The built-in cloud clone hint is likewise matched as fixed
text. RetryMessage has its own column-zero text, which may wrap only
between its fixed words onto the immediately following row, with zero or one leading space;
blank or unrelated intervening rows do not count. `✻` summaries
and `⠶` message bullets cannot count as a busy spinner.

Tool permissions are selected below the last full-width rule. The
`Command Code needs to` and `enter select` phrases match after whitespace
joining across physical rows (equivalently, `\s+` between their words),
while numbered Yes and final No choices remain required as physical rows.
The shell and generic-tool headings pair with `Command Code needs to
execute/run`; file-operation headings pair with `Do you want to` and
make-this-edit/create/delete/execute/modify questions. All five file action
headings and ordinary/risk-filtered choices come from cmd 1.88.3's renderer.
Risk-filtered dialogs remove scoped approvals and renumber No from 3 to 2;
a later numbered option after No invalidates the choice block.

## Recovery

A core restart (`hc server restart`, `hc update`, crash recovery, kept-terminal
relink) rebuilds every screen from the retained tail. Inference survives it:

- **Native authority is permanent.** `reported` is persisted in
  `terminals.json` the moment a program's first 7501 arrives — including a
  child-only or `clear` report — so a restart never re-arms inference on a
  program that already reported, even when 7501 has scrolled out of the tail.
  A kept terminal that gets a replacement program clears `reported` and
  re-arms inference for the new program.
- **A restored inferred root is unconfirmed.** A `source:"screen"` root that
  survives adoption stays visible (waits still see it) but must be re-proven:
  the watcher must first observe its current foreground after adoption; saved
  process names and arguments alone cannot confirm or withdraw it. Then a
  fresh accepted verdict — `working`, `blocked`, or `idle` after its normal
  settle — from a post-restart snapshot clears the mark. Re-confirming the
  same state and kind is a no-op: no event, no turn. `unknown` evidence and
  skipped snapshots neither confirm nor extend the deadline.
- **Unconfirmed roots expire.** Adoption + 30 s without an accepted positive
  verdict withdraws the root quietly — this is the documented turn exception:
  the recovery withdrawal stamps the terminal, publishes a null status and
  persists, but counts **no turn** and produces no completion news (a
  wherdr-style "unread turn" derivation stays false). The deadline is checked
  again under the commit lock; a verdict collected before expiry cannot
  confirm the root at or after its deadline. A *confirmed* inferred
  root withdrawn later — e.g. the foreground agent exits — does count a turn
  exactly like a native status clearing does. Disabled detection and an empty
  catalog keep an expiry-only worker alive, including across detached holders
  and later adoptions. An agent dropped from a non-empty catalog withdraws
  the same quiet way once its foreground is freshly observed. Recovery
  withdrawals coalesce into deferred checkpoints within two seconds;
  empty-catalog passes release the state lock and yield between terminals.
  Normal expiry also defers persistence rather than serializing per root.
- **Failed foreground lookups are unknown.** A missing link, timeout or
  malformed reply, any nonpositive pgid (including older holders' -1 OS
  errors), or unavailable Unix process metadata keeps the last observed
  foreground and emits no foreground or inferred-status event. On Linux,
  a missing `/proc/<pid>`, ESRCH or stat state Z (zombie) / X (dead) proves
  the process gone; permission errors and partial reads of a live process are
  unknown. A dead leader is absent even if its cmdline is empty or it dies
  during the metadata reads. Linux accepts metadata only for the
  sampled group leader, with the same stat start time before and after the
  reads. The internal owner key includes that start time, so same-pid reuse
  by the same agent still invalidates the previous owner's verdict. The stamp
  is absent from wire/schema/persisted foreground facts and is rehydrated
  silently on the first valid observation. macOS similarly requires positive
  ESRCH evidence after a failed metadata lookup. Windows metadata lookup
  behaviour is unchanged. A valid positive pgid identifying the shell or a
  different program still withdraws the old owner's status.
  The first real observation after adoption permits silent reconfirmation.
- **A detached holder freezes its terminals.** Reader failure invalidates
  pending verdicts immediately. Until relink or finish, detection collects,
  commits, settles, confirms, expires and withdraws nothing on those terminals;
  a missing holder means wait. Native reports and finish accounting stay live.
  Relink marks an inferred root unconfirmed and starts normal reconfirmation.
- **In-flight verdicts die at the boundary.** Adoption, relink, finish,
  removal, restart requests, shutdown and handover bump a per-terminal
  invalidation generation and the output sequence under the state lock, so a
  verdict read before the boundary can never commit after it. Stop, restart,
  handover and shutdown also freeze detection entirely: nothing is collected,
  committed, settled, confirmed or withdrawn while frozen; native reports and
  finish accounting still apply.

## Fork version and updates

This build is `0.1.21-followcat.2` — `0.1.21` plus a fork suffix. The suffix
works everywhere the version appears (`hc --version`, `server.status`,
`hello`) and in `hc update --from <binary>`.

- `older()` compares only the three numeric components, so `0.1.17`,
  `0.1.17-followcat.1` and `0.1.17-followcat.2` all compare **equal** — a
  later fork build swaps in without `--force`, and a suffix-only "downgrade"
  is not caught either.
- `hc update --version` still takes only `X.Y.Z` and rejects the suffix.
- **A fork never updates to an official release by itself.** Plain
  `hc update` and `hc update --version` refuse with a clear message; the
  escape — and the only update path — is `hc update --from <binary>`, which
  keeps terminals, `reported` and turn counts. The same refusal guards every
  release download a `hc machine add --install` could make: same-platform
  targets get this binary copied byte-for-byte (which works fine), while a
  Windows or other-OS target refuses instead of fetching an official release.
- `hc machine add --install` installs only after a missing or incompatible
  answer; a box that already runs a compatible core is left untouched (no
  copy, no restart). Replacing a remote core is `hc machine update`.

Install it by copying the packaged `herdr-core` to `~/.local/bin/` (with an
`hc` link beside it) and running `hc update --from <binary>` or
`hc server restart`; do not run `hc update` — it has no release to fetch.

## Limitations

- **Authority is per hc-owned program, not per agent run.** `reported`
  survives `status::reset` and is only cleared on program replacement, so a
  shell can run several agents in a row: once any program in that terminal
  reported 7501, inference stays off for the shell's whole life; a later
  silent agent in the same shell is not detected. Fixing that needs an
  explicit agent-session concept (phase 2+).
- **Sampling.** `watch_screen_status` reads eligible screens about every
  500 ms; transitions shorter than that may be missed or published late.
  Working and blocked publish at once; inferred `idle` waits for a 1.5 s
  settle deadline so a redraw between tool calls does not flap — an
  unchanged screen confirms it without re-evaluating, and `unknown` never
  times out into `idle`.
  Evaluation happens off the runtime lock; a verdict commits only if the
  screen, program, foreground and manifest it was read from are unchanged.
  Resize, stitch completion, a foreground/agent change or a native report
  invalidate pending work; a cwd move alone does not.
- **Foreground is leader-only.** hc probes the foreground process group's
  leader, not every job member. Agents hidden behind intermediate launchers
  the leader doesn't name are simply not detected.
- Inferred statuses are JSON facts only: `paint_state` must never replay
  them to viewers as OSC 7501, so a viewer cannot tell its program
  "reported" them.

The physical-row regression sweep feeds every isolated Ink frame through
Ghostty and the runtime's row normalization before detection. Both the 11
requested widths and every integer width 16–120 run by default. Logical
joining uses Ghostty's wrap flags, never a row-length guess. Known hard
Ink word wraps still need their explicit layout grammar; in particular
RetryMessage can leave one leading space at an exact-width word boundary.
