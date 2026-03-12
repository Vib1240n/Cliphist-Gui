use crate::config::APP_NAME;
use common::css::char_truncate;
use common::logging::log;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

const THUMB_SIZE: u32 = 64;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct ClipEntry {
    pub raw_line: String,
    pub id: String,
    pub preview: String,
    pub is_image: bool,
    pub thumb_path: Option<PathBuf>,
}

/// Thumbnail generation result
#[derive(Clone, Debug)]
pub struct ThumbnailResult {
    pub id: String,
    pub path: Option<PathBuf>,
}

pub fn thumb_cache() -> PathBuf {
    let d = common::paths::cache_dir(APP_NAME).join("thumbs");
    std::fs::create_dir_all(&d).ok();
    d
}

/// Fast synchronous fetch - NO thumbnail generation, just parse cliphist output
/// Returns entries immediately with thumb_path set only if already cached
pub fn fetch_entries_fast(max_items: usize) -> Vec<ClipEntry> {
    let output = match Command::new("cliphist")
        .arg("list")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
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

    iter.map(|line| {
        let raw_line = line.to_string();
        let (id, preview) = match line.split_once('\t') {
            Some((i, p)) => (i.trim().to_string(), p.to_string()),
            None => (line.to_string(), line.to_string()),
        };
        let is_image = preview.contains("[[ binary data");

        // Only check if thumbnail exists - don't generate
        let thumb_path = if is_image {
            let path = cache.join(format!("{}.png", id));
            if path.exists() {
                Some(path)
            } else {
                None
            }
        } else {
            None
        };

        ClipEntry {
            raw_line,
            id,
            preview,
            is_image,
            thumb_path,
        }
    })
    .collect()
}

/// Synchronous thumbnail generation - returns true on success
fn generate_thumbnail_sync(raw_line: &str, out_path: &Path) -> bool {
    // Decode from cliphist
    let mut child = match Command::new("cliphist")
        .arg("decode")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(raw_line.as_bytes());
        drop(si);
    }

    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return false,
    };

    if !out.status.success() || out.stdout.is_empty() {
        return false;
    }

    // Resize with imagemagick
    let mut m = match Command::new("magick")
        .args([
            "png:-",
            "-resize",
            &format!("{}x{}^", THUMB_SIZE * 2, THUMB_SIZE * 2),
            &format!("png:{}", out_path.display()),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    if let Some(mut si) = m.stdin.take() {
        let _ = si.write_all(&out.stdout);
        drop(si);
    }

    m.wait().map(|s| s.success()).unwrap_or(false)
}

/// Generate thumbnails for entries in background thread
/// Returns a shared results vector that gets populated as thumbnails complete
pub fn generate_thumbnails_background(entries: Vec<ClipEntry>) -> Arc<Mutex<Vec<ThumbnailResult>>> {
    let results = Arc::new(Mutex::new(Vec::new()));
    let results_clone = results.clone();

    thread::spawn(move || {
        let cache = thumb_cache();

        // Collect entries that need thumbnails
        let needs_thumb: Vec<_> = entries
            .iter()
            .filter(|e| e.is_image && e.thumb_path.is_none())
            .collect();

        if needs_thumb.is_empty() {
            return;
        }

        log(
            APP_NAME,
            &format!("generating {} thumbnails in background", needs_thumb.len()),
        );

        for entry in needs_thumb {
            let path = cache.join(format!("{}.png", entry.id));

            let result = if generate_thumbnail_sync(&entry.raw_line, &path) {
                ThumbnailResult {
                    id: entry.id.clone(),
                    path: Some(path),
                }
            } else {
                ThumbnailResult {
                    id: entry.id.clone(),
                    path: None,
                }
            };

            if let Ok(mut r) = results_clone.lock() {
                r.push(result);
            }
        }
    });

    results
}

/// Poll for completed thumbnails - returns new results since last poll
pub fn poll_thumbnail_results(
    results: &Arc<Mutex<Vec<ThumbnailResult>>>,
    last_count: usize,
) -> Vec<ThumbnailResult> {
    if let Ok(r) = results.lock() {
        if r.len() > last_count {
            return r[last_count..].to_vec();
        }
    }
    Vec::new()
}

pub fn select_entry(entry: &ClipEntry, notify: bool) {
    let mut dec = Command::new("cliphist")
        .arg("decode")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
                .stdin(Stdio::piped())
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
        .stdin(Stdio::piped())
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

/// Update thumbnail path for an entry by ID
pub fn update_entry_thumbnail(entries: &mut [ClipEntry], id: &str, path: PathBuf) {
    if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
        entry.thumb_path = Some(path);
    }
}
