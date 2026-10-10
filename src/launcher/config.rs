use common::{
    config::{parse_bool, parse_config_file, parse_easing, Easing},
    logging::log,
    paths::{config_dir, shellexpand},
    ConfigBase,
};

pub const APP_NAME: &str = "launch-gui";

pub fn default_config() -> &'static str {
    include_str!("config.default")
}

pub fn default_css() -> &'static str {
    include_str!("style.css")
}

/// Directory names skipped anywhere in the tree (bare names), on top of
/// .gitignore and hidden files. Paths (`~/x`, `/x`) in `exclude` are prefixes.
pub const DEFAULT_EXCLUDES: &[&str] = &[
    "node_modules", "target", "__pycache__", ".venv", "venv", "dist", ".next",
    "Cache", "Code Cache", "GPUCache", "CachedData", "Service Worker", "IndexedDB",
    "blob_storage", "Crashpad", "/proc", "/sys", "/dev", "/run", "/tmp", "/var/tmp",
    "/var/cache", "/var/lib", "/var/log", "/boot", "/lost+found",
];

#[derive(Clone, Debug)]
pub struct FilesConfig {
    pub enabled: bool,
    /// Prefix that switches the launcher to file search, e.g. "f'"
    pub shortcut: String,
    pub roots: Vec<String>,
    pub exclude: Vec<String>,
    pub max_results: usize,
    /// Rescan when the index is older than this (on open, in the background)
    pub rescan_secs: u64,
    /// Text/code/config/log files open as `<terminal> -e <editor> <file>`
    pub editor: String,
    /// Super+Enter opens folders with this
    pub file_manager: String,
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "f'".into(),
            roots: vec!["~".into(), "~/.config".into()],
            exclude: DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect(),
            max_results: 50,
            rescan_secs: 300,
            editor: "nvim".into(),
            file_manager: "xdg-open".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub base: ConfigBase,
    pub search_height: i32,
    pub animation_duration: u64,
    pub animation_easing: Easing,
    pub terminal: String,
    pub calculator: bool,
    pub vim_mode: bool,
    pub files: FilesConfig,
    pub snippets: snippets::Settings,
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    v.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(v)
        .to_string()
}

/// `~` and `~/x` -> absolute (common::shellexpand only handles `~/x`)
pub fn expand_home(s: &str) -> String {
    if s == "~" {
        std::env::var("HOME").unwrap_or_else(|_| "/".into())
    } else {
        shellexpand(s)
    }
}

/// `a, b, "c d"` -> ["a", "b", "c d"]
fn parse_list(v: &str) -> Vec<String> {
    v.split(',').map(unquote).filter(|s| !s.is_empty()).collect()
}

impl Config {
    pub fn default() -> Self {
        let mut files = FilesConfig::default();
        files.roots = files.roots.iter().map(|r| expand_home(r)).collect();
        Self {
            base: ConfigBase::new(APP_NAME, 580, 400),
            search_height: 70,
            animation_duration: 200,
            animation_easing: Easing::EaseOut,
            terminal: "kitty".to_string(),
            calculator: true,
            vim_mode: false,
            files,
            snippets: snippets::Settings::default(),
        }
    }

    pub fn load() -> Self {
        let path = config_dir(APP_NAME).join("config");
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(c) => Self::parse(&c),
            Err(_) => Self::default(),
        };
        cfg.snippets = snippets::Settings::load();
        cfg
    }

    pub fn parse(content: &str) -> Self {
        let mut cfg = Self::default();
        let mut extra_exclude = Vec::new();
        let mut replace_exclude = None;
        for (section, key, val) in parse_config_file(content) {
            cfg.base.parse_section(APP_NAME, &section, &key, &val);
            match (section.as_str(), key.as_str()) {
                ("window", "search_height") => cfg.search_height = val.parse().unwrap_or(70),
                ("behavior", "terminal") => cfg.terminal = unquote(&val),
                ("behavior", "calculator") => cfg.calculator = parse_bool(&val, true),
                ("behavior", "vim_mode") => cfg.vim_mode = parse_bool(&val, false),
                ("behavior", "animation_duration") => {
                    cfg.animation_duration = val.parse().unwrap_or(200)
                }
                ("behavior", "animation_easing") => cfg.animation_easing = parse_easing(&val),
                ("files", "enabled") => cfg.files.enabled = parse_bool(&val, true),
                ("files", "shortcut") => cfg.files.shortcut = unquote(&val),
                ("files", "roots") => cfg.files.roots = parse_list(&val),
                // `exclude` adds to the defaults; `exclude_only` replaces them
                ("files", "exclude") => extra_exclude.extend(parse_list(&val)),
                ("files", "exclude_only") => replace_exclude = Some(parse_list(&val)),
                ("files", "max_results") => cfg.files.max_results = val.parse().unwrap_or(50),
                ("files", "rescan_secs") => cfg.files.rescan_secs = val.parse().unwrap_or(300),
                ("files", "editor") => cfg.files.editor = unquote(&val),
                ("files", "file_manager") => cfg.files.file_manager = unquote(&val),
                ("files", other) => log(APP_NAME, &format!("unknown [files] key: {other}")),
                _ => {}
            }
        }
        if let Some(only) = replace_exclude {
            cfg.files.exclude = only;
        }
        cfg.files.exclude.extend(extra_exclude);
        cfg.files.exclude = cfg.files.exclude.iter().map(|e| expand_home(e)).collect();
        cfg.files.roots = cfg.files.roots.iter().map(|r| expand_home(r)).collect();
        cfg
    }
}
