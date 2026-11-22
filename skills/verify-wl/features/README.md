# Feature map

| Feature | User entry point | State in this verifier |
| --- | --- | --- |
| [Task finish lifecycle](task-finish-lifecycle.md) | Built `wl` binary (`begin`, `hold`, `release-hold`, `finish --retire`, `status [TASK] [--finish ID]`, `retire`, `retire --abandon --decision`) with `.lifecycle.toml` validation and `slot_cache` | Driven by `scripts/verify.py` |
| Push decision | `wl preflight [REPO] [--user-intent] [--temporary] [--oss] [--policy]` | Driven by `scripts/verify.py` on the local origin (ask, push and hold exit statuses). The GitHub metadata path is covered by `wl/src/push.rs` tests and the legacy comparison in workspace-lifecycle-native#8. |
| WSL offload (`where = "any"`) | `wl finish` with `offload` configured in `WL_CONFIG` | not-run. Covered by `wl/tests/finish.rs` (`any_steps_run_in_the_offload_directory_...` on Unix). |
| [Required checks](required-checks.md) | `wl finish` with `.github/required-checks.json` and `gh auth token` | Driven against a loopback API by `scripts/verify_required_checks.py`. The real GitHub service and `CheckState::Unfinalized` (#6, covered by `wl/tests/finish.rs`) are not-run. |
| Merge conflict & `needs-action` recovery | `wl finish` on conflicting target change, then resolution commit and second `wl finish` | not-run. Covered by `wl/tests/finish.rs`. |
| Non-default `--target`, `--base`, and `--branch` | `wl begin --target BRANCH --base REV --branch NAME` | not-run. `--target` is covered by `wl/tests/finish.rs` and invalid `--base` failure cleanup by `wl/tests/cli.rs`; happy-path `--base` and `--branch` are untested. |
| Windows toast notification | `wl` worker on `needs-action` | not-run |
| Foreground latency benchmark | `wl` release binary p90 under 50 ms | not-run. Covered by `wl/tests/latency.rs` (`--ignored`). |
| Concurrent finish & worker crash recovery | Multiple tasks finishing into one target or killed workers | not-run. Covered by `wl/tests/finish.rs` and modeled by `skills/verify-finish-integration/`. |
| Slot/claim concurrency & orphan/#prepare claim recovery | Concurrent `wl begin` / `wl retire`, failed `wl begin`, or crashed prepare/retire | not-run. Covered by `wl/tests/cli.rs` and modeled by `skills/verify-slot-claims/`. |
| `wl finish` without `--retire` & uncommitted notice | `wl finish` on clean or dirty slot without `--retire`, and re-finish on `Integrated` | not-run. Uncommitted notice without `--retire` is covered by `wl/tests/finish.rs`; re-finish on `Integrated` is implemented in `wl/src/finish.rs` without a dedicated test. |
| Other OS than the host running `verify.py` | `wl` on Windows vs Linux | not-run |
