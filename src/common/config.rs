use crate::keys::{default_keybinds, parse_action, parse_key_combos, Action, KeyCombo};
use crate::logging::log;
use crate::paths::{config_dir, shellexpand};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    Center,
    Top,
    TopLeft,
    TopRight,
    Bottom,
    BottomLeft,
    BottomRight,
    Cursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Easing {
    Linear,
    EaseIn,
    #[default]
    EaseOut,
    EaseInOut,
    Bounce,
}

impl Easing {
    /// Apply easing function to t (0.0 to 1.0)
    pub fn apply(&self, t: f64) -> f64 {
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t * t,
            Easing::EaseOut => 1.0 - (1.0 - t).powi(3),
            Easing::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Easing::Bounce => {
                if t < 1.0 {
                    // Overshoot then settle
                    let t2 = t * 1.2;
                    if t2 <= 1.0 {
                        1.0 - (1.0 - t2).powi(2)
                    } else {
                        let over = t2 - 1.0;
                        1.0 + 0.1 * (1.0 - over * 5.0)
                    }
                } else {
                    1.0
                }
            }
        }
    }
}

pub fn parse_easing(s: &str) -> Easing {
    match s.to_lowercase().replace('-', "_").as_str() {
        "linear" => Easing::Linear,
        "ease_in" | "easein" => Easing::EaseIn,
        "ease_out" | "easeout" => Easing::EaseOut,
        "ease_in_out" | "easeinout" => Easing::EaseInOut,
        "bounce" => Easing::Bounce,
        _ => Easing::EaseOut,
    }
}

#[derive(Clone, Debug)]
pub struct ConfigBase {
    pub width: i32,
    pub height: i32,
    pub anchor: Anchor,
    pub margin_top: i32,
    pub margin_bottom: i32,
    pub margin_left: i32,
    pub margin_right: i32,
    pub theme: String,
    pub keybinds: HashMap<Action, Vec<KeyCombo>>,
}

impl ConfigBase {
    pub fn new(app_name: &str, width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            anchor: Anchor::Center,
            margin_top: 0,
            margin_bottom: 0,
            margin_left: 0,
            margin_right: 0,
            theme: config_dir(app_name)
                .join("style.css")
                .to_string_lossy()
                .to_string(),
            keybinds: default_keybinds(),
        }
    }

    pub fn parse_section(&mut self, app_name: &str, section: &str, key: &str, val: &str) {
        match section {
            "window" => match key {
                "width" => self.width = val.parse().unwrap_or(self.width),
                "height" => self.height = val.parse().unwrap_or(self.height),
                "anchor" => self.anchor = parse_anchor(val),
                "margin_top" => self.margin_top = val.parse().unwrap_or(0),
                "margin_bottom" => self.margin_bottom = val.parse().unwrap_or(0),
                "margin_left" => self.margin_left = val.parse().unwrap_or(0),
                "margin_right" => self.margin_right = val.parse().unwrap_or(0),
                _ => log(app_name, &format!("unknown window key: {}", key)),
            },
            "style" => {
                if key == "theme" {
                    self.theme = shellexpand(val);
                }
            }
            "keybinds" => {
                if let Some(action) = parse_action(key) {
                    let combos = parse_key_combos(val);
                    if !combos.is_empty() {
                        self.keybinds.insert(action, combos);
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn parse_anchor(s: &str) -> Anchor {
    match s.to_lowercase().replace('-', "_").as_str() {
        "center" => Anchor::Center,
        "top" => Anchor::Top,
        "top_left" | "topleft" => Anchor::TopLeft,
        "top_right" | "topright" => Anchor::TopRight,
        "bottom" => Anchor::Bottom,
        "bottom_left" | "bottomleft" => Anchor::BottomLeft,
        "bottom_right" | "bottomright" => Anchor::BottomRight,
        "cursor" => Anchor::Cursor,
        _ => Anchor::Center,
    }
}

pub fn parse_bool(s: &str, default: bool) -> bool {
    match s.to_lowercase().as_str() {
        "true" | "yes" | "1" | "on" => true,
        "false" | "no" | "0" | "off" => false,
        _ => default,
    }
}

pub fn parse_config_file(content: &str) -> Vec<(String, String, String)> {
    let mut results = Vec::new();
    let mut section = String::new();
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            section = t[1..t.len() - 1].trim().to_lowercase();
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            results.push((
                section.clone(),
                k.trim().to_lowercase(),
                v.trim().to_string(),
            ));
        }
    }
    results
}
