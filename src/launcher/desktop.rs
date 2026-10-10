use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::config::APP_NAME;
use common::logging::log;

thread_local! {
    /// desktop id -> launch count (persisted)
    pub static FREQUENCY: RefCell<HashMap<String, u32>> = RefCell::new(load_frequency());
    /// newest mtime of the application dirs at the last load
    static APPS_STAMP: RefCell<Option<SystemTime>> = const { RefCell::new(None) };
}

#[derive(Clone, Debug)]
pub struct DesktopEntry {
    /// Desktop file id (e.g. org.gnome.Nautilus.desktop): unique key
    pub id: String,
    pub name: String,
    pub exec: String,
    pub icon: String,
    pub description: String,
    pub terminal: bool,
}

pub fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("applications"));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for dir in data_dirs.split(':').filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(dir).join("applications"));
    }
    dirs
}

/// Strip field codes per the Desktop Entry spec (%f %U ... removed, %% -> %).
fn clean_exec(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.next() {
                Some('%') => out.push('%'),
                Some(_) | None => {}
            }
        } else {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse_desktop_file(path: &Path, id: String) -> Option<DesktopEntry> {
    let content = std::fs::read_to_string(path).ok()?;
    let (mut name, mut exec, mut icon, mut description) =
        (String::new(), String::new(), String::new(), String::new());
    let (mut terminal, mut skip, mut in_entry) = (false, false, false);

    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_entry = t == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((k, v)) = t.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "Name" => name = v.to_string(),
            "Exec" => exec = v.to_string(),
            "Icon" => icon = v.to_string(),
            "Comment" if description.is_empty() => description = v.to_string(),
            "GenericName" if description.is_empty() => description = v.to_string(),
            "Terminal" => terminal = v.eq_ignore_ascii_case("true"),
            "NoDisplay" | "Hidden" if v.eq_ignore_ascii_case("true") => skip = true,
            "Type" if v != "Application" => skip = true,
            _ => {}
        }
    }
    if name.is_empty() || exec.is_empty() || skip {
        return None;
    }
    Some(DesktopEntry {
        id,
        name,
        exec: clean_exec(&exec),
        icon,
        description,
        terminal,
    })
}

/// (path, desktop id) for every .desktop file under `dir`. The id is the path
/// relative to the applications dir with '/' -> '-' (spec).
fn desktop_files(dir: &Path, base: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            desktop_files(&p, base, out);
        } else if p.extension().is_some_and(|x| x == "desktop") {
            if let Ok(rel) = p.strip_prefix(base) {
                let id = rel.to_string_lossy().replace('/', "-");
                out.push((p, id));
            }
        }
    }
}

/// Newest mtime among the application dirs (they change when apps are
/// installed/removed). Cheap: a handful of stat() calls.
fn apps_stamp() -> Option<SystemTime> {
    xdg_data_dirs()
        .iter()
        .filter_map(|d| std::fs::metadata(d).and_then(|m| m.modified()).ok())
        .max()
}

/// Reload only when an application dir changed since the last load.
pub fn needs_reload() -> bool {
    let now = apps_stamp();
    APPS_STAMP.with(|s| *s.borrow() != now)
}

pub fn load_entries() -> Vec<DesktopEntry> {
    APPS_STAMP.with(|s| *s.borrow_mut() = apps_stamp());
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    // Earlier dirs win (user overrides in ~/.local/share/applications)
    for dir in xdg_data_dirs() {
        let mut files = Vec::new();
        desktop_files(&dir, &dir, &mut files);
        for (path, id) in files {
            if !seen.insert(id.clone()) {
                continue; // shadowed by a higher-priority dir, even if hidden there
            }
            if let Some(e) = parse_desktop_file(&path, id) {
                entries.push(e);
            }
        }
    }
    entries.sort_by_cached_key(|e| e.name.to_lowercase());
    log(APP_NAME, &format!("loaded {} desktop entries", entries.len()));
    entries
}

// ---------------------------------------------------------------------------
// Launching
// ---------------------------------------------------------------------------

/// Start a command fully detached: `setsid -f` forks it into its own session
/// and returns immediately, so it never becomes our (zombie) child and isn't
/// tied to the launcher's lifetime.
pub fn spawn_detached(program: &str, args: &[&str]) {
    let r = Command::new("setsid")
        .arg("-f")
        .arg(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if let Err(e) = r {
        log(APP_NAME, &format!("spawn {program} failed: {e}"));
    }
}

pub fn launch_app(entry: &DesktopEntry, terminal: &str) {
    FREQUENCY.with(|f| {
        *f.borrow_mut().entry(entry.id.clone()).or_insert(0) += 1;
        save_frequency(&f.borrow());
    });
    log(APP_NAME, &format!("launching: {} ({})", entry.name, entry.exec));
    let exec = entry.exec.as_str();
    if entry.terminal {
        spawn_detached(terminal, &["-e", "sh", "-c", exec]);
    } else {
        spawn_detached("sh", &["-c", exec]);
    }
}

// ---------------------------------------------------------------------------
// Frequency persistence: ~/.local/state/launch-gui/apps.freq ("count\tid")
// ---------------------------------------------------------------------------

fn frequency_path() -> PathBuf {
    common::logging::log_dir(APP_NAME).join("apps.freq")
}

fn load_frequency() -> HashMap<String, u32> {
    std::fs::read_to_string(frequency_path())
        .map(|s| {
            s.lines()
                .filter_map(|l| {
                    let (n, id) = l.split_once('\t')?;
                    Some((id.to_string(), n.parse().ok()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save_frequency(freq: &HashMap<String, u32>) {
    let path = frequency_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let body: String = freq.iter().map(|(id, n)| format!("{n}\t{id}\n")).collect();
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}
