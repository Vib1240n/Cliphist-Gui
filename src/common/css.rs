use std::path::PathBuf;
use crate::logging::log;

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

pub fn scroll_to_selected(listbox: &gtk4::ListBox, scroll: &gtk4::ScrolledWindow) {
    use gtk4::prelude::*;
    
    let Some(row) = listbox.selected_row() else { return };
    let adj = scroll.vadjustment();
    
    let alloc = row.allocation();
    let row_y = alloc.y() as f64;
    let row_h = alloc.height() as f64;
    let row_bottom = row_y + row_h;
    
    let view_top = adj.value();
    let view_h = adj.page_size();
    let view_bottom = view_top + view_h;
    
    let target = if row_y < view_top {
        row_y
    } else if row_bottom > view_bottom {
        row_bottom - view_h
    } else {
        return;
    };
    
    animate_scroll(adj, target);
}

fn animate_scroll(adj: gtk4::Adjustment, target: f64) {
    use gtk4::prelude::*;
    
    let start = adj.value();
    let diff = target - start;
    if diff.abs() < 1.0 { adj.set_value(target); return; }
    
    let duration_ms = 150;
    let steps = 15;
    let step_ms = duration_ms / steps;
    
    let adj_clone = adj.clone();
    let step = std::rc::Rc::new(std::cell::Cell::new(0));
    let step_clone = step.clone();
    
    glib::timeout_add_local(std::time::Duration::from_millis(step_ms as u64), move || {
        let s = step_clone.get() + 1;
        step_clone.set(s);
        
        let t = s as f64 / steps as f64;
        let eased = 1.0 - (1.0 - t).powi(3);
        let val = start + diff * eased;
        adj_clone.set_value(val);
        
        if s >= steps {
            adj_clone.set_value(target);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}
