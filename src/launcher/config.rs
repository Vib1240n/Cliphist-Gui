use common::{
    ConfigBase,
    config::{parse_bool, parse_config_file},
    logging::log,
    paths::config_dir,
};

pub const APP_NAME: &str = "launch-gui";

pub fn default_config() -> &'static str { include_str!("config.default") }
pub fn default_css() -> &'static str { include_str!("style.css") }

#[derive(Clone, Debug)]
pub struct Config {
    pub base: ConfigBase,
    pub terminal: String,
    pub calculator: bool,
    pub vim_mode: bool,
}

impl Config {
    pub fn default() -> Self {
        Self {
            base: ConfigBase::new(APP_NAME, 580, 400),
            terminal: "kitty".to_string(),
            calculator: true,
            vim_mode: false,
        }
    }

    pub fn load() -> Self {
        let path = config_dir(APP_NAME).join("config");
        if !path.exists() { return Self::default(); }
        
        match std::fs::read_to_string(&path) {
            Ok(c) => {
                log(APP_NAME, &format!("loaded config from {}", path.display()));
                Self::parse(&c)
            }
            Err(e) => {
                log(APP_NAME, &format!("config read error: {}", e));
                Self::default()
            }
        }
    }

    pub fn parse(content: &str) -> Self {
        let mut cfg = Self::default();
        for (section, key, val) in parse_config_file(content) {
            cfg.base.parse_section(APP_NAME, &section, &key, &val);
            if section == "behavior" {
                match key.as_str() {
                    "terminal" => cfg.terminal = val,
                    "calculator" => cfg.calculator = parse_bool(&val, true),
                    "vim_mode" => cfg.vim_mode = parse_bool(&val, false),
                    _ => {}
                }
            }
        }
        cfg
    }
}

