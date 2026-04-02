use common::{
    config::{parse_bool, parse_config_file},
    logging::log,
    paths::{config_dir, shellexpand},
    ConfigBase,
};

pub const APP_NAME: &str = "cliphist-gui";

pub fn default_config() -> &'static str {
    include_str!("config.default")
}
pub fn default_css() -> &'static str {
    include_str!("style.css")
}

pub const DEFAULT_PIN_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 123.14 123.54"><path d="M121.59,36.81,86.3,1.52c-3-3-7.77.09-9.2,2.74-.24.45.19.86-.2,3.91a46.16,46.16,0,0,1-2.72,11.32l-15.7,15.7c-6.26,6.27-15.22,3.48-22.87-.32-1.61-.8-3.68-2.57-5.47-.78l-6.65,6.65a2.5,2.5,0,0,0,0,3.53l55.79,55.78a2.5,2.5,0,0,0,3.53,0l6.64-6.65c1.77-1.77-.49-4.06-1.41-6-3.4-7-6.45-16.42-.78-22.09L103.65,49A84.08,84.08,0,0,1,115,46.38c3.09-.49,3.47-.1,3.91-.39,2.7-1.75,5.7-6.16,2.68-9.18ZM53.86,82.39,41.15,69.69.38,121.25l1.92,1.91L53.86,82.39Z"/></svg>"#;

#[derive(Clone, Debug)]
pub struct Config {
    pub base: ConfigBase,
    pub max_items: usize,
    pub close_on_select: bool,
    pub notify_on_copy: bool,
    pub vim_mode: bool,
    pub max_pinned: usize,
    pub pin_icon: String,
}

impl Config {
    pub fn default() -> Self {
        Self {
            base: ConfigBase::new(APP_NAME, 580, 520),
            max_items: 0,
            close_on_select: true,
            notify_on_copy: false,
            vim_mode: false,
            max_pinned: 20,
            pin_icon: "default".to_string(),
        }
    }

    pub fn load() -> Self {
        let path = config_dir(APP_NAME).join("config");
        if !path.exists() {
            return Self::default();
        }

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
                    "max_items" => cfg.max_items = val.parse().unwrap_or(0),
                    "close_on_select" => cfg.close_on_select = parse_bool(&val, true),
                    "notify_on_copy" => cfg.notify_on_copy = parse_bool(&val, false),
                    "vim_mode" => cfg.vim_mode = parse_bool(&val, false),
                    "max_pinned" => cfg.max_pinned = val.parse().unwrap_or(20),
                    "pin_icon" => {
                        if val != "default" {
                            cfg.pin_icon = shellexpand(&val);
                        }
                    }
                    _ => {}
                }
            }
        }
        cfg
    }

    pub fn get_pin_icon_svg(&self) -> String {
        if self.pin_icon == "default" {
            DEFAULT_PIN_ICON.to_string()
        } else {
            std::fs::read_to_string(&self.pin_icon).unwrap_or_else(|_| DEFAULT_PIN_ICON.to_string())
        }
    }
}
