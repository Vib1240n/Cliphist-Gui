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
pub fn themes_dir() -> PathBuf {
    // Built-in themes compiled into binary, but also check config
    config_dir("").parent().unwrap_or(&PathBuf::from("/tmp")).join("themes")
}

pub fn builtin_themes() -> Vec<(&'static str, &'static str)> {
    vec![
        ("dracula", include_str!("../themes/dracula.css")),
        ("catppuccin", include_str!("../themes/catppuccin.css")),
        ("onedark", include_str!("../themes/onedark.css")),
        ("monokai", include_str!("../themes/monokai.css")),
        ("material-3", include_str!("../themes/material-3.css")),
        ("material-you", include_str!("../themes/material-you.css")),
    ]
}

pub fn get_theme_css(name: &str) -> Option<String> {
    let transparency = r#"window,
window.background {
  background-color: transparent;
  background: transparent;
}
.background {
  background-color: transparent;
}
headerbar,
.titlebar {
  background-color: transparent;
  background: transparent;
}
"#;
    
    for (n, css) in builtin_themes() {
        if n == name { 
            return Some(format!("{}\n{}", transparency, css)); 
        }
    }
    None
}
