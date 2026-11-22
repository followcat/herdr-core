# Fork features (followcat)

**This page is the followcat fork's contract with its clients.** It lists what
this build adds over official `hc` (upstream `herdr-core`), how to call each
addition, and how a client tells whether the core it talks to has it. hc-tui,
backlot and wherdr all run on this fork and must keep working on official hc,
where none of this exists.

Read [c3-compatibility.md](c3-compatibility.md) for the relay/consumer
matrix and [screen-status.md](screen-status.md),
[repository-facts.md](repository-facts.md) and
[record-activity.md](record-activity.md) for the facts in depth.

## How to detect any of this

Protocol generation stays **1**: every addition is an optional field, event or
capability, and a generation-1 client that ignores them behaves as before.
Two questions decide what a client may use:

1. **`hello` capabilities.** `hello.result.features` lists
   `os_hostname`, `quiet_human_release`, `repository_facts`,
   `record_activity` and `ended_by` when this core has them. Official hc
   never lists them.
2. **Field presence.** A fact arrives only where it is supported, and absence
   means **unsupported, never "none observed"**: `null`/missing counts are
   unknown, not zero.

```json
{"jsonrpc":"2.0","id":1,"method":"hello","params":{}}
{"jsonrpc":"2.0","id":1,"result":{"generation":1,"version":"0.1.21-followcat.3",
  "features":["framed","attach","events","status","shell","resize_notifications",
              "automatic_input","os_hostname","quiet_human_release",
              "repository_facts","record_activity","restart_environment",
              "put_chunks","ended_by"]}}
```

A version check is *not* a capability check: the fork suffix
(`0.1.21-followcat.3`) says this is a fork, not which facts it publishes.
Ordinary upgrades use `hello`/`server.status` versions, as on official hc.


## Screen-detected status

**What it is:** a small allowlist of agents that never speak OSC 7501 still
get `idle`/`working`/`blocked`/`done` from their screen, so `wait`, parent
notes, notifications and input observation work for them. A native 7501 report
always wins and ends inference for that program's lifetime. The bundled rule
manifests include the Command Code fixes (turn footers, tool and permission
dialogs, wrapped controls, clipped labels, draft and folder-trust evidence,
plus its `cmd -r` resume picker), Cline (its spinner, its `[y] Approve` box and
its composer; a spinner-less think stays unknown) and `cursor-agent` (its
spinner and `ctrl+c to stop` row, its approval and trust pickers, its resume
picker, sign-in and usage limits, and its composer box).

**How to call it:** call nothing. It arrives in the usual places:
`terminal.get`, `terminal.list`, `terminal.wait`, snapshots and `status`
events. An inferred record carries `"source": "screen"` inside `status`, and
names the matched agent in `app` (`identify`'s id: `command-code`, `cline`,
`cursor`, …), so clients and `hc ls` name a silent agent as they name a
reporting one:

```json
{"status":{"state":"working","app":"command-code","source":"screen"},"records":{},"reported":false}
```

`reported` stays **false** for inferred status: it still means "the program
itself reported". Do not treat an inferred word as the program's own claim
(interrupt keys and input ownership keep following `reported`).

**Since:** unknown; this shipped before `0.1.17-followcat.8`, the oldest fork
build in this branch's history.
The agent's name in `app` since `0.1.21-followcat.3`; before, an inferred
record's `app` was empty.

**Detection:** a `status` object with `source == "screen"`, or a silent
program that gains a status at all. Official hc never sets `source`.

**Doc:** [screen-status.md](screen-status.md).

**Diverges:** official hc takes status only from OSC 7501. Anything that
needs a program's own status word (templated first messages, interrupt-key
choice) must keep using `reported`.

## Quiet human release: `quiet_human_release`

**What it is:** a viewer can ask to send *human* keystrokes (kind 7) without
claiming size or display ownership, so typing in a second viewer is still
counted as a person typing — the quiet period and the unsent-draft hold keep
applying — while the primary viewer keeps the size and the display answers.

**How to call it:** request the `hello` feature `quiet_human_release`, then ask
on attach and check the confirmation; send human bytes on frame kind **7**
instead of DATA:

```json
{"jsonrpc":"2.0","id":2,"method":"terminal.attach","params":
  {"terminal":"t7","observe":false,"resize_notifications":true,"quiet_human_release":true}}
{"jsonrpc":"2.0","id":2,"result":{"channel":3,"terminal":{},"resize_notifications":true,
  "automatic_input":true,"quiet_human_release":true}}
```

Send keys on channel 3 with frame kind 7 (`wire::QUIET_HUMAN_RELEASE`).
`observe: true` never qualifies; a false or absent result means use DATA.

**Since:** `0.1.17-followcat.9`.

**Detection:** the `hello` feature **and** `quiet_human_release == true` in the
attach result. Either missing → DATA, as today. Over a relay, every hop must
confirm; a downgraded hop closes the stream instead of mapping output.

**Doc:** [c3-compatibility.md](c3-compatibility.md),

## The person's draft: `waiting_for_person`, `waited_for_person`

**What it is:** a send into a terminal waits while a person has an unsent
draft in it (Enter, Ctrl+C or Ctrl+U releases it; leaving `blocked` releases
it). The wait is `draft_wait` seconds since the person's last key (config,
default 60, `0` keeps only the quiet pause). Full terminal records say whether
input is parked on that draft, and a send says whether it was.

**How to call it:** `terminal.get`/`terminal.list` full records and snapshots
carry `"waiting_for_person": true` while a send is held; the held
`terminal.send` reply carries `"waited_for_person": true`. `hc get <t>` prints
`input: waiting for the person to submit or clear their draft`.

```json
{"jsonrpc":"2.0","id":3,"method":"terminal.send","params":{"terminal":"t7","text":"go on"}}
{"jsonrpc":"2.0","id":3,"result":{"status":"idle","turn":4,"started":true,"waited_for_person":true}}
```

Do not resend while it waits, and with `--wait` a `--timeout` still applies to
the turn **after** delivery, not to the draft hold.

**Since:** the hold came with the fork's draft work (unknown; before
`0.1.17-followcat.8`); since `0.1.21-followcat.1` the hold itself is upstream's
`draft_wait` protection and these two fields are the fork's facts on top of it.

**Detection:** `waiting_for_person` (full records) or `waited_for_person` in a
send reply. Official hc has neither and never holds.

**Diverges:** official hc holds sends the same way but reports nothing about
it. A client that shows "waiting for the person" must show nothing on official
hc.

## Observed OS hostname: `os_hostname`

**What it is:** the OS hostname the core actually observes, kept apart from
the configured display name, so clients can name a machine by what it *is* and
not by what someone called it. Machine records keep the last hostname seen at
handshake, qualified by line state.

**How to call it:** `hello` and `server.status` carry `core.hostname`
(absent when unavailable, never inferred from the display name);
`machine.list` entries carry `hostname`.

```json
{"jsonrpc":"2.0","id":4,"method":"server.status","params":{}}
{"jsonrpc":"2.0","id":4,"result":{"core":{"id":"…","name":"work","hostname":"box-a.example"},"version":"0.1.21-followcat.3"}}
```

**Since:** `0.1.17-followcat.9`.

**Detection:** the `hello` feature `os_hostname`. A fork machine may still
report a hostname to a client whose own core lacks the feature: treat it as
optional.

**Doc:** [c3-compatibility.md](c3-compatibility.md).

## Who ended a terminal: `ended_by`

**What it is:** a terminal's record says how it ended (`reason`) but not who
asked. This fork adds an optional `ended_by` to the record, filled when the
end came from a request, a parent cascade or the core itself — never when a
program exited by itself:

```json
{"address":"t3","state":"finished","reason":"killed",
 "ended_by":{"by":"terminal.kill","caller":"t1","pid":4821,"comm":"hc","terminal":"t1"}}
```

- `by`: the request (`terminal.kill`, `terminal.rm` on the journal's removed
  line) or the cause — `die_with_parent`, `server_stop`, `host_shutdown`.
- `caller`: the request's caller, as this core names it (`t3`, `rohan/t3`,
  or `ppid:<pid>` when hc was not typed inside a terminal).
- `pid`/`comm`: the process that sent the request over the core's socket
  (Unix only; a Windows named pipe names no peer).
- `terminal`: the terminal that process runs in, found by walking its
  parents up to a program this core started.
- `core`: the core the request came through, for a request from a machine.
- `parent`, `parent_by`: for `die_with_parent`, the parent that ended and its
  own `ended_by`.

The record is deleted by `terminal.rm`, so the core also keeps a journal, one
JSON line per end and per removal, in the session's state directory:
`<state dir>/ended.jsonl`, rotated at 1 MiB into `ended.jsonl.1` (one previous
file kept), file mode `0600`. A line holds `time_ms` (UTC epoch
milliseconds), `event` (`ended` or `removed`), `address`, `name`, the
`herdr.space`/`herdr.tab` labels, `program` (the basename of `argv[0]` only),
`reason`, `exit_code`, `signal` and `ended_by`. It never holds argv beyond
that basename, an environment, cwd contents or screen text. A journal that
cannot be written never fails the request; it is logged once.

**How to call it:** `terminal.get`, `terminal.list` (`hc get`, `hc ls --all`)
and the `finished`/`lost` event data carry `ended_by`; `hc ended [--since 1h]
[--json]` prints the journal oldest first (`--json`: one array), and
`hc ended --since` takes `30s`, `10m`, `4h`, `2d`, `all`.

**Since:** `0.1.21-followcat.2`.

**Detection:** the `hello` feature `ended_by`, or the field on a record. A
machine whose core lacks it has the field stripped from records mirrored here,
so absence stays "unsupported", never "nobody ended it".

**Diverges:** official hc has no such field, no journal and no `hc ended`.

## Repository facts: `repository_facts`

**What it is:** machine-side cached git identity for each terminal's folder:
origin keys, checkout identity, branch, upstream and local ahead/behind
counts, with a freshness state. Probes are bounded, never fetch, and report
unknown rather than zero.

**How to call it:** full terminal records, snapshots and `repository` events
carry `Terminal.repository`:

```json
{"type":"repository","terminal":"t7","data":{"repository":{"state":"ready","cwd":"/srv/app","branch":"main",
  "ahead_behind":{"ahead":1,"behind":0},"checkout":{"repository_key":"…","checkout_key":"…","repo_name":"app",
  "checkout_name":"app","repo_root":"/srv/app","is_linked_worktree":false,"is_bare":false},"auto_label":"app"}}}
```

Keys and paths are opaque: group by origin `core_id` plus `repository_key`.
The compact `terminal.list` projection keeps its old shape; use full records,
`terminal.get` or events for these facts.

**Since:** `0.1.17-followcat.9` (probe containment and branch-name validation
landed in the same build).

**Detection:** the `hello` feature `repository_facts`. Absence of the field
means unsupported, **not** "not a git repository": a `ready` directory can
still have null git fields, and unknown counts are `null`, not `0`.

**Doc:** [repository-facts.md](repository-facts.md).

## Record activity: `record_activity`

**What it is:** durable per-record chronology: a state ordinal per new
effective word and a completion ordinal when readiness follows work, qualified
by origin core, process epoch and scope. It is chronology only — no
acknowledgements, no labels.

**How to call it:** `status` events carry the complete `activity` atomically
with status/records/reported; `activity` events publish scope/reset-only
changes; full records and snapshots carry `Terminal.activity`:

```json
{"core_id":"9c9a…","epoch":"305f…","scope":2,"revision":42,
 "records":{"":{"state_ordinal":42,"completion_ordinal":42},"build/test":{"state_ordinal":40,"completion_ordinal":null}}}
```

Compare only within `(core_id, epoch, scope)`; the root key is `""`.

**Since:** `0.1.17-followcat.9` (readiness-alias chronology fixes followed in
`0.1.18-followcat.1`).

**Detection:** the `hello` feature `record_activity`. Without it, clients
acknowledge locally by turn and status, as before.

**Doc:** [record-activity.md](record-activity.md).

## `HERDR_ENV` for terminal programs

**What it is:** every program hc starts gets `HERDR_ENV=1`, herdr's pane
contract, so agents forward clipboard writes to the client terminal (OSC 52)
instead of only touching this machine's clipboard. hc-tui already forwards
that sequence.

**How to call it:** read `HERDR_ENV` inside the terminal (`$HERDR_ENV` is `1`).
The core sets it when the caller does not; an explicit override still wins.

**Since:** `0.1.21-followcat.1`.

**Detection:** a program's own environment. Treat it as a hint, not a
capability.

**Diverges:** official hc sets `TERM`, `TERM_PROGRAM` and `COLORTERM` but no
`HERDR_ENV`.

## Hardening (behaviour, no new API)

**What it is:** the fork tightens paths that already exist upstream, so a
client sees a conservative answer where official hc might have guessed:

- repository probes deny every git transport (`GIT_ALLOW_PROTOCOL` empty,
  `core.sshCommand=false`, no lazy fetch) and bound children, time and output;
- symbolic branch names and the loose-HEAD fallback follow git's complete ref
  rules, Unicode whitespace included;
- foreground observations must be positive, group-leading and the same process
  incarnation; unreadable metadata is *unknown*, never "gone", and a dead or
  zombie leader withdraws inferred status;
- machine observations persist outside the runtime lock.

**How to call it:** nothing new; it shows up as `state: "unavailable"`, `null`
counts, `unknown` foreground or a withdrawn status.

**Since:** unknown for the foreground and ref rules (before
`0.1.17-followcat.8`); the repository probe containment shipped in
`0.1.17-followcat.9`.

**Detection:** not applicable; the schema already allows the conservative
answers.

**Doc:** [repository-facts.md](repository-facts.md),
[screen-status.md](screen-status.md).

## Fork versioning and updates

**What it is:** this build's version is `0.1.21-followcat.3`: upstream's
`0.1.21` plus a fork suffix. The suffix works everywhere the version appears
(`hc --version`, `server.status`, `hello`) and in `hc update --from <binary>`.

**How to call it:** read any version field and look for a `-followcat.N`
suffix. `older()` compares only the three numeric components, so
`0.1.21-followcat.1` and `0.1.21-followcat.2` compare equal.

**Since:** the fork's first build (unknown; before this branch's history).

**Detection:** the suffix itself. Do not require it to decide whether a
*feature* exists — use the capabilities above.

**Diverges:** a fork never updates to an official release by itself. Plain
`hc update` and `hc update --version` refuse; the only update path is
`hc update --from <binary>`, which keeps terminals and detection state. The
same refusal guards release downloads a `hc machine add --install` could make.

**Doc:** [screen-status.md](screen-status.md).

## Wiring it into a client

hc-tui's `hc-proto` is the reference consumer: it parses the capability list,
keeps every new field optional, and falls back to DATA, to no hostname, and to
client-local chronology when a field or feature is absent. When in doubt,
prefer "unknown" over a guess — that is what these facts mean.

**Diverges: extra event kinds.** With `repository_facts` or
`record_activity`, a subscriber also hears `repository` and `activity` events
for terminals, interleaved with the usual stream. Filter by `params.type`
instead of expecting the next event to be the one you just caused; the fork's
own label-filter test had to do the same.
