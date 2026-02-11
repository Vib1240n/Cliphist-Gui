use std::path::PathBuf;

pub fn config_dir(app_name: &str) -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into())).join(".config")
        })
        .join(app_name)
}

pub fn cache_dir(app_name: &str) -> PathBuf {
    let d = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into())).join(".cache")
        })
        .join(app_name);
    std::fs::create_dir_all(&d).ok();
    d
}

pub fn shellexpand(s: &str) -> String {
    if s.starts_with("~/") {
        if let Ok(h) = std::env::var("HOME") {
            return format!("{}/{}", h, &s[2..]);
        }
    }
    s.to_string()
}
