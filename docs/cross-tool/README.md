# Cross-tool lifecycle

[Statechart](statechart.mmd) · [TLA+ spec](CrossTool.tla) · [TLC configuration](CrossTool.cfg)

The model follows one task across `wl`, legacy workspace-lifecycle,
agent-runtime, GitHub, validation, and its Issue. The modeled legacy task
stands for the check that **all repositories** have drained their legacy tasks.
GitHub checks and validation are abstracted as success or failure. Slot
selection order is exercised by the CLI test `begin_reuses_the_slot_retired_earliest`.

TLC checks four invariants: the legacy install outlives its dependents,
a session can start, an Issue closes only after Default integration, and a
session that can still work in a slot never survives slot reuse. Target
integration and Default integration can occur separately. The repair
decisions are recorded in [Issue #5](https://github.com/yasuyuki/workspace-lifecycle-native/issues/5)
and [ADR 0016](../adr/0016-legacy-install-dependencies.md).

Regenerate the chart with the statechart skill and its TLC runner from the
repository root. Use an empty output directory outside the checkout.

```sh
statechart docs/cross-tool/CrossTool.tla --config docs/cross-tool/CrossTool.cfg --view docs/cross-tool/chart.mjs --out "$EVIDENCE_DIR"
```
