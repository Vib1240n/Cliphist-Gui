use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::path::Path;
use crate::config::APP_NAME;
use common::css::char_truncate;

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

    iter.map(|line| {
        let raw_line = line.to_string();
        let (id, preview) = match line.split_once('\t') {
            Some((i, p)) => (i.trim().to_string(), p.to_string()),
            None => (line.to_string(), line.to_string()),
        };
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
