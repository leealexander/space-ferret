//! space-ferret: fast, parallel disk-usage breakdown that also points out what can be
//! cleaned up (caches, build output, dependency folders, temp, VM disks...).
//! Read-only: it never deletes anything.
mod json;
mod report;
mod rules;
mod scan;
mod sys;
mod util;

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Opts {
    pub path: Option<PathBuf>,
    pub json: bool,
    pub depth: usize,
    pub top: usize,
    pub min_pct: f64,
    pub min_size: u64,
    pub min_file: u64,
    pub excludes: Vec<String>,
    pub cross_mounts: bool,
    pub threads: usize,
    pub color: bool,
    pub quiet: bool,
    pub drives: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            path: None,
            json: false,
            depth: 3,
            top: 15,
            min_pct: 1.0,
            min_size: 10 << 20,
            min_file: 100 << 20,
            excludes: Vec::new(),
            cross_mounts: false,
            threads: 0,
            color: true,
            quiet: false,
            drives: false,
        }
    }
}

const HELP: &str = "\
space-ferret - fast disk usage breakdown + cleanup finder (read-only, never deletes)

USAGE:
    space-ferret [OPTIONS] [PATH]   scan PATH (default: current directory)
    space-ferret --drives           list volumes with size / free space

OUTPUT:
    -j, --json                JSON report (for scripts and AI agents)
    -d, --depth N             tree depth to show                      [default: 3]
    -n, --top N               rows per list (files, types, candidates) [default: 15]
    -p, --min-pct P           hide tree entries below P% of total      [default: 1]
    -m, --min-size SIZE       smallest folder to flag for cleanup      [default: 10M]
    -f, --min-file SIZE       smallest single file to flag for cleanup [default: 100M]
        --no-color            plain output (NO_COLOR is honoured too)
    -q, --quiet               no progress line on stderr

SCAN:
    -e, --exclude NAME|PATH   skip folders with this name or path (repeatable)
        --cross-mounts        descend into other mounted filesystems (Unix; by
                              default it stays on PATH's filesystem, like du -x)
    -t, --threads N           worker threads                 [default: 2x CPU cores]

    -h, --help                this help
    -V, --version             version

SIZE accepts plain bytes or K/M/G/T suffixes (1024-based): 500K, 20M, 1.5G.

EXAMPLES:
    space-ferret ~                    what's using my home folder?
    space-ferret C:\\ -d 2             whole drive, two levels deep (elevate for full numbers)
    space-ferret / --json -q          whole Linux root filesystem, as JSON
    space-ferret . -e node_modules    ignore dependency folders

https://github.com/leealexander/space-ferret
";

fn parse_args() -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut args = std::env::args_os().skip(1);
    while let Some(raw) = args.next() {
        let s = raw.to_string_lossy().into_owned();
        let (key, inline) = match (s.starts_with("--"), s.find('=')) {
            (true, Some(i)) => (s[..i].to_string(), Some(s[i + 1..].to_string())),
            _ => (s.clone(), None),
        };
        let mut val = |name: &str| -> Result<String, String> {
            match &inline {
                Some(v) => Ok(v.clone()),
                None => args
                    .next()
                    .map(|v| v.to_string_lossy().into_owned())
                    .ok_or_else(|| format!("{name} needs a value")),
            }
        };
        fn num<T: std::str::FromStr>(k: &str, v: String) -> Result<T, String> {
            v.parse().map_err(|_| format!("invalid value for {k}: '{v}'"))
        }
        fn size(k: &str, v: String) -> Result<u64, String> {
            util::parse_size(&v).ok_or_else(|| format!("invalid size for {k}: '{v}' (try 10M, 1.5G)"))
        }
        match key.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("space-ferret {VERSION}");
                std::process::exit(0);
            }
            "-j" | "--json" => o.json = true,
            "-q" | "--quiet" => o.quiet = true,
            "--no-color" | "--no-colour" => o.color = false,
            "--drives" | "--volumes" => o.drives = true,
            "--cross-mounts" => o.cross_mounts = true,
            "-x" | "--one-file-system" => o.cross_mounts = false,
            "-d" | "--depth" => o.depth = num(&key, val(&key)?)?,
            "-n" | "--top" => o.top = num::<usize>(&key, val(&key)?)?.max(1),
            "-p" | "--min-pct" => o.min_pct = num(&key, val(&key)?)?,
            "-t" | "--threads" => o.threads = num(&key, val(&key)?)?,
            "-m" | "--min-size" => o.min_size = size(&key, val(&key)?)?,
            "-f" | "--min-file" => o.min_file = size(&key, val(&key)?)?,
            "-e" | "--exclude" => o.excludes.push(val(&key)?),
            k if k.starts_with('-') && k.len() > 1 => return Err(format!("unknown option '{k}'")),
            _ => {
                if o.path.is_some() {
                    return Err("only one PATH can be given".into());
                }
                o.path = Some(PathBuf::from(raw));
            }
        }
    }
    Ok(o)
}

fn resolve_root(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if cfg!(windows) && s.len() == 2 && s.ends_with(':') {
        return PathBuf::from(format!("{s}\\")); // "C:" means "current dir on C:"; users mean the drive
    }
    if cfg!(unix) {
        if let Ok(c) = std::fs::canonicalize(p) {
            return c;
        }
    }
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn emit(s: &str) {
    let _ = std::io::stdout().lock().write_all(s.as_bytes()); // ignore broken pipe (| head)
}

fn progress(sc: &scan::Scanner, done: &AtomicBool, start: Instant) {
    let mut shown = false;
    let mut last = Instant::now();
    while !done.load(Relaxed) {
        std::thread::sleep(Duration::from_millis(40));
        if start.elapsed() < Duration::from_millis(400) || last.elapsed() < Duration::from_millis(200) {
            continue;
        }
        last = Instant::now();
        let line = format!(
            "  scanning… {} files · {} folders · {}",
            util::thousands(sc.files.load(Relaxed)),
            util::thousands(sc.dirs.load(Relaxed)),
            util::human(sc.bytes.load(Relaxed))
        );
        eprint!("\r{line:<72}");
        shown = true;
    }
    if shown {
        eprint!("\r{:<72}\r", "");
    }
}

fn main() {
    let o = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("space-ferret: {e}\nTry 'space-ferret --help'.");
            std::process::exit(2);
        }
    };
    let color = o.color
        && std::env::var_os("NO_COLOR").is_none()
        && std::io::stdout().is_terminal()
        && sys::enable_ansi();

    if o.drives {
        let vols = sys::volumes();
        emit(&if o.json { report::drives_json(&vols) } else { report::drives_human(&vols, color) });
        return;
    }

    let root = resolve_root(o.path.as_deref().unwrap_or(Path::new(".")));
    let md = match std::fs::metadata(&root) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("space-ferret: {}: {e}", root.display());
            std::process::exit(1);
        }
    };
    if !md.is_dir() {
        let (disk, cloud) = sys::usage(&md);
        let note = if cloud > 0 { format!(" (cloud-only, {} logical)", util::human(cloud)) } else { String::new() };
        emit(&format!("{}  {}{}\n", util::human(disk), root.display(), note));
        return;
    }

    let rules = rules::Rules::new();
    let (mut ex_names, mut ex_paths) = (Vec::new(), Vec::new());
    for e in &o.excludes {
        if e.contains('/') || e.contains('\\') {
            ex_paths.push(rules::norm(&resolve_root(Path::new(e))));
        } else {
            ex_names.push(e.to_lowercase());
        }
    }
    let cfg = scan::Config {
        top_n: o.top,
        keep_depth: o.depth,
        min_size: o.min_size,
        min_file: o.min_file,
        cross_mounts: o.cross_mounts,
        ex_names,
        ex_paths,
    };
    let threads = if o.threads > 0 {
        o.threads
    } else {
        std::thread::available_parallelism().map(|n| n.get() * 2).unwrap_or(8).clamp(4, 64)
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .stack_size(32 << 20)
        .thread_name(|i| format!("sf-{i}"))
        .build()
        .expect("thread pool");

    let scanner = scan::Scanner::new(&cfg, &rules, sys::dev(&md));
    let show_progress = !o.quiet && std::io::stderr().is_terminal();
    let done = AtomicBool::new(false);
    let start = Instant::now();
    let tree = std::thread::scope(|s| {
        if show_progress {
            s.spawn(|| progress(&scanner, &done, start));
        }
        let t = pool.install(|| scanner.scan_root(&root));
        done.store(true, Relaxed);
        t
    });
    let elapsed = start.elapsed();

    // Collect the per-thread accumulators.
    let locals = pool.broadcast(|_| scan::LOCAL.with(|l| std::mem::take(&mut *l.borrow_mut())));
    let mut ext_map: HashMap<Box<str>, (u64, u64)> = HashMap::new();
    let mut files: Vec<scan::BigFile> = Vec::new();
    for l in locals {
        for (k, (s, n)) in l.ext {
            let e = ext_map.entry(k).or_insert((0, 0));
            e.0 += s;
            e.1 += n;
        }
        files.extend(l.top.into_iter().map(|r| r.0));
    }
    files.sort_unstable_by(|a, b| b.size.cmp(&a.size));
    files.truncate(o.top);
    let mut exts: Vec<(String, u64, u64)> = ext_map.into_iter().map(|(k, (s, n))| (k.into(), s, n)).collect();
    exts.sort_unstable_by(|a, b| b.1.cmp(&a.1));

    let rep = report::Report {
        volume: sys::volume_for(&root),
        root,
        tree,
        elapsed_ms: elapsed.as_millis(),
        threads,
        errors: scanner.errors.load(Relaxed),
        error_samples: std::mem::take(&mut *scanner.error_samples.lock().unwrap()),
        skipped: std::mem::take(&mut *scanner.skipped.lock().unwrap()),
        skipped_n: scanner.skipped_n.load(Relaxed),
        files,
        exts,
        cands: std::mem::take(&mut *scanner.cands.lock().unwrap()),
        now: util::now_secs(),
        rules: &rules,
    };
    emit(&if o.json { report::json_report(&rep, &o) } else { report::human_report(&rep, &o, color) });
}
