use crate::config::APP_NAME;
use common::css::char_truncate;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

const THUMB_SIZE: u32 = 64;

#[derive(Clone, Debug)]
pub struct PinnedMeta {
    pub hash: String,
    pub content_type: String,
    pub preview: String,
    pub pinned_at: u64,
    pub order: usize,
}

#[derive(Clone, Debug)]
pub struct PinnedClipEntry {
    pub meta: PinnedMeta,
    pub thumb_path: Option<PathBuf>,
    pub data_path: PathBuf,
}

pub fn pinned_dir() -> PathBuf {
    let d = common::paths::config_dir(APP_NAME).join("pinned");
    std::fs::create_dir_all(&d).ok();
    std::fs::create_dir_all(d.join("text")).ok();
    std::fs::create_dir_all(d.join("media")).ok();
    d
}

fn index_path() -> PathBuf {
    pinned_dir().join("index.json")
}

fn compute_hash(data: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_index() -> Vec<PinnedMeta> {
    let path = index_path();
    if !path.exists() {
        return Vec::new();
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    parse_index_json(&content)
}

fn parse_index_json(content: &str) -> Vec<PinnedMeta> {
    let mut entries = Vec::new();
    let content = content.trim();
    if !content.starts_with('[') {
        return entries;
    }
    let inner = &content[1..content.len().saturating_sub(1)];
    for obj in inner.split("},") {
        let obj = obj.trim().trim_start_matches('{').trim_end_matches('}');
        let mut hash = String::new();
        let mut content_type = String::new();
        let mut preview = String::new();
        let mut pinned_at: u64 = 0;
        let mut order: usize = 0;
        for part in obj.split(',') {
            if let Some((k, v)) = part.split_once(':') {
                let key = k.trim().trim_matches('"');
                let val = v.trim().trim_matches('"');
                match key {
                    "hash" => hash = val.to_string(),
                    "content_type" => content_type = val.to_string(),
                    "preview" => preview = val.replace("\\n", "\n").replace("\\\"", "\""),
                    "pinned_at" => pinned_at = val.parse().unwrap_or(0),
                    "order" => order = val.parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        if !hash.is_empty() {
            entries.push(PinnedMeta {
                hash,
                content_type,
                preview,
                pinned_at,
                order,
            });
        }
    }
    entries.sort_by_key(|e| e.order);
    entries
}

fn save_index(entries: &[PinnedMeta]) {
    let mut json = String::from("[\n");
    for (i, e) in entries.iter().enumerate() {
        let preview_escaped = e
            .preview
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n");
        json.push_str(&format!(
            "  {{\"hash\":\"{}\",\"content_type\":\"{}\",\"preview\":\"{}\",\"pinned_at\":{},\"order\":{}}}",
            e.hash, e.content_type, preview_escaped, e.pinned_at, e.order
        ));
        if i < entries.len() - 1 {
            json.push(',');
        }
        json.push('\n');
    }
    json.push(']');
    let _ = std::fs::write(index_path(), json);
}

pub fn load_pinned() -> Vec<PinnedClipEntry> {
    let metas = parse_index();
    let dir = pinned_dir();
    let cache = thumb_cache();

    metas
        .into_iter()
        .filter_map(|meta| {
            let data_path = if meta.content_type == "image" {
                dir.join("media").join(format!("{}.png", meta.hash))
            } else {
                dir.join("text").join(format!("{}.txt", meta.hash))
            };
            if !data_path.exists() {
                return None;
            }
            let thumb_path = if meta.content_type == "image" {
                let tp = cache.join(format!("pinned_{}.png", meta.hash));
                if !tp.exists() {
                    generate_pinned_thumbnail(&data_path, &tp);
                }
                if tp.exists() {
                    Some(tp)
                } else {
                    None
                }
            } else {
                None
            };
            Some(PinnedClipEntry {
                meta,
                thumb_path,
                data_path,
            })
        })
        .collect()
}

fn generate_pinned_thumbnail(src: &Path, out_path: &Path) {
    let data = match std::fs::read(src) {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut m = match Command::new("magick")
        .args([
            "png:-",
            "-resize",
            &format!("{}x{}^", THUMB_SIZE * 2, THUMB_SIZE * 2),
            &format!("png:{}", out_path.display()),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    if let Some(mut si) = m.stdin.take() {
        let _ = si.write_all(&data);
        drop(si);
    }
    let _ = m.wait();
}

pub fn pin_entry(entry: &ClipEntry, max_pinned: usize) -> Result<(), String> {
    let mut metas = parse_index();

    if metas.len() >= max_pinned {
        return Err(format!("Maximum {} pinned items reached", max_pinned));
    }

    let data = decode_entry(entry)?;
    let hash = compute_hash(&data);

    if metas.iter().any(|m| m.hash == hash) {
        return Err("Already pinned".to_string());
    }

    let dir = pinned_dir();
    let (content_type, data_path) = if entry.is_image {
        let p = dir.join("media").join(format!("{}.png", hash));
        ("image".to_string(), p)
    } else {
        let p = dir.join("text").join(format!("{}.txt", hash));
        ("text".to_string(), p)
    };

    std::fs::write(&data_path, &data).map_err(|e| e.to_string())?;

    let order = metas.len();
    metas.push(PinnedMeta {
        hash,
        content_type,
        preview: entry.preview.clone(),
        pinned_at: now_timestamp(),
        order,
    });

    save_index(&metas);
    Ok(())
}

pub fn unpin_entry(hash: &str) {
    let mut metas = parse_index();
    let dir = pinned_dir();

    if let Some(meta) = metas.iter().find(|m| m.hash == hash) {
        let data_path = if meta.content_type == "image" {
            dir.join("media").join(format!("{}.png", hash))
        } else {
            dir.join("text").join(format!("{}.txt", hash))
        };
        let _ = std::fs::remove_file(data_path);

        let thumb = thumb_cache().join(format!("pinned_{}.png", hash));
        let _ = std::fs::remove_file(thumb);
    }

    metas.retain(|m| m.hash != hash);
    for (i, m) in metas.iter_mut().enumerate() {
        m.order = i;
    }
    save_index(&metas);
}

pub fn reorder_pinned(hashes: &[String]) {
    let metas = parse_index();
    let mut new_metas = Vec::new();

    for (i, hash) in hashes.iter().enumerate() {
        if let Some(mut m) = metas.iter().find(|m| &m.hash == hash).cloned() {
            m.order = i;
            new_metas.push(m);
        }
    }

    save_index(&new_metas);
}

pub fn is_pinned(entry: &ClipEntry) -> bool {
    let data = match decode_entry(entry) {
        Ok(d) => d,
        Err(_) => return false,
    };
    let hash = compute_hash(&data);
    parse_index().iter().any(|m| m.hash == hash)
}

pub fn get_pinned_hash(entry: &ClipEntry) -> Option<String> {
    let data = decode_entry(entry).ok()?;
    let hash = compute_hash(&data);
    if parse_index().iter().any(|m| m.hash == hash) {
        Some(hash)
    } else {
        None
    }
}

fn decode_entry(entry: &ClipEntry) -> Result<Vec<u8>, String> {
    let mut dec = Command::new("cliphist")
        .arg("decode")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;

    if let Some(mut si) = dec.stdin.take() {
        let _ = si.write_all(entry.raw_line.as_bytes());
        drop(si);
    }

    let out = dec.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err("Decode failed".to_string())
    }
}

pub fn select_pinned(entry: &PinnedClipEntry, notify: bool) {
    let data = match std::fs::read(&entry.data_path) {
        Ok(d) => d,
        Err(_) => return,
    };

    let mime = if entry.meta.content_type == "image" {
        "image/png"
    } else {
        "text/plain"
    };

    let mut wl = match Command::new("wl-copy")
        .args(["--type", mime])
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return,
    };

    if let Some(mut si) = wl.stdin.take() {
        let _ = si.write_all(&data);
        drop(si);
    }
    let _ = wl.wait();

    if notify {
        let msg = if entry.meta.content_type == "image" {
            "Pinned image copied".to_string()
        } else {
            format!("Copied: {}", char_truncate(&entry.meta.preview, 50))
        };
        let _ = Command::new("notify-send")
            .args(["-t", "2000", APP_NAME, &msg])
            .spawn();
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct ClipEntry {
    pub raw_line: String,
    pub id: String,
    pub preview: String,
    pub is_image: bool,
    pub thumb_path: Option<PathBuf>,
}

pub fn thumb_cache() -> PathBuf {
    let d = common::paths::cache_dir(APP_NAME).join("thumbs");
    std::fs::create_dir_all(&d).ok();
    d
}

pub fn fetch_entries(max_items: usize) -> Vec<ClipEntry> {
    let output = match Command::new("cliphist").arg("list").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let cache = thumb_cache();

    let iter = stdout.lines().filter(|l| !l.is_empty());
    let iter: Box<dyn Iterator<Item = &str>> = if max_items > 0 {
        Box::new(iter.take(max_items))
    } else {
        Box::new(iter)
    };

    iter.filter_map(|line| {
        let raw_line = line.to_string();
        let (id, preview) = match line.split_once('\t') {
            Some((i, p)) => (i.trim().to_string(), p.to_string()),
            None => (line.to_string(), line.to_string()),
        };

        // Skip HTML meta/img tags from browser image copies
        if is_browser_html_junk(&preview) {
            return None;
        }

        let is_image = preview.contains("[[ binary data");
        let thumb_path = if is_image {
            let path = cache.join(format!("{}.png", id));
            if !path.exists() {
                generate_thumbnail(&raw_line, &path);
            }
            if path.exists() {
                Some(path)
            } else {
                None
            }
        } else {
            None
        };
        Some(ClipEntry {
            raw_line,
            id,
            preview,
            is_image,
            thumb_path,
        })
    })
    .collect()
}

/// Check if content is HTML junk from browser image copy
fn is_browser_html_junk(preview: &str) -> bool {
    let p = preview.trim();
    // HTML meta tag followed by img tag (browser image copy pattern)
    if p.starts_with("<meta ") && p.contains("<img ") {
        return true;
    }
    // Just a meta content-type tag
    if p.starts_with("<meta http-equiv=\"content-type\"") && p.len() < 200 {
        return true;
    }
    false
}

pub fn generate_thumbnail(raw_line: &str, out_path: &Path) {
    let mut child = match Command::new("cliphist")
        .arg("decode")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return,
    };

    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(raw_line.as_bytes());
        drop(si);
    }

    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return,
    };
    if !out.status.success() || out.stdout.is_empty() {
        return;
    }

    let mut m = match Command::new("magick")
        .args([
            "png:-",
            "-resize",
            &format!("{}x{}^", THUMB_SIZE * 2, THUMB_SIZE * 2),
            &format!("png:{}", out_path.display()),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return,
    };

    if let Some(mut si) = m.stdin.take() {
        let _ = si.write_all(&out.stdout);
        drop(si);
    }
    let _ = m.wait();
}

pub fn select_entry(entry: &ClipEntry, notify: bool) {
    let mut dec = Command::new("cliphist")
        .arg("decode")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("cliphist decode failed");

    if let Some(mut si) = dec.stdin.take() {
        let _ = si.write_all(entry.raw_line.as_bytes());
        drop(si);
    }

    if let Ok(out) = dec.wait_with_output() {
        if out.status.success() {
            let mime = if entry.is_image {
                "image/png"
            } else {
                "text/plain"
            };
            let mut wl = Command::new("wl-copy")
                .args(["--type", mime])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .expect("wl-copy failed");
            if let Some(mut si) = wl.stdin.take() {
                let _ = si.write_all(&out.stdout);
                drop(si);
            }
            let _ = wl.wait();

            if notify {
                let msg = if entry.is_image {
                    "Image copied".to_string()
                } else {
                    format!("Copied: {}", char_truncate(&entry.preview, 50))
                };
                let _ = Command::new("notify-send")
                    .args(["-t", "2000", APP_NAME, &msg])
                    .spawn();
            }
        }
    }
}

pub fn delete_entry(entry: &ClipEntry) {
    if let Ok(mut c) = Command::new("cliphist")
        .arg("delete")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut si) = c.stdin.take() {
            let _ = si.write_all(entry.raw_line.as_bytes());
            drop(si);
        }
        let _ = c.wait();
    }
    if let Some(ref p) = entry.thumb_path {
        let _ = std::fs::remove_file(p);
    }
}

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
    let parts: Vec<&str> = inner.split_whitespace().collect();
    let mut dims = None;
    let mut fmt = None;

    for p in &parts {
        if p.contains('x') && p.chars().all(|c| c.is_ascii_digit() || c == 'x') {
            dims = Some(p.to_string());
        }
        if ["png", "jpg", "jpeg", "gif", "bmp", "webp"].contains(&p.to_lowercase().as_str()) {
            fmt = Some(p.to_uppercase());
        }
    }

    match (dims, fmt) {
        (Some(d), Some(f)) => Some(format!("{} -- {}", d, f)),
        (Some(d), None) => Some(d),
        (None, Some(f)) => Some(f),
        _ => None,
    }
}

pub fn get_filtered_entry(entries: &[ClipEntry], query: &str, idx: usize) -> Option<ClipEntry> {
    let q = query.to_lowercase();
    let filtered: Vec<&ClipEntry> = if q.is_empty() {
        entries.iter().collect()
    } else {
        entries
            .iter()
            .filter(|e| e.preview.to_lowercase().contains(&q))
            .collect()
    };
    filtered.get(idx).map(|e| (*e).clone())
}
