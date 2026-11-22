# Retirement and abandonment

[Statechart](statechart.mmd) · [TLA+ spec](RetireAbandon.tla) · [TLC configuration](RetireAbandon.cfg)

The model follows one task in one slot from the reader's side: editing,
committing, pushing the topic, a finish that integrates, holds, `wl retire`
and `wl retire --abandon --decision REF`. Integration is one step; its stages
are in the [finish chart](../lifecycle-finish-integration.mmd).

TLC checks three invariants:

- `NoCommitLost`: a retired task's commits are on the target or on `origin/<branch>`.
- `NoDirtyOrHeldRetire`: retirement never ends uncommitted work or a held task.
- `OnlyAbandonSkipsTarget`: only an abandonment ends a task whose commits are not on the target.

The model leaves out concurrency, slot claims and crashes. The code-level check
of the same guards is `NoRetireUnsafeTask` in
[verify-slot-claims](../../skills/verify-slot-claims/SKILL.md).

Regenerate the chart with the statechart skill and its TLC runner from the
repository root. Use an empty output directory outside the checkout.

```sh
statechart docs/retire/RetireAbandon.tla --config docs/retire/RetireAbandon.cfg --view docs/retire/chart.mjs --out "$EVIDENCE_DIR"
```
