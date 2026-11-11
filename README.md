# space-ferret

**Ferrets out where your disk space went.** A fast, parallel, read-only disk-usage scanner for Windows and Linux. It shows a size breakdown and the biggest files, and flags what you can clean up. Each candidate gets a safety level and the proper command to clean it.

It ships as a single static binary under 1 MB, with no runtime and no dependencies. It works well in a terminal and is designed to be driven by AI agents (Claude Code, Codex, ...) through `--json` and the bundled [skill](skill/space-ferret/SKILL.md).

```
$ space-ferret ~ -d 1                       (example, abridged)

 space-ferret  /home/alex
 total   212 GB on disk
 scan    1,204,331 files · 160,212 folders · 3.10s on 16 threads
 volume  / ext4  431 GB of 468 GB used (92%) · 37.0 GB free

 BREAKDOWN  depth 1, entries ≥ 1% of total
     212 GB 100.0% ██████████ /home/alex
    88.1 GB  41.6% ████▏      ├─ projects  3d
    41.0 GB  19.3% █▉         ├─ .cache  today
    30.2 GB  14.2% █▍         ├─ .local  12d
    ...

 CLEANUP CANDIDATES  folders ≥ 10.0 MB, files ≥ 100 MB; read-only scan, nothing was deleted
  safe 24.3 GB  ·  regen 31.0 GB  ·  review 52.4 GB  ·  system 6.1 GB

  regen    19.8 GB  node_modules (JS dependencies)  ×41
                    → delete, restore with npm/pnpm/yarn install; stale projects first
                       2.10 GB    14mo  /home/alex/projects/old-dashboard/node_modules
                    ...
  system   6.10 GB  Docker / Podman storage
                    → docker system prune -a (add --volumes to drop unused volumes)
```

---

## For AI agents: install and use

If a user asks you to set up space-ferret, or you have its skill and the binary is missing, follow these steps. The download URLs always point at the latest release.

| Platform | Asset |
|---|---|
| Linux x86_64 (static, any distro) | `space-ferret-linux-x86_64` |
| Linux aarch64 (static, any distro) | `space-ferret-linux-aarch64` |
| Windows x64 (also runs on Windows on ARM) | `space-ferret-windows-x86_64.exe` |
| Agent skill | `SKILL.md` |
| Checksums | `SHA256SUMS.txt` |

Base URL: `https://github.com/leealexander/space-ferret/releases/latest/download/<asset>`

### Step 1: install the binary

**Linux** (no root needed):
```sh
case "$(uname -m)" in aarch64|arm64) ARCH=aarch64 ;; *) ARCH=x86_64 ;; esac
mkdir -p ~/.local/bin
curl -fsSL "https://github.com/leealexander/space-ferret/releases/latest/download/space-ferret-linux-$ARCH" -o ~/.local/bin/space-ferret
chmod +x ~/.local/bin/space-ferret
~/.local/bin/space-ferret --version
```
If `~/.local/bin` isn't on `PATH`, use the full path, or add `export PATH="$HOME/.local/bin:$PATH"` to the shell profile with the user's OK.

**Windows** (PowerShell, no admin needed; installs into the user profile and adds it to the user PATH):
```powershell
$ProgressPreference = 'SilentlyContinue'   # Windows PowerShell 5.1 downloads crawl with the progress bar on
$dir = "$env:LOCALAPPDATA\Programs\space-ferret"
New-Item -ItemType Directory -Force $dir | Out-Null
Invoke-WebRequest "https://github.com/leealexander/space-ferret/releases/latest/download/space-ferret-windows-x86_64.exe" -OutFile "$dir\space-ferret.exe"
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $dir) { [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User') }
$env:Path += ";$dir"
space-ferret --version
```

**Optional: verify the checksum.**
```sh
# Linux
curl -fsSL https://github.com/leealexander/space-ferret/releases/latest/download/SHA256SUMS.txt \
  | awk -v f="space-ferret-linux-$ARCH" '$2 == f {print $1 "  '"$HOME"'/.local/bin/space-ferret"}' | sha256sum -c -
```
```powershell
# Windows
$sums = "$env:TEMP\space-ferret-SHA256SUMS.txt"
Invoke-WebRequest "https://github.com/leealexander/space-ferret/releases/latest/download/SHA256SUMS.txt" -OutFile $sums
$want = (Select-String -Path $sums -Pattern 'space-ferret-windows-x86_64\.exe').Line.Split(' ')[0]
if ((Get-FileHash "$dir\space-ferret.exe").Hash -eq $want) { 'checksum OK' } else { 'CHECKSUM MISMATCH' }
```

### Step 2: install the skill

The skill tells the agent how to survey, read the JSON, drill down, present a cleanup plan grouped by safety, and run only the deletions the user approves.

| Agent | Skill location |
|---|---|
| Claude Code | `~/.claude/skills/space-ferret/SKILL.md` |
| OpenAI Codex | `~/.codex/skills/space-ferret/SKILL.md` |
| Other agents | add the contents of `SKILL.md` to `AGENTS.md`, rules, or the system prompt |

```sh
# Linux / macOS shell. Change .claude to .codex for Codex.
mkdir -p ~/.claude/skills/space-ferret
curl -fsSL https://github.com/leealexander/space-ferret/releases/latest/download/SKILL.md -o ~/.claude/skills/space-ferret/SKILL.md
```
```powershell
# Windows PowerShell. Change .claude to .codex for Codex.
New-Item -ItemType Directory -Force "$HOME\.claude\skills\space-ferret" | Out-Null
Invoke-WebRequest "https://github.com/leealexander/space-ferret/releases/latest/download/SKILL.md" -OutFile "$HOME\.claude\skills\space-ferret\SKILL.md"
```
Skills are loaded when a session starts, so start a new session to pick it up.

### Step 3: use it

```sh
space-ferret --drives --json       # which volume is full?
space-ferret <path> --json -q      # full report: tree, largest files, file types, cleanup candidates
```

Ground rules (the skill covers them in detail):

- Scanning is read-only and always safe.
- Delete nothing without the user's explicit approval of the specific items.
- Prefer each candidate's `hint` command (e.g. `npm cache clean --force`, `docker system prune`) over deleting files.
- Never delete `keep` items.

---

## Install (humans)

Download the binary for your platform from [Releases](https://github.com/leealexander/space-ferret/releases/latest) and put it on your `PATH`. The commands above do exactly that. Or build from source:

```
cargo install --git https://github.com/leealexander/space-ferret
```

## Usage

```
space-ferret [OPTIONS] [PATH]      scan PATH (default: current directory)
space-ferret --drives              list volumes with size / free space

-j, --json                JSON report (for scripts and AI agents)
-d, --depth N             tree depth to show                      [3]
-n, --top N               rows per list (files, types, candidates) [15]
-p, --min-pct P           hide tree entries below P% of total      [1]
-m, --min-size SIZE       smallest folder to flag for cleanup      [10M]
-f, --min-file SIZE       smallest single file to flag for cleanup [100M]
-e, --exclude NAME|PATH   skip folders by name or path (repeatable)
    --cross-mounts        descend into other filesystems (Unix; default stays on one, like du -x)
-t, --threads N           worker threads                           [2x cores]
-q, --quiet               no progress line
    --no-color            plain output (NO_COLOR also works)
```

Examples: `space-ferret ~` · `space-ferret C:\ -d 2` (elevated for complete numbers) · `space-ferret / --json -q` · `space-ferret . -e node_modules`

## What it finds

Every cleanup candidate gets one of five safety levels:

| safety | meaning |
|---|---|
| `safe`   | cache / temp data; the owning app recreates it |
| `regen`  | build output or dependencies; a rebuild/reinstall restores it |
| `review` | personal data, downloads, models, VM disks, archives; ask before deleting |
| `system` | clean only with the OS/tool command shown (DISM, apt clean, journalctl, docker prune...) |
| `keep`   | needed by the system (installer caches, pagefile); don't touch |

What it recognises:

- **Package caches:** npm, yarn, pnpm, bun, corepack, pip, uv, conda, NuGet, Gradle, Maven, Cargo, Go.
- **Project output:** `node_modules`, Python venvs (`pyvenv.cfg`), `__pycache__`, Rust `target/` (next to `Cargo.toml`), .NET `bin/obj` (next to `*.csproj`), `build/`/`dist/` (with a build file next to them), `.next`, `.gradle`, `.terraform`, Unity/Unreal caches, and anything tagged with `CACHEDIR.TAG`.
- **Tooling:** Playwright browsers, Hugging Face / Ollama / LM Studio models, Android emulator images, editor extensions, minikube, old nvm/pyenv/uv runtimes.
- **Containers and VMs:** Docker/Podman storage, WSL/VM disks (`.vhdx`, `.qcow2`...).
- **Windows:** Temp, Recycle Bin, WinSxS, Windows Update cache, `Windows.old`, crash dumps, shader caches, thumbnail cache, browser and app caches, the Visual Studio installer cache, hiberfil/pagefile.
- **Linux:** apt/dnf/pacman caches, the systemd journal, snap revisions, flatpak, `~/.cache/*`, Trash.
- **Large files:** ISOs, installers, archives, logs, backups.

Guard rails keep it from suggesting anything harmful:

- Symlinks and junctions are never followed.
- Generic names like `cache`/`logs` are ignored inside source repos.
- Project rules don't fire under Program Files, `C:\Windows`, `/usr`, `/var/lib`, global npm, nvm and similar.
- A stray `package.json` in your home folder doesn't turn `~/.cache` into "build output".
- Candidates are never nested, so the totals don't double count.

How sizes are measured:

- **Windows:** logical size. Cloud-only placeholders (OneDrive, Google Drive streaming) are excluded and reported separately.
- **Linux:** allocated blocks, with each hardlinked file counted once (same as `du`).

Performance: about 250k files/s with a warm cache, on 2× cores worth of threads.

## Building

```
cargo build --release
# static Linux binaries from any OS, no C toolchain (rust-lld, see .cargo/config.toml):
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

Releases: push a tag `vX.Y.Z` and the [release workflow](.github/workflows/release.yml) builds, tests and publishes all assets.

## License

MIT
