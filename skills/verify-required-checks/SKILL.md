---
name: verify-required-checks
description: Model-check the required-check gates in wl finish when changing check-run selection, polling, or the transition into target push or integrated status.
license: MIT
---

# Verify required checks in wl finish

**OnlyRequiredChecksAdvance.** The latest matching app/name check must succeed on the topic commit before the target push and on the pushed commit before integration is recorded. Otherwise unrelated or superseded CI results can advance a finish.

**WorkflowFailureStopsNextPoll.** When a required check has no run, every check suite of its app has completed, and one of them failed without check runs, the poll that sees it must move the task to needs-action. This covers a workflow that GitHub could not start. Otherwise the old attempt waits six hours and a corrected commit cannot finish. [Issue #2](https://github.com/yasuyuki/workspace-lifecycle-native/issues/2) owns this acceptance.

**RefusedPushStops.** When the remote refuses the target push and the target has not moved, such as branch protection rejecting it with GH006, the finish must stop in needs-action with the remote's reason. Otherwise the worker merges, validates and pushes again without end, and the reason never reaches the person. [Issue #9](https://github.com/yasuyuki/workspace-lifecycle-native/issues/9) owns this acceptance.

## Run and read

Use [TLA+ Tools v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), SHA-256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`, and Java 11 or newer. Run from a scratch directory outside the checkout. Use `/statechart` on [RequiredChecks.tla](model/RequiredChecks.tla) with [chart.mjs](model/chart.mjs) and each config below. Use a fresh output directory per run. When `mise` is absent, set `TLA2TOOLS` to the jar and `JAVA` to the java binary.

| Config | Expected result at the sync commit |
| --- | --- |
| [RequiredChecks.cfg](model/RequiredChecks.cfg) | 50 states. All three rules hold. |
| [failed-suite.cfg](model/failed-suite.cfg) | Deliberate mutation: `ReadSuites = FALSE` treats a workflow that could not start as a missing run, as wl did before Issue #2. `WorkflowFailureStopsNextPoll` breaks in one step: the poll leaves the task waiting. |
| [wrong-app.cfg](model/wrong-app.cfg) | Deliberate mutation. `OnlyRequiredChecksAdvance` breaks when an unrelated app's success clears the topic gate. |
| [push-refusal-loop.cfg](model/push-refusal-loop.cfg) | Deliberate mutation: `StopOnRefusal = FALSE` sends a refused push back to preparation, as wl did before Issue #9. `RefusedPushStops` breaks in two steps: the topic check passes, then the remote refuses the push. |

Reproduce each counterexample with the [CLI required-checks verification](../verify-wl/features/required-checks.md). The CLI test uses a local Git remote and serves synthetic GitHub responses to a real `wl finish`. For failed-suite, run its scenario with a build of a commit before the fix. That build must stay prepared, and the current build must pass. For the wrong-app source mutation, remove the app comparison in `check_states` on a disposable source copy, build there, and run the gate scenario with that binary. The topic must advance incorrectly when the unrelated success has the highest run ID. Use the unmodified build to confirm the gate passes. A fixture injecting an unexpected success can check the CLI detector itself. For push-refusal-loop, run `cargo test --manifest-path wl/Cargo.toml --test finish a_target_push_the_remote_refuses` on a build of a commit before the fix: the finish stays `topic-pushed` and the test times out. The current build must pass.

Regenerate [the chart](../../docs/lifecycle-required-checks.mmd) from `RequiredChecks.cfg`. The chart folds repeated pending polls and terminal stuttering into their existing labels. Update a config expectation when the behavior changes, after confirming the real trace and rerunning the model. Advance the sync commit only after reconciling every affected row below.

## Coverage

Sync commit `ec4371ef11983f5663be60397a1f97b900ee0abe`, with the Issue #9 change to `push_target` on top. All entry points below live in [wl/src/finish.rs](../../wl/src/finish.rs).

| Source | Model |
| --- | --- |
| `check_states` selects the greatest run ID for the required app/name, then examines status and conclusion. A run that has a conclusion and `completed_at` but another status is unfinalized ([#6](https://github.com/yasuyuki/workspace-lifecycle-native/issues/6)). When the check has no run, `unstarted_workflow` reads the commit's check suites of the same app. It fails the check, with the workflow run's URL, when they have all completed and one has the conclusion `failure` or `startup_failure` with no check runs. Suites of other apps are ignored. | `Replies`, `IsSuccess`, `Stops`. `missing` stands for no run while a suite of the app is unfinished or none failed without runs. `unstarted` stands for the failed suite without runs. The other replies stand for selected-run outcomes: only an unrelated success, a newer pending run, or an unfinalized run. |
| `wait_for_checks` succeeds on all required checks, errors on a failed or unfinalized check, and keeps polling on missing or incomplete runs. | `PollTopic`, `PollTarget`. |
| `push_target` waits for the topic SHA before pushing. `Worker::step` at `TargetPushed` waits for the pushed SHA. | `PushTarget` and the two separate gate states. |
| `push_target` fetches the target again after a failed push. A target that moved returns to `TopicPushed`, which merges and validates again. An unmoved target returns the push error, with git's stderr, as needs-action. | `PushTarget` outcomes `target moved` and `refused`. |
| `work` stores `NeedsAction` after an error. `finish` accepts a new attempt after the old one ends. | Failure transitions and `Retry`. |

The model starts after preparation, with one required check, one worker, a separate topic and target result, and at most one corrected attempt after a topic failure or a refused push. It represents three push outcomes: accepted, refused with the target unmoved, and refused because the target moved first. The six-hour deadline, API error/token retries, a fetch that fails after a refused push, GitHub reruns, holds, queue locks, and real time are outside this model. It also leaves out a check suite of the app that GitHub keeps queued for good. Such a suite keeps the wait going until the deadline, as before. The [finish integration skill](../verify-finish-integration/SKILL.md) covers the shared integration worktree and queue.
