# Required checks during finish

The worker checks the topic commit before pushing the target. When the target
gets a new merge commit, the worker checks that SHA before reporting integrated.
See [finish design](../../../docs/design.md) and [Issue #2](https://github.com/yasuyuki/workspace-lifecycle-native/issues/2).

## Launch and doctor

Use Python 3.10+, Git, GitHub CLI, and an executable `wl` built for this host.
On Windows, use the CI artifact for the revision under test ([install](../../../README.md#install-on-windows)).
In a Linux development session build with `cargo build --locked -p wl`, then
use `target/debug/wl`. Record the full build commit and CI run or build command.

Open a terminal at the source checkout. In the command below, substitute the
binary, its build commit and source, an absolute private evidence directory
outside the checkout, and the driving agent's name and model.

```sh
python -B skills/verify-wl/scripts/verify_required_checks.py \
  --wl "$WL_BINARY" --revision "$BUILD_COMMIT" --binary-source "$BUILD_SOURCE" \
  --evidence-root "$EVIDENCE_ROOT" --agent "$DRIVING_AGENT"
```

Use the host's Python executable. On PowerShell, use one line or pass the
arguments as an array. Add `--doctor-only` to confirm the binary identity,
Git and GitHub CLI, evidence path, loopback listener, and disposable bare
remote. This records `not-run` and performs cleanup without starting `wl`.
The helper does not build or install the binary.

The helper confines Git transport to local files and maps the fake GitHub
origin to the disposable bare repository. It points `WL_GITHUB_API` at the
run's loopback port, gives `gh auth token` a synthetic environment token,
and supplies the same local preflight fixture as the task-lifecycle verifier.
The API serves the commit's check runs, its check suites, and its workflow
runs. Outside the failed-suite scenario, the Actions suite stays queued
without runs, so a commit with no matching run keeps waiting. Another app's
suite stays queued in every scenario, as GitHub leaves some of them.

## Drive and judge

| Operation | Required observation |
| --- | --- |
| `wl begin alpha`, add a required check, independently advance the bare repo's `main`, then `wl finish alpha`. | The accepted topic SHA is pushed to `alpha`. An unrelated app's success and an older same-app success leave the task prepared and `main` unchanged because the newest matching run is pending. |
| Serve a failed matching run. | `wl status alpha` reports needs-action with the run URL. `main` stays unchanged. |
| Commit a fix and run `wl finish alpha` again. | Attempt 2 accepts the corrected SHA. Its successful check permits the target push. The bare target points to a distinct merge SHA that contains the patch. |
| Serve success for the pushed merge SHA. | The worker stays target-pushed until it sees that SHA's success, then reports integrated. The target ref still points to the checked SHA. |

Add `--scenario failed-suite` for Issue #2. The Actions suite completes as
failed with no check runs, like a workflow file that GitHub rejects. The task
must reach needs-action at the next poll with the workflow run's URL in the
detail, and a corrected commit must get attempt 2. A build from before the
fix stays prepared and refuses the corrected commit. The helper records that
as `fail`, then supplies a failing check run to end the scratch worker.

Add `--scenario unexpected-success` to challenge the detector with a success
where the gate scenario requires pending. It must fail and still clean up.

## Evidence and cleanup

Read the printed `result.json` after the helper exits. It contains binary and
verifier identities and hashes, commands, API replies, criteria and observations.
The helper also retains the synthetic preflight log. Count a pass only with
`status: "pass"`, `cleanup.ok: true`, and `evidence_retained: true`.
Exit codes are 0 for pass, 1 for fail, 2 for blocked, and 3 for not-run.

The helper waits for the owned worker, closes its loopback API, and removes the
run's scratch repository. If the worker is waiting when a check fails, the
helper supplies a failing required run so it can exit. When cleanup cannot
confirm completion, retain the scratch path in `result.json` for diagnosis.

Live GitHub, WSL offload, Windows toast delivery, and `CheckState::Unfinalized`
([#6](https://github.com/yasuyuki/workspace-lifecycle-native/issues/6), covered by `wl/tests/finish.rs`) remain outside this recipe. The [formal skill](../../verify-required-checks/SKILL.md) covers bounded
required-check transitions and the failed-suite mutation.
