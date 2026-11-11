//! Rendering: colourful terminal report, JSON for agents/scripts, volume list.
use crate::json::J;
use crate::rules::{Rules, Safety};
use crate::scan::{BigFile, Cand, Node};
use crate::sys::{self, Volume};
use crate::util::{age_days, age_str, human, thousands};
use crate::Opts;
use std::fmt::Write;
use std::path::{Path, PathBuf};

pub struct Report<'a> {
    pub root: PathBuf,
    pub tree: Node,
    pub elapsed_ms: u128,
    pub threads: usize,
    pub errors: u64,
    pub error_samples: Vec<String>,
    pub skipped: Vec<String>,
    pub skipped_n: u64,
    pub files: Vec<BigFile>,
    pub exts: Vec<(String, u64, u64)>,
    pub cands: Vec<Cand>,
    pub volume: Option<Volume>,
    pub now: i64,
    pub rules: &'a Rules,
}

struct Group<'a> {
    rule: usize,
    size: u64,
    items: Vec<&'a Cand>,
}

fn groups<'a>(r: &'a Report) -> Vec<Group<'a>> {
    let mut gs: Vec<Group> = Vec::new();
    for c in &r.cands {
        match gs.iter_mut().find(|g| g.rule == c.rule) {
            Some(g) => {
                g.size += c.size;
                g.items.push(c);
            }
            None => gs.push(Group { rule: c.rule, size: c.size, items: vec![c] }),
        }
    }
    for g in &mut gs {
        g.items.sort_unstable_by(|a, b| b.size.cmp(&a.size));
    }
    gs.sort_unstable_by(|a, b| b.size.cmp(&a.size));
    gs
}

fn totals(r: &Report, gs: &[Group]) -> [u64; 5] {
    let mut t = [0u64; 5];
    for g in gs {
        t[r.rules.list[g.rule].safety as usize] += g.size;
    }
    t
}

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

/// children worth showing + (count, size) of the rest folded into "N smaller"
fn visible(n: &Node, thr: u64) -> (Vec<&Node>, usize, u64) {
    let mut vis = Vec::new();
    let (mut hn, mut hs) = (0usize, 0u64);
    for c in &n.children {
        if c.size >= thr && c.size > 0 {
            vis.push(c);
        } else {
            hn += 1;
            hs += c.size;
        }
    }
    (vis, hn, hs)
}

fn own_files(n: &Node) -> (u64, u64) {
    let (s, f) = n.children.iter().fold((0, 0), |(s, f), c| (s + c.size, f + c.files));
    (n.size.saturating_sub(s), n.files.saturating_sub(f))
}

// =============================================================== terminal

struct St(bool);
impl St {
    fn c(&self, code: &str, s: &str) -> String {
        if self.0 {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn safety(&self, s: Safety) -> String {
        self.c(safety_code(s), &format!("{:<6}", s.tag()))
    }
    fn size(&self, b: u64, total: u64) -> String {
        let s = format!("{:>9}", human(b));
        if pct(b, total) >= 10.0 || b >= 10 << 30 {
            self.c("1;33", &s)
        } else if b >= 1 << 30 {
            self.c("33", &s)
        } else {
            s
        }
    }
}

fn safety_code(s: Safety) -> &'static str {
    match s {
        Safety::Safe => "1;32",
        Safety::Regen => "1;36",
        Safety::Review => "1;33",
        Safety::System => "1;35",
        Safety::Keep => "1;31",
    }
}

fn bar(frac: f64, width: usize) -> String {
    const PART: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let eighths = (frac.clamp(0.0, 1.0) * width as f64 * 8.0).round() as usize;
    let full = (eighths / 8).min(width);
    let mut s: String = "█".repeat(full);
    let mut used = full;
    if full < width && eighths % 8 > 0 {
        s.push(PART[eighths % 8 - 1]);
        used += 1;
    }
    s.extend(std::iter::repeat(' ').take(width - used));
    s
}

fn heading(out: &mut String, st: &St, title: &str, sub: &str) {
    let _ = writeln!(out, "\n {}  {}", st.c("1;36", title), st.c("2", sub));
}

pub fn human_report(r: &Report, o: &Opts, color: bool) -> String {
    let st = St(color);
    let mut out = String::with_capacity(16 * 1024);
    let t = &r.tree;
    let total = t.size.max(1);

    // ---- header
    let _ = writeln!(out);
    let _ = writeln!(out, " {}  {}", st.c("1;36", "space-ferret"), st.c("1", &r.root.display().to_string()));
    let cloud = if t.cloud > 0 {
        st.c("2", &format!("   (+{} in cloud-only placeholders, not on this disk)", human(t.cloud)))
    } else {
        String::new()
    };
    let _ = writeln!(out, " {}  {} on disk{}", st.c("2", "total "), st.c("1;33", &human(t.size)), cloud);
    let _ = writeln!(
        out,
        " {}  {} files · {} folders · {:.2}s on {} threads",
        st.c("2", "scan  "),
        thousands(t.files),
        thousands(t.dirs + 1),
        r.elapsed_ms as f64 / 1000.0,
        r.threads
    );
    if let Some(v) = &r.volume {
        let used = v.total.saturating_sub(v.free);
        let up = pct(used, v.total);
        let free = format!("{} free", human(v.free));
        let free = if up >= 90.0 { st.c("1;31", &free) } else if up >= 75.0 { st.c("33", &free) } else { st.c("32", &free) };
        let _ = writeln!(
            out,
            " {}  {} {}  {} of {} used ({:.0}%) · {}",
            st.c("2", "volume"),
            v.mount,
            st.c("2", &v.fs),
            human(used),
            human(v.total),
            up,
            free
        );
    }
    if r.errors > 0 {
        let _ = writeln!(
            out,
            " {}  {} entries could not be read (access denied etc.); totals may be low. Run elevated for full numbers.",
            st.c("33", "note  "),
            thousands(r.errors)
        );
    }
    if r.skipped_n > 0 {
        let list: Vec<&str> = r.skipped.iter().take(4).map(|s| s.as_str()).collect();
        let more = if r.skipped_n as usize > list.len() { format!(" +{} more", r.skipped_n as usize - list.len()) } else { String::new() };
        let _ = writeln!(
            out,
            " {}  skipped {} other filesystem(s): {}{}  (--cross-mounts to include)",
            st.c("2", "note  "),
            r.skipped_n,
            list.join(", "),
            more
        );
    }

    // ---- tree
    let thr = ((total as f64 * o.min_pct / 100.0) as u64).max(1);
    heading(&mut out, &st, "BREAKDOWN", &format!("depth {}, entries ≥ {}% of total", o.depth, o.min_pct));
    let _ = writeln!(
        out,
        "  {} {} {} {}",
        st.size(t.size, total),
        st.c("2", "100.0%"),
        bar(1.0, 10),
        st.c("1", &r.root.display().to_string())
    );
    if o.depth > 0 {
        tree_lines(&mut out, &st, t, "", total, thr, r.now);
    }

    // ---- largest files
    if !r.files.is_empty() {
        heading(&mut out, &st, "LARGEST FILES", "");
        for f in &r.files {
            let _ = writeln!(
                out,
                "  {} {:>6}  {}",
                st.size(f.size, total),
                st.c("2", &age_str(age_days(r.now, f.mtime))),
                f.path.display()
            );
        }
    }

    // ---- file types
    if !r.exts.is_empty() {
        heading(&mut out, &st, "BY FILE TYPE", "");
        for (ext, size, n) in r.exts.iter().take(o.top) {
            let name = if ext.is_empty() { "(no ext)".to_string() } else { format!(".{ext}") };
            let _ = writeln!(
                out,
                "  {} {} {:<12} {}",
                st.size(*size, total),
                st.c("2", &format!("{:>5.1}%", pct(*size, total))),
                name,
                st.c("2", &format!("{} files", thousands(*n)))
            );
        }
    }

    // ---- cleanup candidates
    let gs = groups(r);
    heading(
        &mut out,
        &st,
        "CLEANUP CANDIDATES",
        &format!("folders ≥ {}, files ≥ {}; read-only scan, nothing was deleted", human(o.min_size), human(o.min_file)),
    );
    if gs.is_empty() {
        let _ = writeln!(out, "  none found above the size thresholds");
    } else {
        let tt = totals(r, &gs);
        let parts: Vec<String> = Safety::ALL
            .iter()
            .filter(|s| tt[**s as usize] > 0)
            .map(|s| format!("{} {}", st.c(safety_code(*s), s.tag()), human(tt[*s as usize])))
            .collect();
        let _ = writeln!(out, "  {}\n", parts.join(st.c("2", "  ·  ").as_str()));
        let per = 4usize;
        for g in gs.iter().take(o.top) {
            let rule = &r.rules.list[g.rule];
            let count = if g.items.len() > 1 { st.c("2", &format!("  ×{}", g.items.len())) } else { String::new() };
            let _ = writeln!(out, "  {} {}  {}{}", st.safety(rule.safety), st.size(g.size, total), st.c("1", rule.label), count);
            let _ = writeln!(out, "  {:18}{} {}", "", st.c("2", "→"), st.c("2", rule.hint));
            for c in g.items.iter().take(per) {
                let _ = writeln!(
                    out,
                    "  {:18}{} {:>6}  {}",
                    "",
                    st.size(c.size, total),
                    st.c("2", &age_str(age_days(r.now, c.newest))),
                    c.path.display()
                );
            }
            if g.items.len() > per {
                let rest: u64 = g.items[per..].iter().map(|c| c.size).sum();
                let _ = writeln!(out, "  {:18}{}", "", st.c("2", &format!("… {} more ({})", g.items.len() - per, human(rest))));
            }
        }
        if gs.len() > o.top {
            let rest: u64 = gs[o.top..].iter().map(|g| g.size).sum();
            let _ = writeln!(out, "  {}", st.c("2", &format!("… {} more kinds ({}); raise --top to see them", gs.len() - o.top, human(rest))));
        }
        let _ = writeln!(
            out,
            "\n  {}",
            st.c("2", "safe = recreated automatically · regen = rebuild/reinstall restores it · review = ask first · system = use the OS/tool command · keep = don't touch")
        );
    }
    let _ = writeln!(out, "  {}\n", st.c("2", "age = time since anything inside changed · drill down with: space-ferret <path> · machine-readable: space-ferret --json"));
    out
}

fn tree_lines(out: &mut String, st: &St, n: &Node, prefix: &str, total: u64, thr: u64, now: i64) {
    enum E<'a> {
        Dir(&'a Node),
        Files(u64, u64),
        More(usize, u64),
    }
    let (vis, hidden_n, hidden_s) = visible(n, thr);
    let mut entries: Vec<E> = vis.into_iter().map(E::Dir).collect();
    let (fs, fc) = own_files(n);
    if fs >= thr && fs > 0 && !n.children.is_empty() {
        let at = entries.iter().position(|e| matches!(e, E::Dir(d) if d.size < fs)).unwrap_or(entries.len());
        entries.insert(at, E::Files(fs, fc));
    }
    if hidden_n > 0 && hidden_s > 0 {
        entries.push(E::More(hidden_n, hidden_s));
    }
    let count = entries.len();
    for (i, e) in entries.into_iter().enumerate() {
        let last = i + 1 == count;
        let conn = if last { "└─ " } else { "├─ " };
        match e {
            E::Dir(c) => {
                let mut label = c.name.clone();
                if c.unreadable {
                    label.push_str("  (no access)");
                }
                let _ = writeln!(
                    out,
                    "  {} {} {} {}{}{}  {}",
                    st.size(c.size, total),
                    st.c("2", &format!("{:>5.1}%", pct(c.size, total))),
                    bar(c.size as f64 / total as f64, 10),
                    st.c("2", prefix),
                    st.c("2", conn),
                    label,
                    st.c("2", &age_str(age_days(now, c.newest)))
                );
                if !c.children.is_empty() {
                    let np = format!("{}{}", prefix, if last { "   " } else { "│  " });
                    tree_lines(out, st, c, &np, total, thr, now);
                }
            }
            E::Files(s, c) => {
                let _ = writeln!(
                    out,
                    "  {} {} {} {}{}{}",
                    st.size(s, total),
                    st.c("2", &format!("{:>5.1}%", pct(s, total))),
                    bar(s as f64 / total as f64, 10),
                    st.c("2", prefix),
                    st.c("2", conn),
                    st.c("2", &format!("[{} files directly in this folder]", thousands(c)))
                );
            }
            E::More(k, s) => {
                let _ = writeln!(
                    out,
                    "  {} {} {} {}{}{}",
                    st.size(s, total),
                    st.c("2", &format!("{:>5.1}%", pct(s, total))),
                    bar(s as f64 / total as f64, 10),
                    st.c("2", prefix),
                    st.c("2", conn),
                    st.c("2", &format!("… {k} smaller folders"))
                );
            }
        }
    }
}

// =================================================================== JSON

fn sz(b: u64) -> J {
    J::s(human(b))
}

fn tree_json(n: &Node, path: &Path, total: u64, thr: u64, now: i64, keep_children: bool) -> J {
    let mut o = vec![
        ("name", J::s(n.name.clone())),
        ("path", J::s(path.display().to_string())),
        ("size", J::U(n.size)),
        ("size_h", sz(n.size)),
        ("pct", J::F(pct(n.size, total))),
        ("files", J::U(n.files)),
        ("dirs", J::U(n.dirs)),
        ("age_days", J::opt_i(age_days(now, n.newest))),
    ];
    if n.cloud > 0 {
        o.push(("cloud_only", J::U(n.cloud)));
    }
    if n.unreadable {
        o.push(("unreadable", J::B(true)));
    }
    if keep_children && !n.children.is_empty() {
        let (vis, hn, hs) = visible(n, thr);
        let (fs, fc) = own_files(n);
        if fs > 0 {
            o.push(("own_files", J::O(vec![("count", J::U(fc)), ("size", J::U(fs)), ("size_h", sz(fs))])));
        }
        if hn > 0 {
            o.push(("smaller_dirs", J::O(vec![("count", J::U(hn as u64)), ("size", J::U(hs)), ("size_h", sz(hs))])));
        }
        o.push((
            "children",
            J::A(vis.into_iter().map(|c| tree_json(c, &path.join(&c.name), total, thr, now, true)).collect()),
        ));
    }
    J::O(o)
}

pub fn json_report(r: &Report, o: &Opts) -> String {
    let t = &r.tree;
    let total = t.size.max(1);
    let thr = ((total as f64 * o.min_pct / 100.0) as u64).max(1);
    let gs = groups(r);
    let tt = totals(r, &gs);

    let volume = match &r.volume {
        Some(v) => {
            let used = v.total.saturating_sub(v.free);
            J::O(vec![
                ("mount", J::s(v.mount.clone())),
                ("fs", J::s(v.fs.clone())),
                ("total", J::U(v.total)),
                ("total_h", sz(v.total)),
                ("free", J::U(v.free)),
                ("free_h", sz(v.free)),
                ("used_pct", J::F(pct(used, v.total))),
            ])
        }
        None => J::Null,
    };

    let tree_o = tree_json(t, &r.root, total, thr, r.now, o.depth > 0);

    let groups_j: Vec<J> = gs
        .iter()
        .map(|g| {
            let rule = &r.rules.list[g.rule];
            let items: Vec<J> = g
                .items
                .iter()
                .take(o.top)
                .map(|c| {
                    let mut it = vec![
                        ("path", J::s(c.path.display().to_string())),
                        ("size", J::U(c.size)),
                        ("size_h", sz(c.size)),
                        ("age_days", J::opt_i(age_days(r.now, c.newest))),
                    ];
                    if !c.is_file {
                        it.push(("files", J::U(c.files)));
                    }
                    J::O(it)
                })
                .collect();
            let mut go = vec![
                ("id", J::s(rule.id)),
                ("label", J::s(rule.label)),
                ("safety", J::s(rule.safety.tag())),
                ("hint", J::s(rule.hint)),
                ("count", J::U(g.items.len() as u64)),
                ("size", J::U(g.size)),
                ("size_h", sz(g.size)),
            ];
            if g.items.len() > o.top {
                let rest: u64 = g.items[o.top..].iter().map(|c| c.size).sum();
                go.push(("not_listed", J::O(vec![("count", J::U((g.items.len() - o.top) as u64)), ("size_h", sz(rest))])));
            }
            go.push(("items", J::A(items)));
            J::O(go)
        })
        .collect();

    let totals_j = J::O(
        Safety::ALL
            .iter()
            .map(|s| (s.tag(), J::O(vec![("size", J::U(tt[*s as usize])), ("size_h", sz(tt[*s as usize]))])))
            .collect(),
    );
    let legend = J::O(Safety::ALL.iter().map(|s| (s.tag(), J::s(s.meaning()))).collect());

    J::O(vec![
        ("tool", J::s("space-ferret")),
        ("version", J::s(crate::VERSION)),
        ("platform", J::s(std::env::consts::OS)),
        ("root", J::s(r.root.display().to_string())),
        ("size", J::U(t.size)),
        ("size_h", sz(t.size)),
        ("cloud_only", J::U(t.cloud)),
        ("files", J::U(t.files)),
        ("dirs", J::U(t.dirs + 1)),
        ("elapsed_ms", J::U(r.elapsed_ms as u64)),
        ("size_basis", J::s(sys::SIZE_BASIS)),
        ("volume", volume),
        (
            "unreadable",
            J::O(vec![
                ("count", J::U(r.errors)),
                ("samples", J::A(r.error_samples.iter().map(|s| J::s(s.clone())).collect())),
            ]),
        ),
        (
            "skipped_mounts",
            J::O(vec![
                ("count", J::U(r.skipped_n)),
                ("samples", J::A(r.skipped.iter().map(|s| J::s(s.clone())).collect())),
            ]),
        ),
        (
            "options",
            J::O(vec![
                ("depth", J::U(o.depth as u64)),
                ("min_pct", J::F(o.min_pct)),
                ("min_size", J::U(o.min_size)),
                ("min_file", J::U(o.min_file)),
                ("top", J::U(o.top as u64)),
            ]),
        ),
        ("tree", tree_o),
        (
            "largest_files",
            J::A(
                r.files
                    .iter()
                    .map(|f| {
                        J::O(vec![
                            ("path", J::s(f.path.display().to_string())),
                            ("size", J::U(f.size)),
                            ("size_h", sz(f.size)),
                            ("age_days", J::opt_i(age_days(r.now, f.mtime))),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "file_types",
            J::A(
                r.exts
                    .iter()
                    .take(o.top)
                    .map(|(e, s, n)| {
                        J::O(vec![
                            ("ext", J::s(e.clone())),
                            ("size", J::U(*s)),
                            ("size_h", sz(*s)),
                            ("pct", J::F(pct(*s, total))),
                            ("files", J::U(*n)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("cleanup", J::O(vec![("totals", totals_j), ("groups", J::A(groups_j))])),
        ("safety_legend", legend),
    ])
    .render()
}

// ================================================================= drives

pub fn drives_human(vols: &[Volume], color: bool) -> String {
    let st = St(color);
    let mut out = String::new();
    let w = vols.iter().map(|v| v.mount.chars().count()).max().unwrap_or(5).max(5);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {}",
        st.c("1;36", &format!("{:<w$}  {:<8} {:>9}  {:>9}  {:>9}  {:>5}", "MOUNT", "FS", "SIZE", "USED", "FREE", "USE%"))
    );
    for v in vols {
        let used = v.total.saturating_sub(v.free);
        let up = pct(used, v.total);
        let code = if up >= 90.0 { "1;31" } else if up >= 75.0 { "33" } else { "32" };
        let _ = writeln!(
            out,
            "  {:<w$}  {:<8} {:>9}  {:>9}  {}  {}  {}",
            v.mount,
            v.fs,
            human(v.total),
            human(used),
            st.c(code, &format!("{:>9}", human(v.free))),
            st.c(code, &format!("{:>4.0}%", up)),
            st.c(code, &bar(up / 100.0, 12)),
        );
    }
    let _ = writeln!(out);
    out
}

pub fn drives_json(vols: &[Volume]) -> String {
    J::A(
        vols.iter()
            .map(|v| {
                let used = v.total.saturating_sub(v.free);
                J::O(vec![
                    ("mount", J::s(v.mount.clone())),
                    ("fs", J::s(v.fs.clone())),
                    ("label", J::s(v.label.clone())),
                    ("total", J::U(v.total)),
                    ("total_h", sz(v.total)),
                    ("used", J::U(used)),
                    ("free", J::U(v.free)),
                    ("free_h", sz(v.free)),
                    ("used_pct", J::F(pct(used, v.total))),
                ])
            })
            .collect(),
    )
    .render()
}
