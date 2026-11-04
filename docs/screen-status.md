# Screen-detected status (fork)

**This is a followcat fork feature, not upstream.** Upstream takes status only
from OSC 7501 ([schema/README.md](../schema/README.md#status)). This fork adds
an opt-out inference path for agents that never speak 7501, so they can still
show `working`/`blocked`/`idle` and unblock waits.

Nothing here replaces 7501. A program that reports status keeps full control of
its own status; the screen is only a fallback.

> **Phase 1 ships the machinery, not the runtime.** What exists today: the
> bundled rule manifests, the pure evaluation engine (`detect::detect`), the
> foreground-agent identifier (`detect::identify`) and the override catalog.
> Nothing samples screens, calls these on a timer, or publishes statuses yet.
> The runtime behavior below is the phase-2 contract the pieces already obey —
> described here so the semantics are reviewable before the wiring lands.

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
  `released.openrpc.json` stays compatible.
- **Interrupt-key selection.** Callers that branch on `reported` — input
  observation, which key interrupts the program, note delivery — keep seeing
  "this program never reported" for silent agents. Inference must not change
  what interrupts or observations do, so it never sets `reported` and never
  makes an inferred record look native.

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
| `command-code` | `command-code`; node entrypoints under `node_modules/command-code` (never bare `cmd`) | blocked, idle (captured); **working: needs capture** |

Anything else — shells, editors, one-off tools — is not inferred at all.
A terminal whose foreground is a plain shell or an unrecognized program has
no status, exactly as upstream.

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
ignored.

## Limitations

- **Authority is per hc-owned program, not per agent run.** `reported`
  survives `status::reset` and is only cleared on program replacement, so a
  shell can run several agents in a row: once any program in that terminal
  reported 7501, inference stays off for the shell's whole life; a later
  silent agent in the same shell is not detected. Fixing that needs an
  explicit agent-session concept (phase 2+).
- **Sampling (phase 2).** The planned loop reads the screen about every
  500 ms; transitions shorter than that may be missed or published late.
  Inferred `idle` may also wait for a settle check so a redraw between tool
  calls does not flap.
- **Foreground is leader-only.** hc probes the foreground process group's
  leader, not every job member. Agents hidden behind intermediate launchers
  the leader doesn't name are simply not detected.
- Inferred statuses are JSON facts only: `paint_state` must never replay
  them to viewers as OSC 7501, so a viewer cannot tell its program
  "reported" them.
