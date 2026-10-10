//! File search + browse.
//!
//! Index: a parallel `ignore` walk (ripgrep/fd's walker: .gitignore, hidden
//! skipped, custom excludes) over the configured roots, run on a background
//! thread at startup and on open when stale. No filesystem watcher, so zero
//! work while idle. The index is one flat `Arc<Snapshot>`; a rescan builds a
//! new one and swaps it in, the old one is freed when its last reader drops.
//!
//! Search: nucleo-matcher (Helix's fuzzy matcher) on a worker thread, top-N
//! returned, re-ranked with frecency on the UI thread.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatchConfig, Matcher, Utf32Str};

use crate::config::{FilesConfig, APP_NAME};
use common::logging::log;

const RECENT_KEEP: usize = 50;
/// Candidates handed back by the matcher before frecency re-ranking
const CANDIDATES: usize = 300;

#[derive(Clone, Debug)]
pub struct FileHit {
    pub path: PathBuf,
    pub is_dir: bool,
}

pub struct Snapshot {
    /// Absolute paths; directories end with '/'
    paths: Vec<Box<str>>,
    /// Most recently modified files, newest first
    recent: Vec<Box<str>>,
    scanned_at: Instant,
}

static INDEX: RwLock<Option<Arc<Snapshot>>> = RwLock::new(None);
static SCANNING: AtomicBool = AtomicBool::new(false);

fn snapshot() -> Option<Arc<Snapshot>> {
    INDEX.read().ok()?.clone()
}

pub fn indexed_count() -> usize {
    snapshot().map_or(0, |s| s.paths.len())
}

pub fn is_scanning() -> bool {
    SCANNING.load(Ordering::Relaxed)
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// `/home/me/x` -> `~/x` (for display and matching)
pub fn display(path: &str) -> String {
    let h = home();
    match path.strip_prefix(&h) {
        Some(rest) if !h.is_empty() => format!("~{rest}"),
        _ => path.to_string(),
    }
}

fn hit(p: &str) -> FileHit {
    match p.strip_suffix('/') {
        Some(d) => FileHit { path: PathBuf::from(d), is_dir: true },
        None => FileHit { path: PathBuf::from(p), is_dir: false },
    }
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

struct Excludes {
    names: Vec<String>,
    prefixes: Vec<PathBuf>,
}

impl Excludes {
    fn new(list: &[String]) -> Self {
        let (prefixes, names): (Vec<_>, Vec<_>) = list.iter().partition(|e| e.starts_with('/'));
        Self {
            names: names.into_iter().cloned().collect(),
            prefixes: prefixes.into_iter().map(PathBuf::from).collect(),
        }
    }

    fn excluded(&self, path: &Path) -> bool {
        if self.prefixes.iter().any(|p| path.starts_with(p)) {
            return true;
        }
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| self.names.iter().any(|x| x == n))
    }
}

/// Daemon start: load the on-disk index right away (searchable instantly),
/// then rescan if the cache predates this boot (or doesn't exist).
pub fn init(cfg: &FilesConfig) {
    if !cfg.enabled || SCANNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let cfg = cfg.clone();
    std::thread::spawn(move || {
        let cache_time = std::fs::metadata(cache_path()).and_then(|m| m.modified()).ok();
        if let Some(snap) = load_cache(cache_time) {
            log(APP_NAME, &format!("loaded cached index ({} paths)", snap.paths.len()));
            publish(snap);
        }
        let fresh_this_boot = match (cache_time, boot_time()) {
            (Some(c), Some(b)) => c >= b,
            _ => false,
        };
        if fresh_this_boot {
            SCANNING.store(false, Ordering::SeqCst);
        } else {
            run_scan(&cfg); // first start since boot: full rescan
        }
    });
}

/// Rescan in the background if there's no index yet or it's older than
/// `rescan_secs`. Never blocks; a scan already running is left alone.
pub fn ensure_fresh(cfg: &FilesConfig) {
    if !cfg.enabled {
        return;
    }
    let stale = snapshot().is_none_or(|s| s.scanned_at.elapsed().as_secs() >= cfg.rescan_secs);
    if !stale || SCANNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let cfg = cfg.clone();
    std::thread::spawn(move || run_scan(&cfg));
}

/// Scan, publish, cache. Caller holds SCANNING; released here.
fn run_scan(cfg: &FilesConfig) {
    let t = Instant::now();
    let snap = scan(cfg);
    log(APP_NAME, &format!("indexed {} paths in {:?}", snap.paths.len(), t.elapsed()));
    save_cache(&snap);
    publish(snap);
    SCANNING.store(false, Ordering::SeqCst);
    // the walk's worker threads leave freed heap in their arenas: give it back
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Swap the new index in and tell the UI (re-runs a visible file search).
fn publish(snap: Snapshot) {
    if let Ok(mut w) = INDEX.write() {
        *w = Some(Arc::new(snap)); // previous snapshot freed here (or by its last reader)
    }
    glib::MainContext::default().invoke(crate::app::on_index_updated);
}

/// Network/FUSE mounts (rclone, gvfs, nfs, sshfs...): walking them is slow and
/// generates network traffic. Skipped unless a root is inside one.
fn remote_mounts(roots: &[String]) -> Vec<PathBuf> {
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else { return Vec::new() };
    mounts
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (_, mnt, fstype) = (f.next()?, f.next()?, f.next()?);
            let remote = fstype.starts_with("fuse.")
                || ["nfs", "nfs4", "cifs", "smb3", "sshfs", "9p", "davfs", "ceph"].contains(&fstype);
            let mnt = PathBuf::from(mnt.replace("\\040", " "));
            let rooted_inside = roots.iter().any(|r| Path::new(r).starts_with(&mnt));
            (remote && !rooted_inside).then_some(mnt)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// On-disk cache (~/.cache/launch-gui/files.idx): instant search after a restart
// ---------------------------------------------------------------------------

fn cache_path() -> PathBuf {
    common::paths::cache_dir(APP_NAME).join("files.idx")
}

/// System boot time from /proc/stat (btime)
fn boot_time() -> Option<SystemTime> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let secs: u64 = stat.lines().find_map(|l| l.strip_prefix("btime "))?.trim().parse().ok()?;
    Some(UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

fn save_cache(s: &Snapshot) {
    use std::io::Write;
    let path = cache_path();
    let tmp = path.with_extension("tmp");
    let Ok(f) = std::fs::File::create(&tmp) else { return };
    let mut w = std::io::BufWriter::new(f);
    let mut ok = writeln!(w, "#recent").is_ok();
    for p in s.recent.iter().filter(|p| !p.contains('\n')) {
        ok &= writeln!(w, "{p}").is_ok();
    }
    ok &= writeln!(w, "#paths").is_ok();
    for p in s.paths.iter().filter(|p| !p.contains('\n')) {
        ok &= writeln!(w, "{p}").is_ok();
    }
    if ok && w.flush().is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn load_cache(modified: Option<SystemTime>) -> Option<Snapshot> {
    let data = std::fs::read_to_string(cache_path()).ok()?;
    let mut lines = data.lines();
    if lines.next()? != "#recent" {
        return None;
    }
    let mut recent = Vec::new();
    let mut paths = Vec::new();
    let mut in_paths = false;
    for l in lines {
        if !in_paths && l == "#paths" {
            in_paths = true;
        } else if in_paths {
            paths.push(Box::<str>::from(l));
        } else {
            recent.push(Box::<str>::from(l));
        }
    }
    let age = modified.and_then(|m| m.elapsed().ok()).unwrap_or_default();
    Some(Snapshot {
        paths,
        recent,
        scanned_at: Instant::now().checked_sub(age).unwrap_or_else(Instant::now),
    })
}

fn scan(cfg: &FilesConfig) -> Snapshot {
    let mut exclude = cfg.exclude.clone();
    for m in remote_mounts(&cfg.roots) {
        log(APP_NAME, &format!("skipping remote mount {}", m.display()));
        exclude.push(m.to_string_lossy().into_owned());
    }
    let excludes = Arc::new(Excludes::new(&exclude));
    let roots: Vec<&String> = cfg.roots.iter().filter(|r| Path::new(r).is_dir()).collect();
    let paths = Mutex::new(Vec::<Box<str>>::with_capacity(200_000));
    let recent = Mutex::new(BinaryHeap::<Reverse<(u64, Box<str>)>>::new());

    let Some((first, rest)) = roots.split_first() else {
        return Snapshot { paths: Vec::new(), recent: Vec::new(), scanned_at: Instant::now() };
    };
    let mut builder = ignore::WalkBuilder::new(first);
    for r in rest {
        builder.add(r);
    }
    let ex = excludes.clone();
    builder
        .hidden(true) // dotfiles skipped, except roots given explicitly (~/.config)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .ignore(true)
        .parents(false)
        .follow_links(false)
        .threads(std::thread::available_parallelism().map_or(2, |n| n.get().min(4)))
        .filter_entry(move |e| !ex.excluded(e.path()));

    let mut factory = VisitorFactory { paths: &paths, recent: &recent };
    builder.build_parallel().visit(&mut factory);
    // every Visitor has been dropped (and flushed) once visit() returns
    let mut paths = paths.into_inner().unwrap_or_default();
    paths.sort_unstable();
    paths.dedup();

    let mut recent: Vec<(u64, Box<str>)> = recent
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .map(|Reverse(x)| x)
        .collect();
    recent.sort_unstable_by(|a, b| b.0.cmp(&a.0));

    Snapshot {
        paths,
        recent: recent.into_iter().map(|(_, p)| p).collect(),
        scanned_at: Instant::now(),
    }
}

type RecentHeap = Mutex<BinaryHeap<Reverse<(u64, Box<str>)>>>;

/// One per walker thread. Buffers locally (few lock round-trips) and flushes
/// on Drop, so nothing buffered is lost when the walk ends.
struct Visitor<'s> {
    paths: &'s Mutex<Vec<Box<str>>>,
    recent: &'s RecentHeap,
    local: Vec<Box<str>>,
    local_recent: Vec<(u64, Box<str>)>,
}

struct VisitorFactory<'s> {
    paths: &'s Mutex<Vec<Box<str>>>,
    recent: &'s RecentHeap,
}

impl<'s> ignore::ParallelVisitorBuilder<'s> for VisitorFactory<'s> {
    fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
        Box::new(Visitor {
            paths: self.paths,
            recent: self.recent,
            local: Vec::with_capacity(4096),
            local_recent: Vec::new(),
        })
    }
}

impl Visitor<'_> {
    fn flush(&mut self) {
        if !self.local.is_empty() {
            if let Ok(mut p) = self.paths.lock() {
                p.append(&mut self.local);
            }
        }
        flush_recent(self.recent, &mut self.local_recent);
    }
}

impl ignore::ParallelVisitor for Visitor<'_> {
    fn visit(&mut self, res: Result<ignore::DirEntry, ignore::Error>) -> ignore::WalkState {
        let Ok(entry) = res else { return ignore::WalkState::Continue };
        if entry.depth() == 0 {
            return ignore::WalkState::Continue; // the root itself
        }
        let Some(s) = entry.path().to_str() else { return ignore::WalkState::Continue };
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            self.local.push(format!("{s}/").into_boxed_str());
        } else {
            self.local.push(s.into());
            if let Ok(m) = entry.metadata().and_then(|m| m.modified().map_err(Into::into)) {
                let secs = m.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                self.local_recent.push((secs, s.into()));
            }
        }
        if self.local.len() >= 4096 || self.local_recent.len() >= RECENT_KEEP * 4 {
            self.flush();
        }
        ignore::WalkState::Continue
    }
}

impl Drop for Visitor<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

fn flush_recent(heap: &RecentHeap, local: &mut Vec<(u64, Box<str>)>) {
    if local.is_empty() {
        return;
    }
    if let Ok(mut h) = heap.lock() {
        for item in local.drain(..) {
            h.push(Reverse(item));
            if h.len() > RECENT_KEEP {
                h.pop(); // drop the oldest
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Searching
// ---------------------------------------------------------------------------

/// Fuzzy match `query` against the index (call off the UI thread).
/// Returns up to CANDIDATES (score, path) pairs, best first.
pub fn search_index(query: &str) -> Vec<(u32, Box<str>)> {
    let Some(snap) = snapshot() else { return Vec::new() };
    let h = home();
    let mut matcher = Matcher::new(MatchConfig::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();

    let mut scored: Vec<(u32, usize)> = Vec::new();
    for (i, p) in snap.paths.iter().enumerate() {
        // match on the ~-relative form so /home/user doesn't produce noise
        let hay = p.strip_prefix(h.as_str()).unwrap_or(p);
        if let Some(s) = pattern.score(Utf32Str::new(hay, &mut buf), &mut matcher) {
            scored.push((s, i));
        }
    }
    if scored.len() > CANDIDATES {
        scored.select_nth_unstable_by(CANDIDATES, |a, b| b.0.cmp(&a.0));
        scored.truncate(CANDIDATES);
    }
    scored.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(s, i)| (s, snap.paths[i].clone())).collect()
}

/// Final ranking on the UI thread: fuzzy score + frecency + filename bonus.
pub fn rank(query: &str, candidates: Vec<(u32, Box<str>)>, limit: usize) -> Vec<FileHit> {
    let q = query.to_lowercase();
    let mut ranked: Vec<(i64, Box<str>)> = candidates
        .into_iter()
        .map(|(s, p)| {
            let name = p.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_lowercase();
            let name_bonus = if name.contains(&q) { 60 } else { 0 };
            let score = s as i64 + name_bonus + frecency_bonus(&p);
            (score, p)
        })
        .collect();
    ranked.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    ranked.into_iter().take(limit).map(|(_, p)| hit(&p)).collect()
}

/// Empty file-search query: frecent files first, then recently modified.
pub fn suggestions(limit: usize) -> Vec<FileHit> {
    let mut out: Vec<FileHit> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in frecent(limit) {
        if seen.insert(p.clone()) {
            out.push(hit(&p));
        }
    }
    if let Some(snap) = snapshot() {
        for p in snap.recent.iter() {
            if out.len() >= limit {
                break;
            }
            if seen.insert(p.to_string()) {
                out.push(hit(p));
            }
        }
    }
    out.truncate(limit);
    out
}

// ---------------------------------------------------------------------------
// Browse (Tab / ~/ / paths)
// ---------------------------------------------------------------------------

/// Entries of `dir` filtered by `filter` (fuzzy), folders first.
/// Hidden entries only when the filter starts with '.'.
pub fn list_dir(dir: &Path, filter: &str, limit: usize) -> Vec<FileHit> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let show_hidden = filter.starts_with('.');
    let mut items: Vec<(String, bool)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && !show_hidden {
                return None;
            }
            // follow symlinks for the folder check
            let is_dir = e.path().is_dir();
            Some((name, is_dir))
        })
        .collect();

    if filter.is_empty() {
        items.sort_by_cached_key(|(n, d)| (!*d, n.to_lowercase()));
    } else {
        let mut matcher = Matcher::new(MatchConfig::DEFAULT);
        let pattern = Pattern::parse(filter, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, (String, bool))> = items
            .into_iter()
            .filter_map(|it| {
                let s = pattern.score(Utf32Str::new(&it.0, &mut buf), &mut matcher)?;
                Some((s, it))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        items = scored.into_iter().map(|(_, it)| it).collect();
    }

    items
        .into_iter()
        .take(limit)
        .map(|(name, is_dir)| FileHit { path: dir.join(name), is_dir })
        .collect()
}

// ---------------------------------------------------------------------------
// Opening + frecency (~/.local/state/launch-gui/files.frecency: "count\tlast\tpath")
// ---------------------------------------------------------------------------

thread_local! {
    static FRECENCY: std::cell::RefCell<HashMap<String, (u32, u64)>> =
        std::cell::RefCell::new(load_frecency());
}

const FRECENCY_MAX: usize = 500;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn frecency_path() -> PathBuf {
    common::logging::log_dir(APP_NAME).join("files.frecency")
}

fn load_frecency() -> HashMap<String, (u32, u64)> {
    std::fs::read_to_string(frecency_path())
        .map(|s| {
            s.lines()
                .filter_map(|l| {
                    let mut it = l.splitn(3, '\t');
                    let n = it.next()?.parse().ok()?;
                    let t = it.next()?.parse().ok()?;
                    Some((it.next()?.to_string(), (n, t)))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save_frecency(map: &HashMap<String, (u32, u64)>) {
    let path = frecency_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let body: String = map.iter().map(|(p, (n, t))| format!("{n}\t{t}\t{p}\n")).collect();
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

/// Recency-weighted count
fn weight(count: u32, last: u64) -> i64 {
    let age_days = now().saturating_sub(last) / 86_400;
    let recency = match age_days {
        0 => 4,
        1..=6 => 2,
        7..=30 => 1,
        _ => 0,
    };
    (count.min(50) as i64) * (1 + recency)
}

fn frecency_bonus(path: &str) -> i64 {
    FRECENCY.with(|f| f.borrow().get(path).map_or(0, |&(n, t)| weight(n, t) * 15))
}

fn frecent(limit: usize) -> Vec<String> {
    FRECENCY.with(|f| {
        let map = f.borrow();
        let mut v: Vec<(&String, i64)> = map
            .iter()
            .filter(|(p, _)| Path::new(p.trim_end_matches('/')).exists())
            .map(|(p, &(n, t))| (p, weight(n, t)))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.into_iter().take(limit).map(|(p, _)| p.clone()).collect()
    })
}

/// Record an open, trimming the table so it can't grow without bound.
pub fn record_open(hit: &FileHit) {
    let key = if hit.is_dir {
        format!("{}/", hit.path.display())
    } else {
        hit.path.display().to_string()
    };
    FRECENCY.with(|f| {
        let mut map = f.borrow_mut();
        let e = map.entry(key).or_insert((0, 0));
        e.0 += 1;
        e.1 = now();
        if map.len() > FRECENCY_MAX {
            let mut v: Vec<(String, i64)> =
                map.iter().map(|(p, &(n, t))| (p.clone(), weight(n, t))).collect();
            v.sort_by(|a, b| a.1.cmp(&b.1));
            for (p, _) in v.into_iter().take(map.len() - FRECENCY_MAX) {
                map.remove(&p);
            }
        }
        save_frecency(&map);
    });
}
