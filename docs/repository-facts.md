# Repository probe contract (followcat fork)

The private provider discovers machine-local checkout identity and caches Git observations.
It is not yet wired to terminal RPC results or the foreground watcher; that is C3 task 05.

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
GIT_NO_LAZY_FETCH=1. There is no inherited GIT_CONFIG_COUNT/GIT_DIR/GIT_WORK_TREE/GIT_EXEC_PATH,
shell, pager, fetch, status/diff, checkout, credential request or worktree scan. Config includes
and conditional/worktree config are evaluated by Git; include origins/missing include targets
are watched. The small pure upstream mapper follows remote `.` and explicit/wildcard/default
fetch mappings. Detached/unborn/missing/broken refs yield nullable facts, never zero guesses.

The whole refresh has at most eight subprocesses, a two-second deadline and 1 MiB aggregate
stdout/stderr capture; individual children have at most one second. Both pipes are nonblocking
and drained, so stderr cannot deadlock stdout. Timeout/overflow kills the process group and
reaps the child. Unix pre-exec limits address space to 512 MiB, CPU to two seconds and core dumps
to zero. A huge included config can invalidate repository facts only. Promisor repositories
omit counts entirely rather than relying on older Git versions to suppress lazy fetch correctly.

Windows deliberately starts no Git child until containment is implemented there. It can still
report discovered identity/labels and a bounded readable loose-HEAD branch; counts are unknown.
There is no uncontained Windows config parser child hidden behind that fallback.

Task 05 must enforce the approved fixed worker/queue/cache limits, never await probes in the
foreground watcher, and detach rather than join a worker stuck in an uncancellable filesystem
call. It must include the never-returning-worker shutdown regression. This provider alone starts
no workers and changes no shutdown behavior.
