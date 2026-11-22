# workspace-lifecycle-native

Native replacement for workspace-lifecycle, designed for Windows process-start cost.

`probe/` and `.github/workflows/probe.yml` measure process start, git and in-process gix reads on GitHub runners. The numbers drive the design.

## Install on Windows

Each push builds `wl.exe` in the `ci` workflow and keeps it as the artifact `wl-windows-x64`. To take the build of an exact commit:

```
gh run download --repo yasuyuki/workspace-lifecycle-native --name wl-windows-x64 <run id of the ci workflow for that commit>
```

`wl` reads the push policy and the WSL offload from `%LOCALAPPDATA%\wl\config.json`:

```json
{
  "push_policy": "<private policy path>",
  "offload": { "prefix": ["wsl.exe", "-d", "<distro>", "-u", "<offload user>", "-e"], "root": "/home/<offload user>/wl-offload" }
}
```

End the offload prefix with `-e`, not `--`. After `--`, `wsl.exe` passes the command line to the distro's shell, which parses it again and drops the quoted arguments of `sh -c`.

## Verification

- [Cross-tool statechart and TLA+ model](docs/cross-tool/README.md) for retirement, Issue closing, and legacy install dependencies.
- [Retirement and abandonment statechart and TLA+ model](docs/retire/README.md) for `retire --abandon`.
- Statecharts: [task lifecycle](docs/lifecycle-statechart.mmd), [finish and target integration](docs/lifecycle-finish-integration.mmd), [slot and claim lifecycle](docs/lifecycle-slot-claims.mmd), and [required checks](docs/lifecycle-required-checks.mmd).
- Live CLI verification skill: [skills/verify-wl/SKILL.md](skills/verify-wl/SKILL.md) (drives the built `wl` binary with disposable repos, retained evidence, and cleanup).
- Formal verification skills: [finish integration](skills/verify-finish-integration/SKILL.md), [slot claims](skills/verify-slot-claims/SKILL.md), and [required checks](skills/verify-required-checks/SKILL.md).
