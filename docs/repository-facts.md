# Repository probe contract (followcat fork)

The `repository_facts` feature supplies optional `Terminal.repository` in full terminal RPC
records and snapshots. `repository` events replace that complete object. It contains origin
`core_id`, observed machine-side `cwd`, `state` (`pending`, `ready`, `stale`, `unavailable`),
nullable `auto_label`, `checkout`, `branch` and `ahead_behind: {ahead, behind}`. Checkout contains
`repository_key`, `checkout_key`, `repo_name`, `checkout_name`, `repo_root`, `is_linked_worktree`
and `is_bare`. Keys and paths are opaque on viewers; group by origin core and repository key.
Absence means unsupported, not a non-repository. Ready plain directories have null Git fields.
No upstream, unborn/detached/broken refs or failed counts never imply observed zero counts.

Discovery walks at most 512 ancestors, accepts valid directory/gitdir/commondir and bare layouts,
and bounds marker reads to 64 KiB. Group keys are the canonical-best-effort common Git directory;
checkout keys are canonical checkout roots. Repo labels use checkout root names, with HOME `~`
outside Git. Linked checkouts share a repository key, never a HEAD/count cache.

Each checkout's probe stats HEAD, root marker, commondir, packed refs, selected loose refs,
config/worktree config, user config, reftable tables and observed include paths first. Unchanged
five-second ticks start no child. Revalidate anyway after 60 seconds. Stat errors are unavailable,
not proof of absence; symlink targets are part of the fingerprint. Failed probes are cached until
a known dependency changes or the 60-second retry. No cache is authoritative across core startup.

Every child is a direct `git` invocation with this fixed prefix:

```text
git --no-pager -c core.fsmonitor=false -c core.hooksPath=/dev/null
    -c core.sshCommand=false -c protocol.allow=never
    -C <discovered root> --git-dir <discovered git directory> <subcommand>
```

Only these subcommands are used (repeated for count revalidation):

```text
config --includes --null --list --show-origin
symbolic-ref --quiet HEAD
rev-parse --revs-only --end-of-options HEAD <full upstream ref>
rev-list --left-right --count <validated head oid>...<validated upstream oid>
```

The environment is cleared, then contains only PATH, the machine-side HOME/XDG_CONFIG_HOME,
LC_ALL=C, GIT_CONFIG_NOSYSTEM=1, GIT_OPTIONAL_LOCKS=0, GIT_TERMINAL_PROMPT=0 and
GIT_NO_LAZY_FETCH=1, GIT_SSH_COMMAND=false and GIT_ASKPASS=false.
There is no inherited GIT_CONFIG_COUNT/GIT_DIR/GIT_WORK_TREE/GIT_EXEC_PATH,
shell, pager, fetch, status/diff, checkout, credential request or worktree scan. Config includes
and conditional/worktree config are evaluated by Git; include origins/missing include targets
are watched. The small pure upstream mapper follows remote `.` and explicit/wildcard/default
fetch mappings. Detached/unborn/missing/broken refs yield nullable facts, never zero guesses.

The whole refresh has at most eight subprocesses, a two-second deadline and 1 MiB aggregate
stdout/stderr capture; individual children have at most one second. Both pipes are nonblocking
and drained, so stderr cannot deadlock stdout. Timeout/overflow kills the process group and
reaps the child. Unix pre-exec limits address space to 512 MiB, CPU to two seconds and core dumps
to zero. A huge included config can invalidate repository facts only. Any configured
`extensions.partialClone`, `remote.*.promisor` or `remote.*.partialCloneFilter` entry omits counts
and skips object commands. This is deliberately conservative even for false/empty/unrecognized
values: no Git version's boolean parsing or lazy-fetch environment support is trusted to prevent
network/helpers. SSH, askpass and protocol overrides provide an additional barrier.

Windows deliberately starts no Git child until containment is implemented there. It can still
report discovered identity/labels and a bounded readable loose-HEAD branch; counts are unknown.
There is no uncontained Windows config parser child hidden behind that fallback.

The scheduler uses observed foreground cwd, otherwise launch cwd. It examines at most 64
terminals every 250 ms, with two fixed IO workers, at most two jobs started per second, one job
per key in flight and a deduplicated 128-key queue. Revalidate positive discovery/probes every
five seconds, non-repository discovery every 30; foreground changes coalesce at a one-second
per-key floor. The LRU caches hold at most 1,024 directories and 256 checkout contexts. Probe
dependencies are capped at 256 paths and 128 KiB of path text per cache entry. Overload can delay
freshness but cannot create workers or an unbounded queue. Registrations retire on finish/removal.

Filesystem/Git work never runs under the runtime lock or in the foreground watcher. Shutdown
detaches the two workers without joining a filesystem syscall that may never return. Late
results from invalidated generations are discarded. Same-cwd observations survive failures as
stale; cwd changes publish pending with null values. Positive observations become stale after
15 seconds without success, plain directories after 45. Finished facts freeze. Restored facts
begin stale and must be revalidated. Older relay paths strip unsupported repository objects and
patches instead of exposing incidental cached facts. The compact CLI list retains its existing
projection; use full terminal RPCs, get or events for these facts.
