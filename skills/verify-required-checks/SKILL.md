---
name: verify-required-checks
description: Model-check the required-check gates in wl finish when changing check-run selection, polling, or the transition into target push or integrated status.
license: MIT
---

# Verify required checks in wl finish

**OnlyRequiredChecksAdvance.** The latest matching app/name check must succeed on the topic commit before the target push and on the pushed commit before integration is recorded. Otherwise unrelated or superseded CI results can advance a finish.

**WorkflowFailureStopsNextPoll.** When a required check has no run, every check suite of its app has completed, and one of them failed without check runs, the poll that sees it must move the task to needs-action. This covers a workflow that GitHub could not start. Otherwise the old attempt waits six hours and a corrected commit cannot finish. [Issue #2](https://github.com/yasuyuki/workspace-lifecycle-native/issues/2) owns this acceptance.

## Run and read

Use [TLA+ Tools v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), SHA-256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`, and Java 11 or newer. Run from a scratch directory outside the checkout. Use `/statechart` on [RequiredChecks.tla](model/RequiredChecks.tla) with [chart.mjs](model/chart.mjs) and each config below. Use a fresh output directory per run. When `mise` is absent, set `TLA2TOOLS` to the jar and `JAVA` to the java binary.

| Config | Expected result at the sync commit |
| --- | --- |
| [RequiredChecks.cfg](model/RequiredChecks.cfg) | 32 states. Both rules hold. |
| [failed-suite.cfg](model/failed-suite.cfg) | Deliberate mutation: `ReadSuites = FALSE` treats a workflow that could not start as a missing run, as wl did before Issue #2. `WorkflowFailureStopsNextPoll` breaks in one step: the poll leaves the task waiting. |
| [wrong-app.cfg](model/wrong-app.cfg) | Deliberate mutation. `OnlyRequiredChecksAdvance` breaks when an unrelated app's success clears the topic gate. |

Reproduce each counterexample with the [CLI required-checks verification](../verify-wl/features/required-checks.md). The CLI test uses a local Git remote and serves synthetic GitHub responses to a real `wl finish`. For failed-suite, run its scenario with a build of a commit before the fix. That build must stay prepared, and the current build must pass. For the wrong-app source mutation, remove the app comparison in `check_states` on a disposable source copy, build there, and run the gate scenario with that binary. The topic must advance incorrectly when the unrelated success has the highest run ID. Use the unmodified build to confirm the gate passes. A fixture injecting an unexpected success can check the CLI detector itself.

Regenerate [the chart](../../docs/lifecycle-required-checks.mmd) from `RequiredChecks.cfg`. The chart folds repeated pending polls and terminal stuttering into their existing labels. Update a config expectation when the behavior changes, after confirming the real trace and rerunning the model. Advance the sync commit only after reconciling every affected row below.

## Coverage

Sync commit `3b9788ff152f9a15ccd5f20e447979d11142ceac`. All entry points below live in [wl/src/finish.rs](../../wl/src/finish.rs).

| Source | Model |
| --- | --- |
| `check_states` selects the greatest run ID for the required app/name, then examines status and conclusion. A run that has a conclusion and `completed_at` but another status is unfinalized ([#6](https://github.com/yasuyuki/workspace-lifecycle-native/issues/6)). When the check has no run, `unstarted_workflow` reads the commit's check suites of the same app. It fails the check, with the workflow run's URL, when they have all completed and one has the conclusion `failure` or `startup_failure` with no check runs. Suites of other apps are ignored. | `Replies`, `IsSuccess`, `Stops`. `missing` stands for no run while a suite of the app is unfinished or none failed without runs. `unstarted` stands for the failed suite without runs. The other replies stand for selected-run outcomes: only an unrelated success, a newer pending run, or an unfinalized run. |
| `wait_for_checks` succeeds on all required checks, errors on a failed or unfinalized check, and keeps polling on missing or incomplete runs. | `PollTopic`, `PollTarget`. |
| `push_target` waits for the topic SHA before pushing. `Worker::step` at `TargetPushed` waits for the pushed SHA. | `PushTarget` and the two separate gate states. |
| `work` stores `NeedsAction` after an error. `finish` accepts a new attempt after the old one ends. | Failure transitions and `Retry`. |

The model starts after preparation, with one required check, one worker, a separate topic and target result, and at most one corrected attempt after a topic failure. It represents a successful push and assumes the target stays put during that push. The six-hour deadline, API error/token retries, post-push retries, GitHub reruns, holds, queue locks, and real time are outside this model. It also leaves out a check suite of the app that GitHub keeps queued for good. Such a suite keeps the wait going until the deadline, as before. The [finish integration skill](../verify-finish-integration/SKILL.md) covers the shared integration worktree and queue.
