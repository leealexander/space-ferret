//! Cleanup-candidate rules: which directories/files are caches, build output,
//! dependency folders, temp data, VM disks... and how safe they are to remove.
//!
//! Matching order for a directory (first hit wins):
//!   1. exact well-known paths (npm cache, Windows Update cache, /var/lib/docker, ...)
//!   2. children of a well-known parent (every folder in ~/.cache, ...)
//!   3. contents: CACHEDIR.TAG (standard cache marker), pyvenv.cfg (virtualenv)
//!   4. project context: `target/` next to Cargo.toml, `bin/obj` next to *.csproj, ...
//!   5. names that only ever mean one thing: node_modules, __pycache__, .pytest_cache, ...
//!   6. generic app-cache names (Cache, GPUCache, logs, temp) - only outside source repos
//!
//! Steps 3-6 are disabled under "protected" roots (Program Files, /usr, global npm,
//! nvm, ...) so we never suggest deleting the node_modules an installed app runs on.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Safety {
    Safe,
    Regen,
    Review,
    System,
    Keep,
}

impl Safety {
    pub const ALL: [Safety; 5] = [Safety::Safe, Safety::Regen, Safety::Review, Safety::System, Safety::Keep];

    pub fn tag(self) -> &'static str {
        match self {
            Safety::Safe => "safe",
            Safety::Regen => "regen",
            Safety::Review => "review",
            Safety::System => "system",
            Safety::Keep => "keep",
        }
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Safety::Safe => "cache/temp data; the owning app recreates it automatically",
            Safety::Regen => "build output or dependencies; restored by rebuilding/reinstalling (costs time/bandwidth)",
            Safety::Review => "may be personal data or expensive to get back; confirm with the user before deleting",
            Safety::System => "clean only with the OS/tool's own command shown in the hint, never by deleting files",
            Safety::Keep => "required by the system or installed software; do not delete",
        }
    }
}

pub struct Rule {
    pub id: &'static str,
    pub label: &'static str,
    pub safety: Safety,
    pub hint: &'static str,
    /// rule-specific minimum size (0 = use the global --min-size)
    pub min: u64,
    /// file rule that still applies inside protected folders (crash dumps, logs)
    pub protected_ok: bool,
}

// ---- marker flags collected while listing a directory -------------------------
pub const F_GIT: u32 = 1 << 0;
pub const F_CARGO: u32 = 1 << 1;
pub const F_NODE: u32 = 1 << 2;
pub const F_DOTNET: u32 = 1 << 3;
pub const F_GRADLE: u32 = 1 << 4;
pub const F_CMAKE: u32 = 1 << 5;
pub const F_DART: u32 = 1 << 6;
pub const F_PY: u32 = 1 << 7;
pub const F_POD: u32 = 1 << 8;
pub const F_PHP: u32 = 1 << 9;
pub const F_GO: u32 = 1 << 10;
pub const F_VENV: u32 = 1 << 11;
pub const F_CACHEDIR: u32 = 1 << 12;
pub const F_JVM: u32 = 1 << 13;
pub const F_SWIFT: u32 = 1 << 14;
pub const F_UNITY: u32 = 1 << 15;
pub const F_UNREAL: u32 = 1 << 16;
const F_PROJECT: u32 = F_GIT
    | F_CARGO
    | F_NODE
    | F_DOTNET
    | F_GRADLE
    | F_CMAKE
    | F_DART
    | F_PY
    | F_PHP
    | F_GO
    | F_JVM
    | F_SWIFT
    | F_UNITY
    | F_UNREAL;

const MARKERS: &[(&str, u32)] = &[
    ("cargo.toml", F_CARGO),
    ("package.json", F_NODE),
    ("build.gradle", F_GRADLE),
    ("build.gradle.kts", F_GRADLE),
    ("settings.gradle", F_GRADLE),
    ("settings.gradle.kts", F_GRADLE),
    ("cmakelists.txt", F_CMAKE),
    ("pubspec.yaml", F_DART),
    ("pyproject.toml", F_PY),
    ("setup.py", F_PY),
    ("podfile", F_POD),
    ("composer.json", F_PHP),
    ("go.mod", F_GO),
    ("pyvenv.cfg", F_VENV),
    ("cachedir.tag", F_CACHEDIR),
    ("pom.xml", F_JVM),
    ("build.sbt", F_JVM),
    ("package.swift", F_SWIFT),
    (".git", F_GIT), // worktrees / submodules use a .git *file*
];
const PROJECT_EXTS: &[(&[u8], u32)] =
    &[(b".csproj", F_DOTNET), (b".fsproj", F_DOTNET), (b".vbproj", F_DOTNET), (b".uproject", F_UNREAL)];

#[derive(Clone, Copy, Default)]
pub struct Flags(pub u32);

impl Flags {
    #[inline]
    pub fn has(self, mask: u32) -> bool {
        self.0 & mask != 0
    }

    #[inline]
    pub fn is_project(self) -> bool {
        self.has(F_PROJECT)
    }

    #[inline]
    pub fn mark_file(&mut self, name: &str) {
        for &(m, f) in MARKERS {
            if name.len() == m.len() && name.eq_ignore_ascii_case(m) {
                self.0 |= f;
                return;
            }
        }
        let b = name.as_bytes();
        for &(ext, f) in PROJECT_EXTS {
            if b.len() > ext.len() && b[b.len() - ext.len()..].eq_ignore_ascii_case(ext) {
                self.0 |= f;
                return;
            }
        }
    }

    #[inline]
    pub fn mark_dir(&mut self, name: &str) {
        if name == ".git" {
            self.0 |= F_GIT;
        } else if name.eq_ignore_ascii_case("projectsettings") {
            self.0 |= F_UNITY;
        }
    }
}

/// Lower-case, forward-slash, no trailing slash (except roots "/" and "c:/").
pub fn norm(p: &Path) -> String {
    let mut s = p.to_string_lossy().replace('\\', "/").to_lowercase();
    if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    if s.ends_with(':') {
        s.push('/');
    }
    s
}

#[inline]
pub fn join_norm(parent: &str, lname: &str) -> String {
    let mut s = String::with_capacity(parent.len() + lname.len() + 1);
    s.push_str(parent);
    if !parent.ends_with('/') {
        s.push('/');
    }
    s.push_str(lname);
    s
}

pub struct DirInfo<'a> {
    pub norm: &'a str,
    pub parent_norm: &'a str,
    pub lname: &'a str,
    pub own: Flags,
    pub parent: Flags,
    pub in_repo: bool,
    pub protected: bool,
}

#[derive(Default)]
pub struct Rules {
    pub list: Vec<Rule>,
    exact: HashMap<String, usize>,
    children_of: HashMap<String, usize>,
    by_name: HashMap<&'static str, usize>,
    generic: HashMap<&'static str, usize>,
    protect: HashSet<String>,
    file_name: HashMap<&'static str, usize>,
    file_ext: HashMap<&'static str, usize>,
    home_norm: String,
    rust_target: usize,
    cachedir: usize,
    venv: usize,
    dotnet: usize,
    build: usize,
    tool_cache: usize,
    pods: usize,
    php_vendor: usize,
    game_engine: usize,
    crash: usize,
}

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

impl Rules {
    fn add(&mut self, id: &'static str, label: &'static str, safety: Safety, hint: &'static str) -> usize {
        self.list.push(Rule { id, label, safety, hint, min: 0, protected_ok: false });
        self.list.len() - 1
    }
    fn at(&mut self, r: usize, p: Option<PathBuf>) {
        if let Some(p) = p {
            self.exact.insert(norm(&p), r);
        }
    }
    fn kids(&mut self, r: usize, p: Option<PathBuf>) {
        if let Some(p) = p {
            self.children_of.insert(norm(&p), r);
        }
    }
    fn guard(&mut self, p: Option<PathBuf>) {
        if let Some(p) = p {
            self.protect.insert(norm(&p));
        }
    }
    fn names(&mut self, r: usize, names: &[&'static str]) {
        for n in names {
            self.by_name.insert(n, r);
        }
    }
    fn generic_names(&mut self, r: usize, names: &[&'static str]) {
        for n in names {
            self.generic.insert(n, r);
        }
    }
    fn fnames(&mut self, r: usize, names: &[&'static str]) {
        for n in names {
            self.file_name.insert(n, r);
        }
    }
    fn fexts(&mut self, r: usize, exts: &[&'static str]) {
        for e in exts {
            self.file_ext.insert(e, r);
        }
    }

    pub fn new() -> Rules {
        use Safety::*;
        let win = cfg!(windows);
        let mut r = Rules::default();

        let home = env_path(if win { "USERPROFILE" } else { "HOME" });
        r.home_norm = home.as_deref().map(norm).unwrap_or_default();
        let hj = |rel: &str| home.as_ref().map(|h| h.join(rel));
        let local = env_path("LOCALAPPDATA");
        let lj = |rel: &str| local.as_ref().map(|p| p.join(rel));
        let roaming = env_path("APPDATA");
        let aj = |rel: &str| roaming.as_ref().map(|p| p.join(rel));
        let sysroot = env_path("SystemRoot");
        let sj = |rel: &str| sysroot.as_ref().map(|p| p.join(rel));
        let progdata = env_path("ProgramData");
        let pj = |rel: &str| progdata.as_ref().map(|p| p.join(rel));
        let sysdrive = std::env::var("SystemDrive").ok().map(|d| PathBuf::from(format!("{d}\\")));
        let dj = |rel: &str| sysdrive.as_ref().map(|p| p.join(rel));
        let xcache = env_path("XDG_CACHE_HOME").or_else(|| hj(".cache"));
        let cj = |rel: &str| xcache.as_ref().map(|p| p.join(rel));
        let xdata = env_path("XDG_DATA_HOME").or_else(|| hj(".local/share"));
        let xj = |rel: &str| xdata.as_ref().map(|p| p.join(rel));
        let abs = |p: &str| (!win).then(|| PathBuf::from(p));

        // ---------------------------------------------------------------- temp
        let temp = if win {
            r.add("temp", "Temporary files", Safe,
                "Remove-Item \"$env:TEMP\\*\" -Recurse -Force -ErrorAction SilentlyContinue   (files in use are skipped; C:\\Windows\\Temp needs admin)")
        } else {
            r.add("temp", "Temporary files", Review,
                "remove old entries only: sudo find /tmp /var/tmp -mindepth 1 -mtime +7 -delete   (running programs use these dirs)")
        };
        r.at(temp, lj("Temp"));
        r.at(temp, env_path("TEMP"));
        r.at(temp, sj("Temp"));
        r.at(temp, abs("/tmp"));
        r.at(temp, abs("/var/tmp"));

        // ------------------------------------------------ package-manager caches
        let npm = r.add("npm-cache", "npm cache", Safe, "npm cache clean --force   (the _npx folder can simply be deleted)");
        for p in [lj("npm-cache"), aj("npm-cache"), hj(".npm/_cacache"), hj(".npm/_npx")] {
            r.at(npm, p);
        }
        let yarn = r.add("yarn-cache", "Yarn cache", Safe, "yarn cache clean");
        for p in [lj("Yarn/Cache"), lj("Yarn/Berry/cache"), cj("yarn"), hj(".yarn/berry/cache")] {
            r.at(yarn, p);
        }
        let pnpm = r.add("pnpm-store", "pnpm store", Safe, "pnpm store prune");
        for p in [lj("pnpm/store"), lj("pnpm-cache"), xj("pnpm/store"), hj(".pnpm-store"), cj("pnpm")] {
            r.at(pnpm, p);
        }
        let corepack = r.add("corepack-cache", "Corepack package-manager cache", Safe,
            "corepack cache clean   (or delete the folder; pnpm/yarn versions re-download on demand)");
        r.at(corepack, lj("node/corepack"));
        r.at(corepack, cj("node/corepack"));
        let bun = r.add("bun-cache", "Bun cache", Safe, "bun pm cache rm");
        r.at(bun, hj(".bun/install/cache"));
        let pip = r.add("pip-cache", "pip cache", Safe, "pip cache purge");
        r.at(pip, lj("pip/Cache"));
        r.at(pip, cj("pip"));
        let uv = r.add("uv-cache", "uv cache", Safe, "uv cache clean   (or `uv cache prune` to keep what's in use)");
        r.at(uv, lj("uv/cache"));
        r.at(uv, cj("uv"));
        let conda = r.add("conda-pkgs", "conda package cache", Safe, "conda clean --all");
        for p in [".conda/pkgs", "anaconda3/pkgs", "miniconda3/pkgs", "miniforge3/pkgs", "mambaforge/pkgs"] {
            r.at(conda, hj(p));
        }
        let nuget = r.add("nuget-cache", "NuGet package cache", Safe, "dotnet nuget locals all --clear");
        for p in [hj(".nuget/packages"), lj("NuGet/v3-cache"), lj("NuGet/Cache"), lj("NuGet/plugins-cache"), xj("NuGet/v3-cache")] {
            r.at(nuget, p);
        }
        let gradle = r.add("gradle-cache", "Gradle caches", Safe,
            "gradle --stop, then delete; dependencies re-download on the next build");
        r.at(gradle, hj(".gradle/caches"));
        r.at(gradle, hj(".gradle/wrapper/dists"));
        let maven = r.add("maven-repo", "Maven local repository", Safe, "delete it; Maven re-downloads what builds need");
        r.at(maven, hj(".m2/repository"));
        let cargo = r.add("cargo-registry", "Cargo registry & git cache", Safe,
            "cargo install cargo-cache && cargo cache --autoclean   (or delete ~/.cargo/registry/{cache,src} and ~/.cargo/git/checkouts)");
        r.at(cargo, hj(".cargo/registry"));
        r.at(cargo, hj(".cargo/git"));
        let gomod = r.add("go-mod-cache", "Go module cache", Safe, "go clean -modcache");
        r.at(gomod, hj("go/pkg/mod"));
        let gobuild = r.add("go-build-cache", "Go build cache", Safe, "go clean -cache");
        r.at(gobuild, lj("go-build"));
        r.at(gobuild, cj("go-build"));
        r.at(gobuild, hj("Library/Caches/go-build"));

        // -------------------------------------------- big downloads worth a look
        let browsers = r.add("test-browsers", "Playwright/Puppeteer browser downloads", Review,
            "npx playwright uninstall --all   (reinstall later with npx playwright install)");
        for p in [lj("ms-playwright"), cj("ms-playwright"), cj("puppeteer"), hj(".cache/puppeteer"), hj("Library/Caches/ms-playwright")] {
            r.at(browsers, p);
        }
        let rustup = r.add("rust-toolchains", "Rust toolchains", Review,
            "rustup toolchain list, then rustup toolchain uninstall <unused>");
        r.at(rustup, hj(".rustup/toolchains"));
        let ml = r.add("ml-models", "AI/ML model files", Review,
            "downloaded model weights: `hf cache delete` (Hugging Face), `ollama list` + `ollama rm <model>`, or remove in LM Studio > My Models");
        for p in [cj("huggingface"), hj(".cache/huggingface"), cj("torch"), hj(".cache/torch"), hj(".ollama/models"), hj(".lmstudio/models"), cj("lm-studio/models")] {
            r.at(ml, p);
        }
        let runtimes = r.add("runtime-versions", "Installed language runtime versions", Review,
            "list and uninstall versions you no longer use: nvm ls / nvm uninstall <v>, pyenv uninstall <v>, uv python uninstall <v>");
        for p in [hj(".nvm/versions/node"), aj("nvm"), hj(".volta/tools/image/node"), aj("fnm/node-versions"), hj(".local/share/fnm/node-versions"), hj(".fnm/node-versions"), hj(".pyenv/versions"), hj(".pyenv/pyenv-win/versions"), aj("uv/python"), xj("uv/python")] {
            r.kids(runtimes, p);
        }
        let android = r.add("android-sdk", "Android SDK / emulator images", Review,
            "Android Studio > SDK Manager / Device Manager: remove old system images and unused virtual devices");
        for p in [hj(".android/avd"), lj("Android/Sdk"), hj("Android/Sdk"), hj("Library/Android/sdk")] {
            r.at(android, p);
        }
        let editors = r.add("editor-extensions", "Editor extensions (VS Code & forks)", Review,
            "old extension versions pile up: uninstall unused extensions; stale versioned folders can be deleted with the editor closed");
        for p in [".vscode/extensions", ".vscode-insiders/extensions", ".vscode-oss/extensions", ".cursor/extensions", ".windsurf/extensions", ".antigravity/extensions", ".vscode-server", ".cursor-server"] {
            r.at(editors, hj(p));
        }
        let k8s = r.add("local-k8s", "Local Kubernetes VM (minikube)", Review, "minikube delete   (or minikube delete --all --purge)");
        r.at(k8s, hj(".minikube"));
        let docker = r.add("containers", "Docker / Podman storage", System,
            if win {
                "docker system prune -a (add --volumes to drop unused volumes) / podman system prune -a; then shrink the WSL disk: wsl --shutdown and Optimize-VHD or diskpart compact vdisk"
            } else {
                "docker system prune -a (add --volumes to drop unused volumes) / podman system prune -a"
            });
        for p in [lj("Docker/wsl"), pj("docker"), hj(".local/share/containers"), abs("/var/lib/docker"), abs("/var/lib/containers")] {
            r.at(docker, p);
        }
        let downloads = r.add("downloads", "Downloads folder", Review,
            "personal files: look for old installers, ISOs, archives and duplicates (see largest_files)");
        r.at(downloads, hj("Downloads"));
        let trash = r.add("trash", if win { "Recycle Bin" } else { "Trash" }, Safe,
            if win { "Clear-RecycleBin -Force   (or empty it from Explorer)" } else { "gio trash --empty   (or empty it from the file manager)" });
        r.at(trash, xj("Trash"));
        r.at(trash, hj(".local/share/Trash"));
        r.at(trash, hj(".Trash"));
        r.names(trash, &["$recycle.bin"]);

        // ------------------------------------------------------- crash / logs
        let crash = r.add("crash-dumps", "Crash dumps & error reports", Safe, "safe to delete; only useful for debugging a past crash");
        r.crash = crash;
        for p in [lj("CrashDumps"), sj("Minidump"), sj("LiveKernelReports"), pj("Microsoft/Windows/WER"), abs("/var/crash"), abs("/var/lib/systemd/coredump")] {
            r.at(crash, p);
        }
        r.list[crash].protected_ok = true;
        r.fnames(crash, &["memory.dmp"]);
        r.fexts(crash, &["dmp", "mdmp", "hdmp"]);
        r.generic_names(crash, &["crashpad", "crash reports", "crashdumps", "crashes"]);

        // ----------------------------------------------------------- Windows
        let winupd = r.add("windows-update", "Windows Update download cache", System,
            "Disk Cleanup (cleanmgr) > Clean up system files > Windows Update Cleanup   (or as admin: net stop wuauserv; delete contents; net start wuauserv)");
        r.at(winupd, sj("SoftwareDistribution/Download"));
        let winsxs = r.add("winsxs", "Windows component store (WinSxS)", System,
            "elevated: DISM /Online /Cleanup-Image /StartComponentCleanup   (its size is overstated: most files are hardlinks)");
        r.at(winsxs, sj("WinSxS"));
        let winold = r.add("windows-old", "Previous Windows installation / upgrade leftovers", System,
            "Settings > System > Storage > Temporary files > Previous Windows installation(s)");
        for p in ["Windows.old", "$WINDOWS.~BT", "$Windows.~WS"] {
            r.at(winold, dj(p));
        }
        let instcache = r.add("installer-cache", "Installer caches (MSI / Package Cache)", Keep,
            "needed to repair or uninstall software; never delete by hand (use PatchCleaner-style tools only if you know why)");
        r.at(instcache, sj("Installer"));
        r.at(instcache, pj("Package Cache"));
        r.at(instcache, lj("Package Cache"));
        let vscache = r.add("vs-installer-cache", "Visual Studio installer download cache", Review,
            "Visual Studio Installer > More > Settings: untick \"Keep download cache\" (it is re-downloaded when you modify/repair VS)");
        r.at(vscache, pj("Microsoft/VisualStudio/Packages"));
        let webcache = r.add("browser-cache", "Internet / browser caches", Safe, "clear from the browser settings, or delete while it is closed");
        r.at(webcache, lj("Microsoft/Windows/INetCache"));
        let shader = r.add("shader-cache", "GPU shader caches", Safe,
            "rebuilt automatically (first launches of games get slower): Disk Cleanup > DirectX Shader Cache, or delete contents");
        for p in [lj("D3DSCache"), lj("NVIDIA/DXCache"), lj("NVIDIA/GLCache"), pj("NVIDIA Corporation/NV_Cache"), lj("AMD/DxCache"), lj("AMD/DxcCache"), lj("AMD/VkCache"), lj("AMD/GLCache"), lj("Intel/ShaderCache"), cj("mesa_shader_cache"), cj("nvidia")] {
            r.at(shader, p);
        }
        let thumbs = r.add("thumbnails", "Thumbnail cache", Safe,
            if win { "Disk Cleanup > Thumbnails" } else { "rm -rf ~/.cache/thumbnails/*" });
        r.at(thumbs, lj("Microsoft/Windows/Explorer"));
        r.at(thumbs, cj("thumbnails"));
        let hiber = r.add("hibernation", "Hibernation file", System,
            "elevated: powercfg /h off (disables hibernate + Fast Startup) or powercfg /h /type reduced");
        r.fnames(hiber, &["hiberfil.sys"]);
        let pagefile = r.add("pagefile", "Page / swap file", Keep,
            "virtual memory; leave it (resize only via System Properties > Performance > Virtual memory, or swap settings)");
        r.fnames(pagefile, &["pagefile.sys", "swapfile.sys", "swapfile", "swap.img"]);

        // ------------------------------------------------------------- Linux
        if !win {
            let apt = r.add("apt-cache", "APT package cache", System, "sudo apt clean   (and sudo apt autoremove --purge for old kernels)");
            r.at(apt, abs("/var/cache/apt"));
            let dnf = r.add("dnf-cache", "DNF/YUM package cache", System, "sudo dnf clean all");
            r.at(dnf, abs("/var/cache/dnf"));
            r.at(dnf, abs("/var/cache/yum"));
            let pacman = r.add("pacman-cache", "pacman package cache", System, "sudo paccache -rk1   (pacman-contrib) or sudo pacman -Sc");
            r.at(pacman, abs("/var/cache/pacman/pkg"));
            let journal = r.add("journal", "systemd journal logs", System, "sudo journalctl --vacuum-size=200M");
            r.at(journal, abs("/var/log/journal"));
            let snap = r.add("snap", "Snap package revisions", System,
                "remove disabled revisions: snap list --all | awk '/disabled/{print $1, $3}' | while read n r; do sudo snap remove \"$n\" --revision=\"$r\"; done");
            r.at(snap, abs("/var/lib/snapd/snaps"));
            let flatpak = r.add("flatpak", "Flatpak runtimes", System, "flatpak uninstall --unused");
            r.at(flatpak, abs("/var/lib/flatpak"));
            r.at(flatpak, xj("flatpak"));
            let xcode = r.add("xcode", "Xcode DerivedData / simulators", Safe,
                "rm -rf ~/Library/Developer/Xcode/DerivedData/*; xcrun simctl delete unavailable");
            r.at(xcode, hj("Library/Developer/Xcode/DerivedData"));
            r.at(xcode, hj("Library/Developer/CoreSimulator/Caches"));
        }

        // --------------------------------------- per-app caches (~/.cache/*)
        let appcache = r.add("app-cache", "Application caches", Safe, "app cache: close the app, then delete; it is recreated as needed");
        r.kids(appcache, xcache.clone());
        r.kids(appcache, hj(".cache"));
        r.kids(appcache, hj("Library/Caches"));
        r.generic_names(appcache, &[
            "cache", "caches", "code cache", "gpucache", "shadercache", "dawncache", "grshadercache",
            "graphitedawncache", "dawngraphitecache", "dawnwebgpucache", "cachestorage", "cache_data",
            // NOT "localcache": MSIX/Store apps get their real AppData redirected into
            // Packages\<app>\LocalCache (logins, settings, pip --user packages...).
            "cacheddata", "cachedextensionvsixs", "inetcache", "dxcache", "glcache",
            "d3dscache", "component_crx_cache",
        ]);
        let synccache = r.add("cloud-sync-cache", "Cloud-drive local cache", Review,
            "shrink it from the sync app's settings (Google Drive: Settings > cache location/size); never delete by hand, it can hold files not yet uploaded");
        r.generic_names(synccache, &["content_cache"]);
        let tempdirs = r.add("temp-dirs", "Application temp folders", Review, "usually leftovers; delete with the owning app closed");
        r.generic_names(tempdirs, &["temp", "tmp", "tempstate"]);
        let logs = r.add("logs", "Log folders", Review, "old logs; safe to prune when the app isn't running");
        r.generic_names(logs, &["logs"]);

        // ---------------------------------------------------- project artifacts
        let nm = r.add("node_modules", "node_modules (JS dependencies)", Regen,
            "delete, restore with npm/pnpm/yarn install; stale projects first (`npx npkill` lists them interactively)");
        r.names(nm, &["node_modules"]);
        let pyc = r.add("pycache", "Python bytecode caches (__pycache__)", Safe,
            if win {
                "Get-ChildItem -Recurse -Directory -Filter __pycache__ | Remove-Item -Recurse -Force"
            } else {
                "find . -type d -name __pycache__ -prune -exec rm -rf {} +"
            });
        r.names(pyc, &["__pycache__"]);
        r.tool_cache = r.add("tool-cache", "Tool / test / IDE caches", Safe, "safe to delete; recreated on next run (close the IDE for .vs)");
        let tc = r.tool_cache;
        r.names(tc, &[
            ".pytest_cache", ".mypy_cache", ".ruff_cache", ".hypothesis", ".tox", ".nox", ".eggs",
            ".ipynb_checkpoints", ".parcel-cache", ".turbo", ".angular", ".dart_tool", ".sass-cache",
            ".vs", "ipch", ".godot", ".zig-cache", "zig-cache", ".stack-work", "elm-stuff", ".docusaurus",
        ]);
        let jsbuild = r.add("js-framework-build", "JS framework build output (.next, .nuxt, ...)", Regen, "regenerated by the dev server / next build");
        r.names(jsbuild, &[".next", ".nuxt", ".svelte-kit", ".expo"]);
        let tf = r.add("terraform", "Terraform providers (.terraform)", Regen, "terraform init downloads them again");
        r.names(tf, &[".terraform"]);
        let git = r.add("git", "Large git repositories (.git)", Review,
            "not removable, but can shrink: git gc --prune=now, git lfs prune, or look for big committed binaries");
        r.names(git, &[".git"]);
        r.list[git].min = 200 << 20;
        r.rust_target = r.add("rust-target", "Rust build output (target/)", Regen,
            "cargo clean in the project (or `cargo install cargo-sweep && cargo sweep -r -t 30` for artifacts older than 30 days)");
        r.cachedir = r.add("tagged-cache", "Tagged cache directories (CACHEDIR.TAG)", Safe, "marked as disposable cache by the tool that made it");
        r.venv = r.add("python-venv", "Python virtual environments", Regen,
            "delete and recreate when needed (python -m venv .venv / uv venv && uv sync)");
        r.dotnet = r.add("dotnet-build", ".NET build output (bin/, obj/)", Regen, "dotnet clean, or delete bin/ and obj/");
        r.build = r.add("build-output", "Build output (build/, dist/, target/)", Regen, "recreated by the project's build");
        r.names(r.build, &["dist-newstyle"]);
        r.pods = r.add("cocoapods", "CocoaPods Pods/", Regen, "pod install restores it");
        r.php_vendor = r.add("php-vendor", "Composer vendor/", Regen, "composer install restores it");
        r.game_engine = r.add("game-engine-cache", "Unity/Unreal generated folders", Regen,
            "Library/, Intermediate/, DerivedDataCache/ are rebuilt when the project is reopened (can take a while)");

        // ------------------------------------------------------- large files
        let vdisk = r.add("vm-disks", "Virtual disks (WSL / Docker / VMs)", Review,
            "never shrink on their own: free space inside first, then compact (wsl --shutdown; Optimize-VHD -Mode Full, or diskpart compact vdisk; `wsl --manage <distro> --set-sparse true`)");
        r.fexts(vdisk, &["vhdx", "vhd", "avhdx", "vdi", "vmdk", "qcow2"]);
        let installers = r.add("installers", "Disk images & installers", Review, "usually deletable once the software is installed");
        r.fexts(installers, &["iso", "dmg", "msi", "msix", "msixbundle", "appxbundle", "deb", "rpm"]);
        let archives = r.add("archives", "Archives", Review, "delete if already extracted or backed up elsewhere");
        r.fexts(archives, &["zip", "7z", "rar", "tar", "gz", "tgz", "xz", "zst", "bz2"]);
        let partial = r.add("partial-files", "Incomplete downloads / temp files", Safe, "leftovers of interrupted downloads or installs");
        r.fexts(partial, &["crdownload", "part", "partial", "tmp"]);
        let logf = r.add("log-files", "Large log / trace files", Review, "truncate or delete if the owning app is not running");
        r.list[logf].protected_ok = true;
        r.fexts(logf, &["log", "etl"]);
        let bak = r.add("backups", "Backup / old copies", Review, "check they're not your only copy, then delete");
        r.fexts(bak, &["bak", "old"]);
        r.fexts(ml, &["gguf", "safetensors", "ckpt", "onnx", "pth"]);

        // ------------------------------------------------- protected subtrees
        for k in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "SystemRoot", "ProgramData"] {
            r.guard(env_path(k));
        }
        for p in [lj("Programs"), lj("Microsoft/WinGet/Packages"), aj("npm"), aj("nvm"), aj("fnm"), lj("Yarn/Data/global"), lj("pnpm/global"), hj("scoop/apps")] {
            r.guard(p);
        }
        for p in [".nvm", ".volta", ".npm-global", ".bun/install/global", ".local/share/pnpm/global", ".local/share/fnm", ".pyenv", ".rustup", ".cargo", ".dotnet", ".local/lib", "go/pkg"] {
            r.guard(hj(p));
        }
        for p in ["/usr", "/opt", "/snap", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/etc", "/boot", "/var/lib", "/var/cache", "/var/log", "/var/spool", "/nix", "/Applications", "/System", "/Library"] {
            r.guard(abs(p));
        }
        r
    }

    pub fn match_dir(&self, d: &DirInfo) -> Option<usize> {
        if let Some(&r) = self.exact.get(d.norm) {
            return Some(r);
        }
        if let Some(&r) = self.children_of.get(d.parent_norm) {
            return Some(r);
        }
        // virtualenv also drops a CACHEDIR.TAG into the envs it creates, so a
        // pyvenv.cfg wins: it is a (regenerable) environment, not a cache.
        if d.own.has(F_VENV) && !d.protected {
            return Some(self.venv);
        }
        if d.own.has(F_CACHEDIR) {
            return Some(if d.lname == "target" { self.rust_target } else { self.cachedir });
        }
        if d.protected {
            return None;
        }
        // A stray package.json / Cargo.toml in $HOME must not turn ~/.cache or
        // ~/build into "project build output".
        let p = if self.is_home_or_above(d.parent_norm) { Flags::default() } else { d.parent };
        let by_context = match d.lname {
            "target" if p.has(F_CARGO) => Some(self.rust_target),
            "target" if p.has(F_JVM) => Some(self.build),
            "bin" | "obj" if p.has(F_DOTNET) => Some(self.dotnet),
            "build" if p.has(F_GRADLE | F_CMAKE | F_DART | F_PY | F_NODE) => Some(self.build),
            "dist" if p.has(F_NODE | F_PY) => Some(self.build),
            ".build" if p.has(F_SWIFT) => Some(self.build),
            ".gradle" if p.has(F_GRADLE) => Some(self.tool_cache),
            ".cache" if p.has(F_NODE) => Some(self.tool_cache),
            "pods" if p.has(F_POD) => Some(self.pods),
            "vendor" if p.has(F_PHP) => Some(self.php_vendor),
            "library" | "temp" | "obj" if p.has(F_UNITY) => Some(self.game_engine),
            "intermediate" | "deriveddatacache" if p.has(F_UNREAL) => Some(self.game_engine),
            n if n.starts_with("cmake-build-") => Some(self.build),
            _ => None,
        };
        if by_context.is_some() {
            return by_context;
        }
        if let Some(&r) = self.by_name.get(d.lname) {
            return Some(r);
        }
        if !d.in_repo {
            if let Some(&r) = self.generic.get(d.lname) {
                return Some(r);
            }
        }
        None
    }

    /// `protected`: the file lives under Program Files, /usr, ... where only
    /// crash dumps, logs and system files (hiberfil, pagefile) are reported.
    pub fn match_file(&self, name: &str, protected: bool) -> Option<usize> {
        let l = name.to_ascii_lowercase();
        if let Some(&r) = self.file_name.get(l.as_str()) {
            return Some(r);
        }
        if l.len() > 5 && l.starts_with("core.") && l[5..].bytes().all(|c| c.is_ascii_digit()) {
            return Some(self.crash);
        }
        let (_, ext) = l.rsplit_once('.')?;
        let r = *self.file_ext.get(ext)?;
        (!protected || self.list[r].protected_ok).then_some(r)
    }

    pub fn is_protected(&self, norm: &str) -> bool {
        self.protect.contains(norm)
    }

    /// True for the home directory itself and every directory above it; a stray
    /// package.json in $HOME must not turn the whole profile into a "repo".
    pub fn is_home_or_above(&self, norm: &str) -> bool {
        let h = &self.home_norm;
        h == norm
            || (h.starts_with(norm) && (norm.ends_with('/') || h.as_bytes().get(norm.len()) == Some(&b'/')))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info<'a>(norm: &'a str, parent: &'a str, lname: &'a str, own: Flags, pf: Flags) -> DirInfo<'a> {
        DirInfo { norm, parent_norm: parent, lname, own, parent: pf, in_repo: false, protected: false }
    }

    #[test]
    fn norms() {
        assert_eq!(norm(Path::new("/")), "/");
        assert_eq!(join_norm("/", "usr"), "/usr");
        assert_eq!(join_norm("/usr", "lib"), "/usr/lib");
        assert_eq!(join_norm("c:/", "users"), "c:/users");
    }

    #[test]
    fn project_rules() {
        let r = Rules::new();
        let mut cargo = Flags::default();
        cargo.mark_file("Cargo.toml");
        let m = r.match_dir(&info("/x/target", "/x", "target", Flags::default(), cargo)).unwrap();
        assert_eq!(r.list[m].id, "rust-target");
        // target/ without Cargo.toml is not a candidate
        assert!(r.match_dir(&info("/x/target", "/x", "target", Flags::default(), Flags::default())).is_none());
        let m = r.match_dir(&info("/x/node_modules", "/x", "node_modules", Flags::default(), Flags::default())).unwrap();
        assert_eq!(r.list[m].id, "node_modules");
        let mut venv = Flags::default();
        venv.mark_file("pyvenv.cfg");
        let m = r.match_dir(&info("/x/.venv", "/x", ".venv", venv, Flags::default())).unwrap();
        assert_eq!(r.list[m].id, "python-venv");
        // virtualenv-made envs carry CACHEDIR.TAG too; still a venv, not a "safe" cache
        venv.mark_file("CACHEDIR.TAG");
        let m = r.match_dir(&info("/x/env3", "/x", "env3", venv, Flags::default())).unwrap();
        assert_eq!(r.list[m].id, "python-venv");
        // MSIX apps keep real data in LocalCache; never a generic "safe" cache
        let lc = info("c:/u/appdata/local/packages/app/localcache", "c:/u/appdata/local/packages/app", "localcache", Flags::default(), Flags::default());
        assert!(r.match_dir(&lc).is_none());
        // generic cache names are ignored inside source repos
        let mut d = info("/x/src/cache", "/x/src", "cache", Flags::default(), Flags::default());
        assert!(r.match_dir(&d).is_some());
        d.in_repo = true;
        assert!(r.match_dir(&d).is_none());
        // a package.json in $HOME doesn't make ~/.cache a project cache
        let mut node = Flags::default();
        node.mark_file("package.json");
        let home = r.home_norm.clone();
        let hc = join_norm(&home, ".cache");
        let m = r.match_dir(&info(&hc, &home, ".cache", Flags::default(), node));
        assert!(m.is_none() || r.list[m.unwrap()].id != "tool-cache");
        // and project rules are ignored under protected roots
        let mut d = info("/usr/lib/node_modules", "/usr/lib", "node_modules", Flags::default(), Flags::default());
        d.protected = true;
        assert!(r.match_dir(&d).is_none());
    }

    #[test]
    fn file_rules() {
        let r = Rules::new();
        assert_eq!(r.list[r.match_file("ext4.VHDX", false).unwrap()].id, "vm-disks");
        assert!(r.match_file("system.vhd", true).is_none()); // e.g. C:\Program Files\WSL\system.vhd
        assert_eq!(r.list[r.match_file("core.1234", false).unwrap()].id, "crash-dumps");
        assert_eq!(r.list[r.match_file("MEMORY.DMP", true).unwrap()].id, "crash-dumps");
        assert_eq!(r.list[r.match_file("hiberfil.sys", false).unwrap()].id, "hibernation");
        assert!(r.match_file("photo.jpg", false).is_none());
    }
}
