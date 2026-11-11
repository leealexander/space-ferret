//! Parallel directory walker. Each directory is listed once; sub-directories
//! are fanned out over rayon's work-stealing pool. Per-thread accumulators
//! (extension totals, largest files) avoid lock contention on the hot path.
use crate::rules::{self, DirInfo, Flags, Rules};
use crate::sys;
use rayon::prelude::*;
use std::borrow::Cow;
use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

#[derive(Default)]
pub struct Node {
    pub name: String,
    pub size: u64,
    pub cloud: u64,
    pub files: u64,
    pub dirs: u64,
    /// newest mtime (unix secs) of anything in the subtree
    pub newest: i64,
    pub unreadable: bool,
    /// kept only down to --depth, sorted largest first
    pub children: Vec<Node>,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub struct BigFile {
    pub size: u64,
    pub mtime: i64,
    pub path: PathBuf,
}

pub struct Cand {
    pub rule: usize,
    pub path: PathBuf,
    pub size: u64,
    pub files: u64,
    pub newest: i64,
    pub is_file: bool,
}

#[derive(Default)]
pub struct Local {
    pub ext: HashMap<Box<str>, (u64, u64)>,
    pub top: BinaryHeap<Reverse<BigFile>>,
}

thread_local! {
    pub static LOCAL: RefCell<Local> = RefCell::new(Local::default());
}

pub struct Config {
    pub top_n: usize,
    pub keep_depth: usize,
    pub min_size: u64,
    pub min_file: u64,
    pub cross_mounts: bool,
    pub ex_names: Vec<String>,
    pub ex_paths: Vec<String>,
}

pub struct Scanner<'a> {
    cfg: &'a Config,
    rules: &'a Rules,
    root_dev: u64,
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    pub error_samples: Mutex<Vec<String>>,
    pub skipped: Mutex<Vec<String>>,
    pub skipped_n: AtomicU64,
    pub cands: Mutex<Vec<Cand>>,
    inodes: Mutex<HashSet<(u64, u64)>>,
}

#[derive(Clone, Copy)]
struct Ctx<'p> {
    parent_norm: &'p str,
    parent_flags: Flags,
    in_cand: bool,
    in_repo: bool,
    protected: bool,
    depth: usize,
}

struct Sub {
    path: PathBuf,
    name: String,
    norm: String,
    lname: String,
    mtime: i64,
}

const MAX_SAMPLES: usize = 12;

#[inline]
fn ext_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() && name.len() - i <= 12 => &name[i + 1..],
        _ => "",
    }
}

fn flags_of(dir: &Path) -> Flags {
    let mut f = Flags::default();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name();
            let n = n.to_string_lossy();
            match e.file_type() {
                Ok(t) if t.is_dir() => f.mark_dir(&n),
                Ok(_) => f.mark_file(&n),
                Err(_) => {}
            }
        }
    }
    f
}

impl<'a> Scanner<'a> {
    pub fn new(cfg: &'a Config, rules: &'a Rules, root_dev: u64) -> Self {
        Scanner {
            cfg,
            rules,
            root_dev,
            files: AtomicU64::new(0),
            dirs: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            error_samples: Mutex::new(Vec::new()),
            skipped: Mutex::new(Vec::new()),
            skipped_n: AtomicU64::new(0),
            cands: Mutex::new(Vec::new()),
            inodes: Mutex::new(HashSet::new()),
        }
    }

    pub fn scan_root(&self, root: &Path) -> Node {
        let norm = rules::norm(root);
        let lname = root.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let mtime = fs::metadata(root).map(|m| sys::mtime(&m)).unwrap_or(0);
        let parent = root.parent();
        let parent_norm = parent.map(rules::norm).unwrap_or_default();
        let parent_flags = parent.map(flags_of).unwrap_or_default();
        // Inherit context from ancestors, e.g. when scanning a folder inside a repo.
        let (mut in_repo, mut protected) = (false, false);
        for a in root.ancestors().skip(1) {
            let an = rules::norm(a);
            protected |= self.rules.is_protected(&an);
            if !in_repo && !self.rules.is_home_or_above(&an) && a.join(".git").exists() {
                in_repo = true;
            }
        }
        let sub = Sub { path: root.to_path_buf(), name: root.display().to_string(), norm, lname, mtime };
        let ctx = Ctx { parent_norm: &parent_norm, parent_flags, in_cand: false, in_repo, protected, depth: 0 };
        self.scan_dir(sub, ctx)
    }

    fn scan_dir(&self, sub: Sub, ctx: Ctx<'_>) -> Node {
        let Sub { path, name, norm, lname, mtime } = sub;
        let mut node = Node { name, newest: mtime, ..Default::default() };
        let rd = match fs::read_dir(&path) {
            Ok(rd) => rd,
            Err(e) => {
                self.err(&path, &e);
                node.unreadable = true;
                return node;
            }
        };

        let mut flags = Flags::default();
        let mut subs: Vec<Sub> = Vec::new();
        let mut file_cands: Vec<Cand> = Vec::new();
        let (mut own_bytes, mut own_cloud, mut own_files) = (0u64, 0u64, 0u64);
        let top_n = self.cfg.top_n;
        let protected = ctx.protected || self.rules.is_protected(&norm);

        // List this directory. The thread-local borrow must end before we recurse
        // (rayon may run a nested scan on this same thread while we wait).
        LOCAL.with(|cell| {
            let mut local = cell.borrow_mut();
            for ent in rd {
                let ent = match ent {
                    Ok(e) => e,
                    Err(e) => {
                        self.err(&path, &e);
                        continue;
                    }
                };
                let ft = match ent.file_type() {
                    Ok(t) => t,
                    Err(e) => {
                        self.err(&ent.path(), &e);
                        continue;
                    }
                };
                // Never follow symlinks or junctions (avoids loops and double counting).
                if ft.is_symlink() {
                    continue;
                }
                let os_name = ent.file_name();
                let fname = os_name.to_string_lossy();
                let md = match ent.metadata() {
                    Ok(m) => m,
                    Err(e) => {
                        self.err(&ent.path(), &e);
                        continue;
                    }
                };
                let mt = sys::mtime(&md);

                if ft.is_dir() {
                    flags.mark_dir(&fname);
                    let lname = fname.to_lowercase();
                    let cnorm = rules::join_norm(&norm, &lname);
                    if self.excluded(&lname, &cnorm) {
                        continue;
                    }
                    if !self.cfg.cross_mounts && sys::dev(&md) != self.root_dev {
                        self.skip(ent.path());
                        continue;
                    }
                    if cfg!(target_os = "linux") && matches!(cnorm.as_str(), "/proc" | "/sys" | "/dev" | "/run") {
                        self.skip(ent.path());
                        continue;
                    }
                    subs.push(Sub { path: ent.path(), name: fname.into_owned(), norm: cnorm, lname, mtime: mt });
                    continue;
                }

                flags.mark_file(&fname);
                if mt > node.newest {
                    node.newest = mt;
                }
                let (mut disk, cloud) = sys::usage(&md);
                if let Some(key) = sys::link_key(&md) {
                    if !self.inodes.lock().unwrap().insert(key) {
                        disk = 0; // another hardlink to a file we already counted
                    }
                }
                own_files += 1;
                own_bytes += disk;
                own_cloud += cloud;

                let ext = ext_of(&fname);
                let key: Cow<str> = if ext.bytes().any(|c| c.is_ascii_uppercase()) {
                    Cow::Owned(ext.to_ascii_lowercase())
                } else {
                    Cow::Borrowed(ext)
                };
                match local.ext.get_mut(key.as_ref()) {
                    Some(v) => {
                        v.0 += disk;
                        v.1 += 1;
                    }
                    None => {
                        local.ext.insert(key.into(), (disk, 1));
                    }
                }

                if disk > 0 && (local.top.len() < top_n || local.top.peek().is_some_and(|m| disk > m.0.size)) {
                    local.top.push(Reverse(BigFile { size: disk, mtime: mt, path: ent.path() }));
                    if local.top.len() > top_n {
                        local.top.pop();
                    }
                }

                if disk >= self.cfg.min_file && !ctx.in_cand {
                    if let Some(r) = self.rules.match_file(&fname, protected) {
                        file_cands.push(Cand { rule: r, path: ent.path(), size: disk, files: 1, newest: mt, is_file: true });
                    }
                }
            }
        });

        let rule = if ctx.in_cand {
            None
        } else {
            self.rules.match_dir(&DirInfo {
                norm: &norm,
                parent_norm: ctx.parent_norm,
                lname: &lname,
                own: flags,
                parent: ctx.parent_flags,
                in_repo: ctx.in_repo,
                protected,
            })
        };
        let child_ctx = Ctx {
            parent_norm: &norm,
            parent_flags: flags,
            in_cand: ctx.in_cand || rule.is_some(),
            in_repo: ctx.in_repo || (flags.is_project() && !self.rules.is_home_or_above(&norm)),
            protected,
            depth: ctx.depth + 1,
        };

        self.files.fetch_add(own_files, Relaxed);
        self.dirs.fetch_add(1, Relaxed);
        self.bytes.fetch_add(own_bytes, Relaxed);

        let mut children: Vec<Node> = match subs.len() {
            0 => Vec::new(),
            1 => vec![self.scan_dir(subs.pop().unwrap(), child_ctx)],
            _ => subs.into_par_iter().map(|s| self.scan_dir(s, child_ctx)).collect(),
        };

        node.size = own_bytes;
        node.cloud = own_cloud;
        node.files = own_files;
        for c in &children {
            node.size += c.size;
            node.cloud += c.cloud;
            node.files += c.files;
            node.dirs += c.dirs + 1;
            node.newest = node.newest.max(c.newest);
        }

        if let Some(r) = rule {
            if node.size >= self.cfg.min_size.max(self.rules.list[r].min) {
                self.cands.lock().unwrap().push(Cand {
                    rule: r,
                    path,
                    size: node.size,
                    files: node.files,
                    newest: node.newest,
                    is_file: false,
                });
            }
        }
        if !file_cands.is_empty() {
            self.cands.lock().unwrap().append(&mut file_cands);
        }
        if ctx.depth < self.cfg.keep_depth {
            children.sort_unstable_by(|a, b| b.size.cmp(&a.size));
            node.children = children;
        }
        node
    }

    fn err(&self, p: &Path, e: &std::io::Error) {
        let n = self.errors.fetch_add(1, Relaxed);
        if (n as usize) < MAX_SAMPLES {
            let mut s = self.error_samples.lock().unwrap();
            if s.len() < MAX_SAMPLES {
                s.push(format!("{}: {}", p.display(), e));
            }
        }
    }

    fn skip(&self, p: PathBuf) {
        let n = self.skipped_n.fetch_add(1, Relaxed);
        if (n as usize) < MAX_SAMPLES {
            self.skipped.lock().unwrap().push(p.display().to_string());
        }
    }

    fn excluded(&self, lname: &str, norm: &str) -> bool {
        self.cfg.ex_names.iter().any(|n| n == lname) || self.cfg.ex_paths.iter().any(|p| p == norm)
    }
}
