# workspace-lifecycle-native

Native replacement for workspace-lifecycle, designed for Windows process-start cost.

`probe/` and `.github/workflows/probe.yml` measure process start, git and in-process gix reads on GitHub runners. The numbers drive the design.

## Install on Windows

Each push builds `wl.exe` in the `ci` workflow and keeps it as the artifact `wl-windows-x64`. To take the build of an exact commit:

```
gh run download --repo yasuyuki/workspace-lifecycle-native --name wl-windows-x64 <run id of the ci workflow for that commit>
```

`wl` reads the push preflight and the WSL offload from `%LOCALAPPDATA%\wl\config.json`:

```json
{
  "preflight": ["<python>", "-m", "workspace_lifecycle.push", "{repo}", "--policy", "<private policy path>"],
  "offload": { "prefix": ["wsl.exe", "-d", "<distro>", "-u", "<offload user>", "-e"], "root": "/home/<offload user>/wl-offload" }
}
```

End the offload prefix with `-e`, not `--`. After `--`, `wsl.exe` passes the command line to the distro's shell, which parses it again and drops the quoted arguments of `sh -c`.

## Verification

- Statecharts: [task lifecycle](docs/lifecycle-statechart.mmd), [finish and target integration](docs/lifecycle-finish-integration.mmd), and [slot and claim lifecycle](docs/lifecycle-slot-claims.mmd).
- Live CLI verification skill: [skills/verify-wl/SKILL.md](skills/verify-wl/SKILL.md) (drives the built `wl` binary with disposable repos, retained evidence, and cleanup).
- Formal verification skills: [skills/verify-finish-integration/SKILL.md](skills/verify-finish-integration/SKILL.md) (`wl finish` and target integration) and [skills/verify-slot-claims/SKILL.md](skills/verify-slot-claims/SKILL.md) (`wl begin`, `wl retire`, and `__prepare-slot` slot claims).
