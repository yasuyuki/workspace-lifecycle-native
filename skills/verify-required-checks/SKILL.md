---
name: verify-required-checks
description: Model-check the required-check gates in wl finish when changing check-run selection, polling, or the transition into target push or integrated status.
license: MIT
---

# Verify required checks in wl finish

**OnlyRequiredChecksAdvance.** The latest matching app/name check must succeed on the topic commit before the target push and on the pushed commit before integration is recorded. Otherwise unrelated or superseded CI results can advance a finish.

**WorkflowFailureStopsNextPoll.** A completed failed workflow with no check runs should move the task to needs-action at the next poll. Otherwise a corrected commit cannot finish while the old attempt waits. [Issue #2](https://github.com/yasuyuki/workspace-lifecycle-native/issues/2) owns this acceptance.

## Run and read

Use [TLA+ Tools v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), SHA-256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`, and Java 11 or newer. Run from a scratch directory outside the checkout. Use `/statechart` on [RequiredChecks.tla](model/RequiredChecks.tla) with [chart.mjs](model/chart.mjs) and each config below. Use a fresh output directory per run.

| Config | Expected result at the sync commit |
| --- | --- |
| [RequiredChecks.cfg](model/RequiredChecks.cfg) | 28 states. Both rules hold for check-run responses. |
| [failed-suite.cfg](model/failed-suite.cfg) | `WorkflowFailureStopsNextPoll` breaks. One poll sees no matching run and leaves the task prepared. |
| [wrong-app.cfg](model/wrong-app.cfg) | Deliberate mutation. `OnlyRequiredChecksAdvance` breaks when an unrelated app's success clears the topic gate. |

Keep the counterexample while reproducing it with the [CLI required-checks verification](../verify-wl/features/required-checks.md). The CLI test uses a local Git remote and serves synthetic GitHub responses to a real `wl finish`. For the wrong-app source mutation, remove the app comparison in `check_states` on a disposable source copy, build there, and run the gate scenario with that binary. The topic must advance incorrectly when the unrelated success has the highest run ID. Use the unmodified build to confirm the gate passes. A fixture injecting an unexpected success can check the CLI detector itself.

Regenerate [the chart](../../docs/lifecycle-required-checks.mmd) from `failed-suite.cfg` while the finding is present. The chart folds repeated pending polls and terminal stuttering into their existing labels. Update the config expectation when the failed-suite behavior changes, after confirming the real trace and rerunning the model. Advance the sync commit only after reconciling every affected row below.

## Coverage

Sync commit `0ea2eb4c86a18680acbe0097b0d5d7b1d86f5ec2`. All entry points below live in [wl/src/finish.rs](../../wl/src/finish.rs).

| Source | Model |
| --- | --- |
| `check_states` selects the greatest run ID for the required app/name, then examines status and conclusion. A run that has a conclusion and `completed_at` but another status is unfinalized ([#6](https://github.com/yasuyuki/workspace-lifecycle-native/issues/6)). | `Replies`, `IsSuccess`, `Stops`. The replies stand for selected-run outcomes, including only an unrelated success, a newer pending run, or an unfinalized run. |
| `wait_for_checks` succeeds on all required checks, errors on a failed or unfinalized check, and keeps polling on missing or incomplete runs. | `PollTopic`, `PollTarget`, `PollEmptyFailedSuite`. The latter represents a failed workflow as seen from the existing check-runs query. |
| `push_target` waits for the topic SHA before pushing. `Worker::step` at `TargetPushed` waits for the pushed SHA. | `PushTarget` and the two separate gate states. |
| `work` stores `NeedsAction` after an error. `finish` accepts a new attempt after the old one ends. | Failure transitions and `Retry`. |

The model starts after preparation, with one required check, one worker, a separate topic and target result, and at most one corrected attempt after a topic failure. It represents a successful push and assumes the target stays put during that push. The failed-suite path ends at the missed poll. The six-hour deadline, API error/token retries, post-push retries, GitHub reruns, holds, queue locks, and real time are outside this model. The [finish integration skill](../verify-finish-integration/SKILL.md) covers the shared integration worktree and queue.
