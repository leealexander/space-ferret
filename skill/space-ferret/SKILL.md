---
name: space-ferret
description: Find out what is using disk space and reclaim it safely with the `space-ferret` CLI, a fast, read-only disk-usage scanner that tags caches, build output, dependency folders, temp files, VM/Docker disks and system leftovers with a safety level and the right cleanup command. Use when the user asks why a drive is full, what is taking up space, wants a disk usage breakdown, or wants to free space or clean up caches, node_modules, build artifacts, Docker/WSL/Podman disks, temp files or logs, on Windows or Linux.
---

# Disk cleanup with space-ferret

`space-ferret` never deletes anything, so scanning is always safe. Deleting is the part that needs care.

## 1. Make sure it's installed

Run `space-ferret --version`. If it isn't found, install the latest release.

Linux (x86_64 or aarch64, static binary):
```sh
case "$(uname -m)" in aarch64|arm64) ARCH=aarch64 ;; *) ARCH=x86_64 ;; esac
mkdir -p ~/.local/bin
curl -fsSL "https://github.com/leealexander/space-ferret/releases/latest/download/space-ferret-linux-$ARCH" -o ~/.local/bin/space-ferret
chmod +x ~/.local/bin/space-ferret
```

Windows (PowerShell, no admin needed; the x64 build also runs on Windows on ARM):
```powershell
$ProgressPreference = 'SilentlyContinue'   # Windows PowerShell 5.1 downloads crawl with the progress bar on
$dir = "$env:LOCALAPPDATA\Programs\space-ferret"
New-Item -ItemType Directory -Force $dir | Out-Null
Invoke-WebRequest "https://github.com/leealexander/space-ferret/releases/latest/download/space-ferret-windows-x86_64.exe" -OutFile "$dir\space-ferret.exe"
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $dir) { [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User') }
$env:Path += ";$dir"
```

If `~/.local/bin` (Linux) isn't on PATH in the current shell, call the binary by its full path.

## 2. Survey

```
space-ferret --drives --json        # which volume is full, and how badly
space-ferret <path> --json -q       # full report for a folder or drive (-q = no progress line)
```

Pick the path from the request: the home folder for "my stuff", `C:\` or `/` for a whole drive, or a project folder. On Linux, scanning `/` stays on the root filesystem; other mounts are listed in `skipped_mounts`. Without admin/sudo some folders can't be read. `unreadable.count` says how many; mention it if it's large, but don't elevate without asking.

Timing: about 250k files/s with a warm cache. The first scan of a big, cold drive can take 1–2 minutes, so give whole-drive scans a 10-minute timeout.

Flags: `-d N` tree depth (3) · `-n N` rows per list (15) · `-p P` hide tree entries under P% (1) · `-m SIZE` smallest folder to flag (10M) · `-f SIZE` smallest file to flag (100M) · `-e NAME|PATH` exclude (repeatable) · `--cross-mounts` (Unix).

## 3. Read the report

- `volume`: `free_h` and `used_pct` show how urgent this is.
- `tree`: nested breakdown. Each node has `path`, `size_h`, `pct` of total, and `age_days` (days since anything inside changed, so a high number means stale). It also has `own_files` (files directly in the folder) and `smaller_dirs` (children under the % threshold, folded together).
- `largest_files`, `file_types`: the biggest files and the size per extension.
- `cleanup.groups[]`: candidates grouped by kind, biggest first. Each group has `id`, `label`, `safety`, `hint` (the proper cleanup command), `count`, `size_h`, and `items[]` (`path`, `size_h`, `age_days`). `not_listed` summarizes items beyond `--top`.
- `cleanup.totals`: bytes per safety level.
- `size_basis`: caveats. Windows reports logical sizes with cloud-only placeholders excluded (`cloud_only`), and WinSxS is inflated by hardlinks. Linux reports allocated blocks and counts each hardlink once.

| safety | meaning | what you may do |
|---|---|---|
| `safe` | cache/temp; the app recreates it | propose; clean after the user approves the plan |
| `regen` | build output / dependencies; rebuild or reinstall restores it | propose, starting with stale ones (`age_days` > 60); confirm first |
| `review` | personal files, downloads, models, VM disks, archives, git repos | list them and let the user choose item by item |
| `system` | only via the OS/tool command in `hint` | give the command; run only with explicit OK (often needs admin/sudo) |
| `keep` | needed by the system (installer caches, pagefile) | never delete; mention only to explain where space went |

## 4. Drill down

The tree stops at depth 3 and hides entries under 1%. If a big folder isn't explained by candidates, scan it directly (`space-ferret "<path>" --json -q`, fast on a warm cache) until the space is accounted for. A large folder with no candidate is usually user data or installed software. Describe it and suggest the proper uninstall route; never propose deleting files out of it.

## 5. Present a plan before touching anything

Show a table, biggest win first, with: what, path, size, safety, how (the exact command), and the cost of cleaning it (e.g. "next build is slower", "models must be re-downloaded", "removes stopped containers"). Then give:
- the total reclaimable without losing anything irreplaceable (`safe` + `regen`),
- the `review` items worth a look, with the large stale ones called out,
- the biggest consumers that are *not* cleanup candidates.

## 6. Clean up only after explicit confirmation

- Delete only the specific items the user approved. Approving one item doesn't cover others.
- Prefer the tool's own command from `hint` over deleting files: `npm cache clean --force`, `pip cache purge`, `dotnet nuget locals all --clear`, `cargo clean`, `docker system prune -a`, `sudo journalctl --vacuum-size=200M`, `DISM /Online /Cleanup-Image /StartComponentCleanup`.
- Plain deletion uses the exact path space-ferret reported. Never use wildcards near a drive root, and never delete the parent of a candidate.
  - PowerShell: `Remove-Item -LiteralPath "<path>" -Recurse -Force`. To delete contents only (Temp): `Get-ChildItem -LiteralPath "<path>" -Force | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue`
  - Linux: `rm -rf -- "<path>"`
- Close the owning app first (browser, IDE, Docker/Podman, Ollama) for app caches and in-use data.
- Never touch `keep` items. Never hand-delete inside Program Files, `C:\Windows`, `/usr`, `/etc` or `/var/lib`; use the hint's command instead.
- Virtual disks (`.vhdx`, `.qcow2`, Docker/Podman/WSL) don't shrink when you delete inside them. Free space inside first (`docker system prune -a`, `podman system prune -a`, cleanup inside the distro). Then stop the VM (`wsl --shutdown`, `podman machine stop`) and compact the disk: `Optimize-VHD -Path <vhdx> -Mode Full` (admin, Hyper-V module) or `diskpart` → `select vdisk file="<vhdx>"` → `attach vdisk readonly` → `compact vdisk` → `detach vdisk`. For WSL distros, `wsl --manage <distro> --set-sparse true` makes the disk shrink automatically.
- `hiberfil.sys`: `powercfg /h off` frees it, but it disables hibernate and Fast Startup. Ask first.
- Emptying the Recycle Bin / Trash is the last step, and only if the user agrees.
- Afterwards run `space-ferret --drives` again and report free space before and after.
