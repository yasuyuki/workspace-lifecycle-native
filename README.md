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
  "offload": { "prefix": ["wsl.exe", "-d", "ubuntu-26-development", "-u", "<offload user>", "--"], "root": "/home/<offload user>/wl-offload" }
}
```
