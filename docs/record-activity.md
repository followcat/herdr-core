# Record activity (followcat fork)

The `record_activity` hello feature supplies optional `Terminal.activity` in full terminal
records, created/restarted and subscription/missed snapshots. Status events include the complete
current Activity atomically with status/records/reported. An `activity` event replaces Activity
alone when scope or chronology changes without a status event. Every reset/removal publishes the
complete revised object; named clocks disappear with their records. `turn` remains unchanged.

```json
{"core_id":"<origin>","epoch":"<process epoch>","scope":1,"revision":42,"records":{"":{"state_ordinal":42,"completion_ordinal":42},"build/test":{"state_ordinal":40,"completion_ordinal":null}}}
```

The root key is `""`, retained even when status is null. Ordinals come from one checked counter
per origin registry, allocated under the runtime lock in root/lexical record order. A new effective
word gets a state ordinal; completion uses that same ordinal. Adjacent `idle`/`done` spellings
are one readiness state: within the same record/scope/incarnation they retain the exact state
and completion ordinals (including null) and activity revision. Only the remembered raw word
changes; acquisition/work guards stay unchanged. The raw status still updates and publishes
with the complete unchanged Activity. App/foreground replacement and reset checks take
precedence; readiness aliases cannot revive a withdrawn or cleared marker.
Metadata changes (title, kind, message, progress, source) and repeated words do not advance it. Named removals and scope resets advance
revision even though no new word exists. This order is independent of wall time, event seq,
turns, parent `news`/`seen`, and relay arrival order. Relays preserve origin fields verbatim and
strip unsupported activity on old paths. Labels, including `hc.activity_session`, remain ordinary
uninterpreted strings.

Each record starts acquiring with no observed work. Initial idle/done or startup blocked →
idle/done is readiness. Fresh working → idle/done completes. Working → blocked → idle/done also
completes; working → error/unknown → idle/done does not. After acquisition, blocked → idle/done
also completes. Repeated or aliased readiness endpoints do not complete again. Clear, withdrawal,
eviction, exit/loss and replacement only clear markers. The complete transition table is in `src/activity.rs`.

Private acquisition/work guards and confirmed foreground owner stamps persist with records.
Recovery replay uses durable guards but cannot seed work from a repeated working word. A fresh
report or accepted inferred verdict can establish work even if its word is unchanged. Old
registries get null clocks and unknown prehistory, never synthetic completions. Restored genuine
work may still complete. The global counter resumes at least as high as saved clocks; each core
process gets a fresh epoch. Compare only within `(core_id, epoch, scope)`. Numeric order survives
planned handover; ordinary deferred-write loss on abrupt crashes remains possible. Counter or
scope exhaustion drops this terminal's optional Activity rather than wrapping.

Confirmed foreground incarnation changes/loss and known nonempty `Status.app` replacement reset
scope, guards and markers. First observation binds an owner; same-PID start-stamp hydration and
cwd changes do not replace it. Failed/unknown OS reads retain facts. No new status inference,
vendor detection or notification accounting is introduced. Same-process session replacement
without these observations is unclaimed; a session-token producer is deferred.

There is no acknowledgement API, shared seen ordinal, state-label API or frame kind 8. Clients
keep their own acknowledgement maps, baseline existing completions on a new boot/epoch, and
apply a conservative focused-render rule. Inventory can arrive ahead of the output a viewer has
consumed: new inventory with old stream output can acknowledge too early. That stall race is a
known limitation until an ordered consumption-fence contract is designed. No cross-client Done
parity or causal-consumption guarantee is supplied by this fact alone.
