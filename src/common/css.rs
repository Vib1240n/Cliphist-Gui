use std::path::PathBuf;
use crate::logging::log;
use gtk4::prelude::*;
pub fn load_css(app_name: &str, theme_path: &str, default_css: &str) -> String {
    let p = PathBuf::from(theme_path);
    if p.exists() {
        if let Ok(css) = std::fs::read_to_string(&p) {
            log(app_name, &format!("loaded css from {}", p.display()));
            return css;
        }
    }
    log(app_name, &format!("theme not found: {}, using default", theme_path));
    default_css.to_string()
}

pub fn char_truncate(s: &str, max: usize) -> String {
    let t = s.trim().replace('\n', " ").replace('\t', " ");
    if t.chars().count() > max {
        format!("{}...", t.chars().take(max).collect::<String>())
    } else {
        t
    }
}

pub fn scroll_to_selected(listbox: &gtk4::ListBox) {
    if let Some(row) = listbox.selected_row() {
        row.grab_focus();

        if row.activate_action("list.scroll-to-item", None).is_err() {
            if let Some(adjustment) = listbox.adjustment() {
                let allocation = row.allocation();
                let row_top = allocation.y() as f64;
                let row_bottom = row_top + allocation.height() as f64;
                
                let view_top = adjustment.value();
                let view_bottom = view_top + adjustment.page_size();

                if row_top < view_top {
                    adjustment.set_value(row_top);
                } else if row_bottom > view_bottom {
                    adjustment.set_value(row_bottom - adjustment.page_size());
                }
            }
        }
    }
}
