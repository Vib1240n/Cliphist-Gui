use crate::config::APP_NAME;
use common::cli::spawn_reaped;
use common::css::char_truncate;
use common::logging::log;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const THUMB_SIZE: u32 = 64;
const PREVIEW_CHARS: usize = 200;

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

/// FNV-1a 64-bit. Stable across Rust versions and platforms. The previous
/// `DefaultHasher` is explicitly not, so a toolchain update silently changed
/// every pin's identity.
pub fn stable_hash(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", h)
}

fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn text_preview(data: &[u8]) -> String {
    char_truncate(&String::from_utf8_lossy(data), PREVIEW_CHARS)
}

// ---------------------------------------------------------------------------
// Pinned storage: ~/.config/cliphist-gui/pinned/{index.json,text/,media/}
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PinnedMeta {
    pub hash: String,
    pub content_type: String,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub pinned_at: u64,
    #[serde(default)]
    pub order: usize,
}

impl PinnedMeta {
    pub fn is_image(&self) -> bool {
        self.content_type == "image"
    }
}

#[derive(Clone, Debug)]
pub struct PinnedClipEntry {
    pub meta: PinnedMeta,
    /// Where the thumbnail lives (or will, once the background worker makes it)
    pub thumb_path: Option<PathBuf>,
    pub data_path: PathBuf,
}

pub fn pinned_dir() -> PathBuf {
    let d = common::paths::config_dir(APP_NAME).join("pinned");
    std::fs::create_dir_all(d.join("text")).ok();
    std::fs::create_dir_all(d.join("media")).ok();
    d
}

fn index_path() -> PathBuf {
    pinned_dir().join("index.json")
}

fn data_path(dir: &Path, content_type: &str, hash: &str) -> PathBuf {
    if content_type == "image" {
        dir.join("media").join(format!("{hash}.png"))
    } else {
        dir.join("text").join(format!("{hash}.txt"))
    }
}

fn pinned_thumb_path(hash: &str) -> PathBuf {
    thumb_cache().join(format!("pinned_{hash}.png"))
}

/// Read index.json. If it is missing or unparseable, rebuild it from the data
/// files so pins are never lost to a bad index.
fn read_index() -> Vec<PinnedMeta> {
    let path = index_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => match serde_json::from_str::<Vec<PinnedMeta>>(&s) {
            Ok(v) => v,
            Err(e) => {
                log(APP_NAME, &format!("index.json unreadable ({e}), rebuilding from files"));
                let _ = std::fs::copy(&path, path.with_extension("json.broken"));
                rebuild_index_from_files()
            }
        },
        Err(_) => rebuild_index_from_files(),
    }
}

fn rebuild_index_from_files() -> Vec<PinnedMeta> {
    let dir = pinned_dir();
    let mut out = Vec::new();
    for (sub, ctype) in [("text", "text"), ("media", "image")] {
        let Ok(rd) = std::fs::read_dir(dir.join(sub)) else { continue };
        for f in rd.flatten() {
            let p = f.path();
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
            let preview = if ctype == "text" {
                std::fs::read(&p).map(|d| text_preview(&d)).unwrap_or_default()
            } else {
                "[[ pinned image ]]".to_string()
            };
            let pinned_at = f
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            out.push(PinnedMeta {
                hash: stem.to_string(),
                content_type: ctype.to_string(),
                preview,
                pinned_at,
                order: 0,
            });
        }
    }
    out.sort_by_key(|m| m.pinned_at);
    out
}

fn save_index(entries: &[PinnedMeta]) {
    let path = index_path();
    let tmp = path.with_extension("json.tmp");
    match serde_json::to_string_pretty(entries) {
        Ok(json) => {
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
        Err(e) => log(APP_NAME, &format!("index.json serialize failed: {e}")),
    }
}

/// Load the index and repair it in place:
/// - re-key every pin by `stable_hash` of its data (migrates old DefaultHasher keys)
/// - regenerate text previews from the stored data (fixes previews the old
///   hand-rolled parser truncated)
/// - drop entries whose data file is gone, and duplicates
/// - renumber `order` 0..n
fn load_index() -> Vec<PinnedMeta> {
    let dir = pinned_dir();
    let mut metas = read_index();
    metas.sort_by_key(|m| (m.order, m.pinned_at));

    let mut changed = false;
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(metas.len());

    for mut m in metas {
        let path = data_path(&dir, &m.content_type, &m.hash);
        let Ok(data) = std::fs::read(&path) else {
            changed = true;
            continue;
        };
        let h = stable_hash(&data);
        if h != m.hash {
            let new_path = data_path(&dir, &m.content_type, &h);
            if std::fs::rename(&path, &new_path).is_err() {
                continue;
            }
            let _ = std::fs::remove_file(pinned_thumb_path(&m.hash));
            log(APP_NAME, &format!("migrated pin {} -> {}", m.hash, h));
            m.hash = h;
            changed = true;
        }
        if !m.is_image() {
            let p = text_preview(&data);
            if p != m.preview {
                m.preview = p;
                changed = true;
            }
        }
        if !seen.insert(m.hash.clone()) {
            changed = true;
            continue;
        }
        out.push(m);
    }

    for (i, m) in out.iter_mut().enumerate() {
        if m.order != i {
            m.order = i;
            changed = true;
        }
    }
    if changed {
        save_index(&out);
    }
    out
}

pub fn load_pinned() -> Vec<PinnedClipEntry> {
    let dir = pinned_dir();
    load_index()
        .into_iter()
        .map(|meta| PinnedClipEntry {
            data_path: data_path(&dir, &meta.content_type, &meta.hash),
            thumb_path: meta.is_image().then(|| pinned_thumb_path(&meta.hash)),
            meta,
        })
        .collect()
}

pub fn pin_entry(entry: &ClipEntry, max_pinned: usize) -> Result<(), String> {
    let mut metas = load_index();
    if metas.len() >= max_pinned {
        return Err(format!("Maximum {max_pinned} pinned items reached"));
    }

    let data = decode_entry(entry)?;
    let hash = stable_hash(&data);
    if metas.iter().any(|m| m.hash == hash) {
        return Err("Already pinned".to_string());
    }

    let content_type = if entry.is_image { "image" } else { "text" };
    std::fs::write(data_path(&pinned_dir(), content_type, &hash), &data).map_err(|e| e.to_string())?;

    metas.push(PinnedMeta {
        hash,
        content_type: content_type.to_string(),
        preview: if entry.is_image {
            entry.preview.clone()
        } else {
            text_preview(&data)
        },
        pinned_at: now_timestamp(),
        order: metas.len(),
    });
    save_index(&metas);
    Ok(())
}

pub fn unpin_entry(hash: &str) {
    let dir = pinned_dir();
    let mut metas = load_index();
    if let Some(m) = metas.iter().find(|m| m.hash == hash) {
        let _ = std::fs::remove_file(data_path(&dir, &m.content_type, hash));
        let _ = std::fs::remove_file(pinned_thumb_path(hash));
    }
    metas.retain(|m| m.hash != hash);
    for (i, m) in metas.iter_mut().enumerate() {
        m.order = i;
    }
    save_index(&metas);
}

/// Drag-and-drop reorder: move `source` to `target`'s position.
pub fn move_pinned(source: &str, target: &str) {
    let mut metas = load_index();
    let (Some(from), Some(to)) = (
        metas.iter().position(|m| m.hash == source),
        metas.iter().position(|m| m.hash == target),
    ) else {
        return;
    };
    let m = metas.remove(from);
    metas.insert(to, m);
    for (i, m) in metas.iter_mut().enumerate() {
        m.order = i;
    }
    save_index(&metas);
}

/// Hash of a cliphist entry if its content is pinned (decodes it once).
pub fn get_pinned_hash(entry: &ClipEntry) -> Option<String> {
    let hash = stable_hash(&decode_entry(entry).ok()?);
    load_index().iter().any(|m| m.hash == hash).then_some(hash)
}

pub fn select_pinned(entry: &PinnedClipEntry, notify: bool) -> bool {
    let Ok(data) = std::fs::read(&entry.data_path) else {
        log(APP_NAME, &format!("pinned data missing: {}", entry.data_path.display()));
        return false;
    };
    let mime = if entry.meta.is_image() { "image/png" } else { "text/plain" };
    if !wl_copy(&data, mime) {
        return false;
    }
    if notify {
        let msg = if entry.meta.is_image() {
            "Pinned image copied".to_string()
        } else {
            format!("Copied: {}", char_truncate(&entry.meta.preview, 50))
        };
        notify_send(&msg);
    }
    true
}

// ---------------------------------------------------------------------------
// cliphist entries
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ClipEntry {
    pub raw_line: String,
    pub id: String,
    pub preview: String,
    pub is_image: bool,
    /// Where the thumbnail lives (or will, once the background worker makes it).
    /// Keyed by the cliphist line, not the id: ids restart after `cliphist wipe`.
    pub thumb_path: Option<PathBuf>,
}

pub fn thumb_cache() -> PathBuf {
    let d = common::paths::cache_dir(APP_NAME).join("thumbs");
    std::fs::create_dir_all(&d).ok();
    d
}

/// Fast: one `cliphist list`, no decoding. Thumbnails come from the worker.
pub fn fetch_entries(max_items: usize) -> Vec<ClipEntry> {
    let output = match Command::new("cliphist").arg("list").output() {
        Ok(o) => o,
        Err(e) => {
            log(APP_NAME, &format!("cliphist list failed: {e}"));
            return Vec::new();
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let cache = thumb_cache();
    let limit = if max_items > 0 { max_items } else { usize::MAX };

    stdout
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let (id, preview) = match line.split_once('\t') {
                Some((i, p)) => (i.trim().to_string(), p.to_string()),
                None => (line.to_string(), line.to_string()),
            };
            if is_browser_html_junk(&preview) {
                return None;
            }
            let is_image = preview.contains("[[ binary data");
            let thumb_path =
                is_image.then(|| cache.join(format!("{}.png", stable_hash(line.as_bytes()))));
            Some(ClipEntry {
                raw_line: line.to_string(),
                id,
                preview,
                is_image,
                thumb_path,
            })
        })
        .take(limit)
        .collect()
}

/// HTML junk that browsers put on the clipboard alongside a copied image
fn is_browser_html_junk(preview: &str) -> bool {
    let p = preview.trim();
    (p.starts_with("<meta ") && p.contains("<img "))
        || (p.starts_with("<meta http-equiv=\"content-type\"") && p.len() < 200)
}

pub fn select_entry(entry: &ClipEntry, notify: bool) -> bool {
    let data = match decode_entry(entry) {
        Ok(d) => d,
        Err(e) => {
            log(APP_NAME, &format!("select failed: {e}"));
            return false;
        }
    };
    let mime = if entry.is_image { "image/png" } else { "text/plain" };
    if !wl_copy(&data, mime) {
        return false;
    }
    if notify {
        let msg = if entry.is_image {
            "Image copied".to_string()
        } else {
            format!("Copied: {}", char_truncate(&entry.preview, 50))
        };
        notify_send(&msg);
    }
    true
}

pub fn delete_entry(entry: &ClipEntry) {
    match Command::new("cliphist").arg("delete").stdin(Stdio::piped()).spawn() {
        Ok(mut c) => {
            if let Some(mut si) = c.stdin.take() {
                let _ = si.write_all(entry.raw_line.as_bytes());
            }
            let _ = c.wait();
        }
        Err(e) => log(APP_NAME, &format!("cliphist delete failed: {e}")),
    }
    if let Some(ref p) = entry.thumb_path {
        let _ = std::fs::remove_file(p);
    }
}

fn decode_entry(entry: &ClipEntry) -> Result<Vec<u8>, String> {
    pipe_through(Command::new("cliphist").arg("decode"), entry.raw_line.as_bytes())
}

/// Run `cmd`, feed `input` on stdin, return stdout.
fn pipe_through(cmd: &mut Command, input: &[u8]) -> Result<Vec<u8>, String> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(input);
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() && !out.stdout.is_empty() {
        Ok(out.stdout)
    } else {
        Err("command failed".to_string())
    }
}

/// Run `cmd` with `input` on stdin, discard output, report success.
fn feed(cmd: &mut Command, input: &[u8]) -> bool {
    let Ok(mut child) = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(input);
    }
    child.wait().is_ok_and(|s| s.success())
}

fn wl_copy(data: &[u8], mime: &str) -> bool {
    // wl-copy forks its own server and exits once stdin is read
    match Command::new("wl-copy").args(["--type", mime]).stdin(Stdio::piped()).spawn() {
        Ok(mut c) => {
            if let Some(mut si) = c.stdin.take() {
                let _ = si.write_all(data);
            }
            let _ = c.wait();
            true
        }
        Err(e) => {
            log(APP_NAME, &format!("wl-copy failed: {e}"));
            false
        }
    }
}

fn notify_send(msg: &str) {
    let mut cmd = Command::new("notify-send");
    cmd.args(["-t", "2000", APP_NAME, msg]);
    spawn_reaped(cmd);
}

// ---------------------------------------------------------------------------
// Thumbnails (background worker)
// ---------------------------------------------------------------------------

pub enum ThumbSource {
    /// cliphist list line, decoded with `cliphist decode`
    Cliphist(String),
    /// pinned image file
    File(PathBuf),
}

pub struct ThumbJob {
    pub source: ThumbSource,
    pub out: PathBuf,
}

/// Thumbnails that still need generating. Cheap: only stat()s.
pub fn thumb_jobs(entries: &[ClipEntry], pinned: &[PinnedClipEntry]) -> Vec<ThumbJob> {
    let mut jobs = Vec::new();
    for p in pinned {
        if let Some(out) = &p.thumb_path {
            if !out.exists() {
                jobs.push(ThumbJob { source: ThumbSource::File(p.data_path.clone()), out: out.clone() });
            }
        }
    }
    for e in entries {
        if let Some(out) = &e.thumb_path {
            if !out.exists() {
                jobs.push(ThumbJob { source: ThumbSource::Cliphist(e.raw_line.clone()), out: out.clone() });
            }
        }
    }
    jobs
}

/// Every thumbnail still in use; anything else in the cache gets pruned.
pub fn thumb_keep_set(entries: &[ClipEntry], pinned: &[PinnedClipEntry]) -> HashSet<PathBuf> {
    entries
        .iter()
        .filter_map(|e| e.thumb_path.clone())
        .chain(pinned.iter().filter_map(|p| p.thumb_path.clone()))
        .collect()
}

/// Runs off the UI thread. Returns how many thumbnails were made.
pub fn generate_thumbs(jobs: &[ThumbJob]) -> usize {
    let mut made = 0;
    for job in jobs {
        let data = match &job.source {
            ThumbSource::Cliphist(line) => {
                match pipe_through(Command::new("cliphist").arg("decode"), line.as_bytes()) {
                    Ok(d) => d,
                    Err(_) => continue,
                }
            }
            ThumbSource::File(p) => match std::fs::read(p) {
                Ok(d) => d,
                Err(_) => continue,
            },
        };
        // write to a temp name, then rename: the UI never sees a half-written file
        let tmp = job.out.with_extension("tmp.png");
        let resize = format!("{s}x{s}^", s = THUMB_SIZE * 2);
        let dest = format!("png:{}", tmp.display());
        let ok = feed(Command::new("magick").args(["-", "-resize", &resize, &dest]), &data);
        if ok && tmp.exists() && std::fs::rename(&tmp, &job.out).is_ok() {
            made += 1;
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    made
}

/// Delete cached thumbnails no longer referenced (cleared/deleted entries,
/// unpinned items, and the old id-keyed cache files).
pub fn prune_thumb_cache(keep: &HashSet<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(thumb_cache()) else { return };
    for f in rd.flatten() {
        let p = f.path();
        if !keep.contains(&p) {
            let _ = std::fs::remove_file(&p);
        }
    }
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

pub fn content_type(e: &ClipEntry) -> &'static str {
    if e.is_image {
        return "IMAGE";
    }
    let p = e.preview.trim();
    if p.starts_with("http://") || p.starts_with("https://") {
        "URL"
    } else {
        "TEXT"
    }
}

pub fn parse_image_meta(preview: &str) -> Option<String> {
    let inner = preview
        .trim_start_matches("[[ binary data")
        .trim_end_matches("]]")
        .trim();
    let mut dims = None;
    let mut fmt = None;
    for p in inner.split_whitespace() {
        if p.contains('x') && p.chars().all(|c| c.is_ascii_digit() || c == 'x') {
            dims = Some(p.to_string());
        }
        if ["png", "jpg", "jpeg", "gif", "bmp", "webp"].contains(&p.to_lowercase().as_str()) {
            fmt = Some(p.to_uppercase());
        }
    }
    match (dims, fmt) {
        (Some(d), Some(f)) => Some(format!("{d} -- {f}")),
        (Some(d), None) => Some(d),
        (None, Some(f)) => Some(f),
        _ => None,
    }
}
