# Screen-detected status (fork)

**This is a followcat fork feature, not upstream.** Upstream takes status only
from OSC 7501 ([schema/README.md](../schema/README.md#status)). This fork adds
an opt-out inference path for agents that never speak 7501, so they can still
show `working`/`blocked`/`idle` and unblock waits.

Nothing here replaces 7501. A program that reports status keeps full control of
its own status; the screen is only a fallback.

## How it works

For each live terminal, the core watches the foreground process (the same probe
that fills `foreground` on the record and events). When the foreground resolves
to a **validated agent** (below) and the terminal's current program has never
sent a 7501 report, the core samples the visible screen about twice a second
and evaluates that agent's rule manifest: a strict set of `contains`/`regex`
matchers over named screen regions (bottom lines, prompt box, title, ...).

The best matching rule publishes a `Status` record as the terminal's root
status. `state` is one of `idle`, `working`, `blocked`; on `blocked`, `kind`
is `permission` or `question` when the matched rule says so. The record is
published through the same `status_changed` path as a native report, so
`status` events, `wait`, turn counting and parent notes behave identically.

Two inputs differ from a native report:

- **`osc_title`**: filled from the terminal's OSC title (`Screen::title()`),
  since Claude and Codex rules read it.
- **`osc_progress`**: always empty; hc's wrapper does not capture OSC progress
  sequences. Rules on that region simply never match.

## Provenance

Inferred statuses carry `"source": "screen"` on the `Status` record. Native
7501 records have no `source` field. The field is an additive generation-1
extension: old clients ignore it, and `released.openrpc.json` stays
compatible.

The `reported` flag keeps its native meaning — "the program itself sent at
least one 7501 report". Inference never sets it. Callers that branch on
`reported` (input observation, interrupt key choice, note delivery) keep
behaving exactly as if the program had no status.

## Native authority

A program's first real 7501 report ends screen inference for the **lifetime
of that hc-owned program**: the inferred root is replaced by the native root,
pending inferred transitions are dropped, and detection stays off until the
program ends and its status facts reset (`status::reset`). Native `clear`
reports also win: after a native clear the root stays empty rather than
falling back to the screen.

Core restarts do not re-arm inference on a program that already reported:
`reported` is persisted in `terminals.json` and survives handover.

## State mapping

- `working` — the screen shows the agent mid-turn (spinners, tool footers,
  interrupt hints). Published promptly.
- `blocked` — the screen shows a prompt that needs a person (permission,
  question, trust dialog). Published promptly.
- `idle` — the screen shows the agent's live prompt ready for input. Inferred
  readiness is published as `idle`, never `done`: a visible prompt proves the
  agent is waiting, not that work completed.
- No rule match → **unknown**: nothing is published. Unknown never times out
  into `idle`; only positive evidence can end an inferred `working`.

Rules marked `skip_state_update` (e.g. transcript viewers, model pickers)
leave the current inferred status untouched.

## The allowlist

Inference runs only for manifests validated against captured screens:

| manifest | foreground match | coverage |
|---|---|---|
| `devin` | `devin`, `devin-cli`; node entrypoints under `devin*`/`@devin*` paths | working, blocked, idle |
| `claude` | `claude`, `claude-code`; `@anthropic-ai/claude-code` entrypoints | working, blocked, idle, transcript-skip |
| `codex` | `codex`; `@openai/codex` entrypoints | working, blocked, idle (all mostly title-driven) |
| `opencode` | `opencode`, `open-code`, `opencode2`, `opencode-ai` | working, blocked; no idle rule |
| `command-code` | `command-code`; node entrypoints under `node_modules/command-code` (never bare `cmd`) | blocked, idle; no working rule |

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
`id` matches the agent replaces the bundled manifest; an unreadable or
invalid file is ignored with a warning in `core.log` and the bundled rules
apply. Manifests load and compile once when the core starts; `hc server
restart` picks up changes without a rebuild and keeps running terminals.

Manifests are bounded: ≤128 rules, gate depth ≤8, ≤512 gates and ≤1024
matchers total, ≤512 chars per pattern, regexes compiled once and shared.
A manifest that exceeds the bounds fails validation and is ignored.

## Limitations

- **Authority is per hc-owned program, not per agent run.** A shell can run
  several agents in a row. Once any program in that terminal reported 7501,
  inference stays off for the shell's whole life; a later silent agent in the
  same shell is not detected. Fixing that needs an explicit agent-session
  concept (phase 2+).
- **Sampling.** Detection reads the screen about every 500 ms; transitions
  shorter than that may be missed or published late. Inferred `idle` may also
  wait for a settle check so a redraw between tool calls does not flap.
- **Foreground is leader-only.** hc probes the foreground process group's
  leader, not every job member. Agents hidden behind intermediate launchers
  the leader doesn't name are simply not detected.
- Inferred statuses are JSON facts only: `paint_state` never replays them to
  viewers as OSC 7501, so a viewer cannot tell its program "reported" them.
