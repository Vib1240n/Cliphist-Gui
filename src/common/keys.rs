use std::collections::HashMap;
use gdk4::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Select, Delete, ClearSearch, Close,
    Next, Prev, PageDown, PageUp, First, Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VimMode {
    #[default]
    Normal,
    Insert,
}

#[derive(Clone, Debug)]
pub struct KeyCombo {
    pub key: gdk4::Key,
    pub mods: gdk4::ModifierType,
}

pub fn parse_action(s: &str) -> Option<Action> {
    match s {
        "select" => Some(Action::Select),
        "delete" => Some(Action::Delete),
        "clear_search" => Some(Action::ClearSearch),
        "close" => Some(Action::Close),
        "next" => Some(Action::Next),
        "prev" => Some(Action::Prev),
        "page_down" => Some(Action::PageDown),
        "page_up" => Some(Action::PageUp),
        "first" => Some(Action::First),
        "last" => Some(Action::Last),
        _ => None,
    }
}

pub fn parse_key_combos(s: &str) -> Vec<KeyCombo> {
    s.split_whitespace().filter_map(parse_single_combo).collect()
}

pub fn parse_single_combo(s: &str) -> Option<KeyCombo> {
    let parts: Vec<&str> = s.split('+').collect();
    let mut mods = gdk4::ModifierType::empty();
    let key_str = parts.last()?;

    for &p in &parts[..parts.len() - 1] {
        match p.to_lowercase().as_str() {
            "ctrl" | "control" => mods |= gdk4::ModifierType::CONTROL_MASK,
            "shift" => mods |= gdk4::ModifierType::SHIFT_MASK,
            "alt" | "mod1" => mods |= gdk4::ModifierType::ALT_MASK,
            "super" | "mod4" => mods |= gdk4::ModifierType::SUPER_MASK,
            _ => {}
        }
    }

    let key = match key_str.to_lowercase().as_str() {
        "return" | "enter" => gdk4::Key::Return,
        "escape" | "esc" => gdk4::Key::Escape,
        "tab" => gdk4::Key::Tab,
        "delete" | "del" => gdk4::Key::Delete,
        "backspace" => gdk4::Key::BackSpace,
        "up" => gdk4::Key::Up,
        "down" => gdk4::Key::Down,
        "left" => gdk4::Key::Left,
        "right" => gdk4::Key::Right,
        "home" => gdk4::Key::Home,
        "end" => gdk4::Key::End,
        "page_up" | "pageup" | "pgup" => gdk4::Key::Page_Up,
        "page_down" | "pagedown" | "pgdn" => gdk4::Key::Page_Down,
        "space" => gdk4::Key::space,
        s if s.len() == 1 => gdk4::Key::from_name(s)?,
        _ => return None,
    };
    Some(KeyCombo { key, mods })
}

pub fn match_action(keybinds: &HashMap<Action, Vec<KeyCombo>>, key: gdk4::Key, mods: gdk4::ModifierType) -> Option<Action> {
    let relevant = gdk4::ModifierType::CONTROL_MASK 
        | gdk4::ModifierType::SHIFT_MASK
        | gdk4::ModifierType::ALT_MASK 
        | gdk4::ModifierType::SUPER_MASK;
    let pressed = mods & relevant;
    
    for (action, combos) in keybinds {
        for combo in combos {
            if combo.key == key && combo.mods == pressed {
                return Some(action.clone());
            }
        }
    }
    None
}

/// Get the character for a key press (for vim mode)
pub fn key_to_char(key: gdk4::Key) -> Option<char> {
    key.to_unicode().filter(|c| c.is_ascii_graphic())
}

pub fn default_keybinds() -> HashMap<Action, Vec<KeyCombo>> {
    let mut kb = HashMap::new();
    kb.insert(Action::Select, vec![
        KeyCombo { key: gdk4::Key::Return, mods: gdk4::ModifierType::empty() },
        KeyCombo { key: gdk4::Key::KP_Enter, mods: gdk4::ModifierType::empty() },
    ]);
    kb.insert(Action::Delete, vec![
        KeyCombo { key: gdk4::Key::Delete, mods: gdk4::ModifierType::empty() },
    ]);
    kb.insert(Action::ClearSearch, vec![
        KeyCombo { key: gdk4::Key::u, mods: gdk4::ModifierType::CONTROL_MASK },
    ]);
    kb.insert(Action::Close, vec![
        KeyCombo { key: gdk4::Key::Escape, mods: gdk4::ModifierType::empty() },
    ]);
    kb.insert(Action::Next, vec![
        KeyCombo { key: gdk4::Key::Down, mods: gdk4::ModifierType::empty() },
        KeyCombo { key: gdk4::Key::Tab, mods: gdk4::ModifierType::empty() },
    ]);
    kb.insert(Action::Prev, vec![
        KeyCombo { key: gdk4::Key::Up, mods: gdk4::ModifierType::empty() },
        KeyCombo { key: gdk4::Key::Tab, mods: gdk4::ModifierType::SHIFT_MASK },
    ]);
    kb.insert(Action::PageDown, vec![
        KeyCombo { key: gdk4::Key::Page_Down, mods: gdk4::ModifierType::empty() },
    ]);
    kb.insert(Action::PageUp, vec![
        KeyCombo { key: gdk4::Key::Page_Up, mods: gdk4::ModifierType::empty() },
    ]);
    kb.insert(Action::First, vec![
        KeyCombo { key: gdk4::Key::Home, mods: gdk4::ModifierType::empty() },
    ]);
    kb.insert(Action::Last, vec![
        KeyCombo { key: gdk4::Key::End, mods: gdk4::ModifierType::empty() },
    ]);
    kb
}

