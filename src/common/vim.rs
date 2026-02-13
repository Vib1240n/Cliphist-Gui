use crate::keys::{key_to_char, VimMode};
use gtk4::prelude::*;
use gtk4::Label;
use std::cell::RefCell;

thread_local! {
    pub static VIM_STATE: RefCell<VimMode> = const { RefCell::new(VimMode::Normal) };
    pub static LAST_KEY: RefCell<Option<char>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VimAction {
    EnterInsert,
    ExitInsert,
    Close,
    Down,
    Up,
    Top,
    Bottom,
    HalfPageDown,
    HalfPageUp,
    Select,
    Delete,
}

pub fn set_vim_mode(mode: VimMode) {
    VIM_STATE.with(|s| *s.borrow_mut() = mode);
    LAST_KEY.with(|k| *k.borrow_mut() = None);
}

pub fn get_vim_mode() -> VimMode {
    VIM_STATE.with(|s| *s.borrow())
}

pub fn update_mode_display(label: &Label, mode: VimMode) {
    match mode {
        VimMode::Normal => {
            label.set_text("NORMAL");
            label.remove_css_class("vim-mode-insert");
            label.add_css_class("vim-mode-normal");
        }
        VimMode::Insert => {
            label.set_text("INSERT");
            label.remove_css_class("vim-mode-normal");
            label.add_css_class("vim-mode-insert");
        }
    }
}

/// Handle vim key press in Normal mode
/// Returns Some(VimAction) if handled, None if not
/// `allow_delete` enables dd sequence (for cliphist)
pub fn handle_vim_normal_key(
    key: gdk4::Key,
    mods: gdk4::ModifierType,
    allow_delete: bool,
) -> Option<VimAction> {
    let key_char = key_to_char(key);
    // Escape -> close
    if key == gdk4::Key::Escape {
        return Some(VimAction::Close);
    }
    // Enter -> select
    if key == gdk4::Key::Return {
        return Some(VimAction::Select);
    }
    // Check for vim keys
    if let Some(c) = key_char {
        match c {
            'i' | 'a' | 'A' | 'I' | '/' => {
                return Some(VimAction::EnterInsert);
            }
            'j' => {
                LAST_KEY.with(|k| *k.borrow_mut() = None);
                return Some(VimAction::Down);
            }
            'k' => {
                LAST_KEY.with(|k| *k.borrow_mut() = None);
                return Some(VimAction::Up);
            }
            'g' => {
                let last = LAST_KEY.with(|k| *k.borrow());
                if last == Some('g') {
                    LAST_KEY.with(|k| *k.borrow_mut() = None);
                    return Some(VimAction::Top);
                } else {
                    LAST_KEY.with(|k| *k.borrow_mut() = Some('g'));
                    return None;
                }
            }
            'G' => {
                LAST_KEY.with(|k| *k.borrow_mut() = None);
                return Some(VimAction::Bottom);
            }
            'd' if allow_delete => {
                let last = LAST_KEY.with(|k| *k.borrow());
                if last == Some('d') {
                    LAST_KEY.with(|k| *k.borrow_mut() = None);
                    return Some(VimAction::Delete);
                } else {
                    LAST_KEY.with(|k| *k.borrow_mut() = Some('d'));
                    return None;
                }
            }
            _ => {
                LAST_KEY.with(|k| *k.borrow_mut() = None);
            }
        }
    }
    // Ctrl+d / Ctrl+u for half page
    if mods.contains(gdk4::ModifierType::CONTROL_MASK) {
        if let Some(c) = key_char {
            match c {
                'd' => return Some(VimAction::HalfPageDown),
                'u' => return Some(VimAction::HalfPageUp),
                _ => {}
            }
        }
    }
    None
}

/// Handle vim key press in Insert mode
/// Returns Some(VimAction) if handled (only Escape), None to pass through
pub fn handle_vim_insert_key(key: gdk4::Key) -> Option<VimAction> {
    if key == gdk4::Key::Escape {
        return Some(VimAction::ExitInsert);
    }
    None
}
