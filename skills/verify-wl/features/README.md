# Feature map

| Feature | User entry point | State in this verifier |
| --- | --- | --- |
| [Task finish lifecycle](task-finish-lifecycle.md) | Built `wl` binary (`begin`, `hold`, `release-hold`, `finish --retire`, `status`, `retire`) with `.lifecycle.toml` validation and `slot_cache` | Driven by `scripts/verify.py` |
| WSL offload (`where = "any"`) | `wl finish` with `offload` configured in `WL_CONFIG` | not-run. Covered by `wl/tests/finish.rs` (`any_steps_run_in_the_offload_directory_...` on Unix). |
| GitHub required checks | `wl finish` with `.github/required-checks.json` and `gh auth token` | not-run. Covered by `wl/tests/finish.rs` (`the_target_waits_for_required_checks_...` on Unix). |
| Merge conflict & `needs-action` recovery | `wl finish` on conflicting target change, then resolution commit and second `wl finish` | not-run. Covered by `wl/tests/finish.rs`. |
| Non-default `--target` and `--base` | `wl begin --target BRANCH --base REV` | not-run. Covered by `wl/tests/finish.rs`. |
| Windows toast notification | `wl` worker on `needs-action` | not-run |
| Foreground latency benchmark | `wl` release binary p90 under 50 ms | not-run. Covered by `wl/tests/latency.rs` (`--ignored`). |
| Concurrent finish & worker crash recovery | Multiple tasks finishing into one target or killed workers | not-run. Modeled by `skills/verify-finish-integration/`. |
| Other OS than the host running `verify.py` | `wl` on Windows vs Linux | not-run |
