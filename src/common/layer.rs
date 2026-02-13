use gtk4::ApplicationWindow;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::process::Command;

use crate::config::{Anchor, ConfigBase};

pub fn apply_layer_shell(window: &ApplicationWindow, cfg: &ConfigBase, namespace: &str) {
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_namespace(namespace);

    match cfg.anchor {
        Anchor::Center => {}
        Anchor::Top => {
            window.set_anchor(Edge::Top, true);
        }
        Anchor::TopLeft => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
        }
        Anchor::TopRight => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
        }
        Anchor::Bottom => {
            window.set_anchor(Edge::Bottom, true);
        }
        Anchor::BottomLeft => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
        }
        Anchor::BottomRight => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Right, true);
        }
        Anchor::Cursor => {
            let (cx, cy) = get_cursor_position();
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
            window.set_margin(Edge::Top, cy);
            window.set_margin(Edge::Left, cx);
        }
    }

    if cfg.margin_top != 0 {
        window.set_margin(Edge::Top, cfg.margin_top);
    }
    if cfg.margin_bottom != 0 {
        window.set_margin(Edge::Bottom, cfg.margin_bottom);
    }
    if cfg.margin_left != 0 {
        window.set_margin(Edge::Left, cfg.margin_left);
    }
    if cfg.margin_right != 0 {
        window.set_margin(Edge::Right, cfg.margin_right);
    }
}

pub fn get_cursor_position() -> (i32, i32) {
    if let Ok(out) = Command::new("hyprctl").arg("cursorpos").output() {
        let s = String::from_utf8_lossy(&out.stdout);
        if let Some((x, y)) = s.trim().split_once(',') {
            return (x.trim().parse().unwrap_or(0), y.trim().parse().unwrap_or(0));
        }
    }
    (0, 0)
}

pub fn update_cursor_position(window: &gtk4::Window) {
    let (cx, cy) = get_cursor_position();
    window.set_margin(Edge::Top, cy);
    window.set_margin(Edge::Left, cx);
}
