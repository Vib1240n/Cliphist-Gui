use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::cell::RefCell;
use std::collections::HashMap;

use common::logging::log;
use crate::config::APP_NAME;

thread_local! {
    pub static FREQUENCY: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Debug)]
pub struct DesktopEntry {
    pub name: String,
    pub exec: String,
    pub icon: String,
    pub description: String,
    pub terminal: bool,
    pub path: PathBuf,
    pub score: i32,
}

pub fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("applications"));
    }
    
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or("/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        dirs.push(PathBuf::from(dir).join("applications"));
    }
    
    dirs
}

pub fn parse_desktop_file(path: &PathBuf) -> Option<DesktopEntry> {
    let content = std::fs::read_to_string(path).ok()?;
    
    let mut name = String::new();
    let mut exec = String::new();
    let mut icon = String::new();
    let mut description = String::new();
    let mut terminal = false;
    let mut no_display = false;
    let mut hidden = false;
    let mut in_desktop_entry = false;

    for line in content.lines() {
        let t = line.trim();
        
        if t.starts_with('[') {
            in_desktop_entry = t == "[Desktop Entry]";
            continue;
        }
        
        if !in_desktop_entry { continue; }
        
        if let Some((k, v)) = t.split_once('=') {
            let key = k.trim();
            let val = v.trim();
            match key {
                "Name" if name.is_empty() => name = val.to_string(),
                "Exec" => exec = val.to_string(),
                "Icon" => icon = val.to_string(),
                "Comment" if description.is_empty() => description = val.to_string(),
                "GenericName" if description.is_empty() => description = val.to_string(),
                "Terminal" => terminal = val.to_lowercase() == "true",
                "NoDisplay" => no_display = val.to_lowercase() == "true",
                "Hidden" => hidden = val.to_lowercase() == "true",
                _ => {}
            }
        }
    }

    if name.is_empty() || exec.is_empty() || no_display || hidden {
        return None;
    }

    let exec_clean = exec
        .replace("%f", "").replace("%F", "")
        .replace("%u", "").replace("%U", "")
        .replace("%c", "").replace("%k", "")
        .replace("%i", "").replace("%d", "").replace("%D", "")
        .trim().to_string();

    Some(DesktopEntry {
        name, exec: exec_clean, icon, description, terminal,
        path: path.clone(), score: 0,
    })
}

fn walkdir(dir: PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                files.extend(walkdir(p));
            } else {
                files.push(p);
            }
        }
    }
    files
}

pub fn load_entries() -> Vec<DesktopEntry> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    for dir in xdg_data_dirs() {
        if !dir.exists() { continue; }
        
        let walker = walkdir(dir.clone());
        for path in walker {
            if path.extension().map(|e| e == "desktop").unwrap_or(false) {
                if let Some(entry) = parse_desktop_file(&path) {
                    if seen.insert(entry.name.clone()) {
                        entries.push(entry);
                    }
                }
            }
        }
    }

    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    log(APP_NAME, &format!("loaded {} desktop entries", entries.len()));
    entries
}

pub fn launch_app(entry: &DesktopEntry, terminal: &str) {
    let exec = &entry.exec;
    
    FREQUENCY.with(|f| {
        let mut freq = f.borrow_mut();
        *freq.entry(entry.name.clone()).or_insert(0) += 1;
    });

    log(APP_NAME, &format!("launching: {} ({})", entry.name, exec));

    if entry.terminal {
        let _ = Command::new(terminal)
            .arg("-e")
            .arg("sh")
            .arg("-c")
            .arg(exec)
            .spawn();
    } else {
        let _ = Command::new("sh")
            .arg("-c")
            .arg(exec)
            .spawn();
    }
}

