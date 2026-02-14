use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;

use gdk4::prelude::*;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, CssProvider, Entry, EventControllerKey,
    Label, ListBox, Orientation, ScrolledWindow,
};

use common::{
    config::Easing,
    css::load_css,
    keys::match_action,
    layer::{apply_layer_shell, update_cursor_position},
    logging::log,
    vim::{
        get_vim_mode, handle_vim_insert_key, handle_vim_normal_key, set_vim_mode,
        update_mode_display,
    },
    Anchor, VimAction, VimMode,
};

use crate::calc::calc_eval;
use crate::config::{default_css, Config, APP_NAME};
use crate::desktop::{launch_app, load_entries, DesktopEntry};
use crate::search::get_filtered_entry;
use crate::ui::populate_list;

pub struct AppWidgets {
    pub search: Entry,
    pub listbox: ListBox,
    pub scroll: ScrolledWindow,
    pub section_label: Label,
    pub status_bar: GtkBox,
    pub status: Label,
    pub mode_label: Label,
    pub container: GtkBox,
    pub entries: Rc<RefCell<Vec<DesktopEntry>>>,
}

thread_local! {
    pub static WIDGETS: RefCell<Option<AppWidgets>> = const { RefCell::new(None) };
    pub static CONFIG: RefCell<Config> = RefCell::new(Config::default());
    pub static EXPANDED: RefCell<bool> = const { RefCell::new(false) };
}

fn set_expanded(expanded: bool) {
    EXPANDED.with(|e| *e.borrow_mut() = expanded);
}

fn is_expanded() -> bool {
    EXPANDED.with(|e| *e.borrow())
}

/// Animate height transition
fn animate_height(
    container: &GtkBox,
    scroll: &ScrolledWindow,
    section_label: &Label,
    status_bar: &GtkBox,
    from_height: i32,
    to_height: i32,
    duration_ms: u64,
    easing: Easing,
    expanding: bool,
) {
    let steps = 20;
    let step_ms = duration_ms / steps;

    // Update CSS classes immediately
    if expanding {
        container.remove_css_class("collapsed");
        container.add_css_class("expanded");
        scroll.set_visible(true);
        section_label.set_visible(true);
        status_bar.set_visible(true);
    } else {
        container.remove_css_class("expanded");
        container.add_css_class("collapsed");
    }

    let container = container.clone();
    let scroll = scroll.clone();
    let section_label = section_label.clone();
    let status_bar = status_bar.clone();
    let step = Rc::new(std::cell::Cell::new(0u64));
    let step_clone = step.clone();

    let width = container.width();

    glib::timeout_add_local(std::time::Duration::from_millis(step_ms), move || {
        let s = step_clone.get() + 1;
        step_clone.set(s);

        let t = s as f64 / steps as f64;
        let eased = easing.apply(t);
        let current = from_height as f64 + (to_height - from_height) as f64 * eased;

        container.set_size_request(width, current as i32);

        if s >= steps {
            container.set_size_request(width, to_height);

            // Hide elements after collapse animation completes
            if !expanding {
                scroll.set_visible(false);
                section_label.set_visible(false);
                status_bar.set_visible(false);
            }

            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn expand(cfg: &Config) {
    if is_expanded() {
        return;
    }
    set_expanded(true);

    WIDGETS.with(|w| {
        if let Some(ref wg) = *w.borrow() {
            animate_height(
                &wg.container,
                &wg.scroll,
                &wg.section_label,
                &wg.status_bar,
                cfg.search_height,
                cfg.base.height,
                cfg.animation_duration,
                cfg.animation_easing,
                true,
            );
        }
    });
}

fn collapse(cfg: &Config) {
    if !is_expanded() {
        return;
    }
    set_expanded(false);

    WIDGETS.with(|w| {
        if let Some(ref wg) = *w.borrow() {
            animate_height(
                &wg.container,
                &wg.scroll,
                &wg.section_label,
                &wg.status_bar,
                cfg.base.height,
                cfg.search_height,
                cfg.animation_duration,
                cfg.animation_easing,
                false,
            );
        }
    });
}

pub fn activate(app: &Application) {
    let cfg = Config::load();
    CONFIG.with(|c| *c.borrow_mut() = cfg.clone());

    if cfg.vim_mode {
        set_vim_mode(VimMode::Normal);
    }

    // Reset to collapsed state
    set_expanded(false);

    if let Some(win) = app.active_window() {
        if win.is_visible() {
            win.set_visible(false);
        } else {
            if cfg.base.anchor == Anchor::Cursor {
                update_cursor_position(&win);
            }

            if cfg.vim_mode {
                set_vim_mode(VimMode::Normal);
            }

            // Reset to collapsed
            set_expanded(false);

            WIDGETS.with(|w| {
                if let Some(ref wg) = *w.borrow() {
                    let ents = wg.entries.borrow();
                    let _ = populate_list(&wg.listbox, &ents, "", cfg.calculator);
                    wg.status.set_text(&format!("{} apps", ents.len()));
                    wg.search.set_text("");

                    // Start collapsed
                    wg.container
                        .set_size_request(cfg.base.width, cfg.search_height);
                    wg.scroll.set_visible(false);
                    wg.section_label.set_visible(false);
                    wg.status_bar.set_visible(false);

                    if cfg.vim_mode {
                        update_mode_display(&wg.mode_label, VimMode::Normal);
                        wg.listbox.grab_focus();
                    } else {
                        wg.search.grab_focus();
                    }
                }
            });
            win.set_visible(true);
            win.present();
        }
        return;
    }

    let css_content = if let Ok(theme) = std::env::var("GUI_THEME_OVERRIDE") {
        common::paths::get_theme_css(&theme)
            .unwrap_or_else(|| load_css(APP_NAME, &cfg.base.theme, default_css()))
    } else if !cfg.base.theme.contains('/') && !cfg.base.theme.ends_with(".css") {
        common::paths::get_theme_css(&cfg.base.theme).unwrap_or_else(|| default_css().to_string())
    } else {
        load_css(APP_NAME, &cfg.base.theme, default_css())
    };

    let provider = CssProvider::new();
    provider.load_from_data(&css_content);
    gtk4::style_context_add_provider_for_display(
        &gdk4::Display::default().expect("no display"),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let entries: Rc<RefCell<Vec<DesktopEntry>>> = Rc::new(RefCell::new(Vec::new()));

    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(cfg.base.width)
        .default_height(cfg.search_height) // Start with collapsed height
        .resizable(false)
        .build();

    apply_layer_shell(&window, &cfg.base, APP_NAME);
    window.set_default_size(cfg.base.width, cfg.search_height);

    let container = GtkBox::new(Orientation::Vertical, 0);
    container.add_css_class("launch-container");
    container.add_css_class("collapsed"); // Start collapsed
    container.set_size_request(cfg.base.width, cfg.search_height);

    // search wrapper - for collapsed state padding
    let search_wrapper = GtkBox::new(Orientation::Vertical, 0);
    search_wrapper.add_css_class("launch-search-wrapper");

    let search_row = GtkBox::new(Orientation::Horizontal, 8);
    search_row.add_css_class("launch-search-row");
    let search = Entry::new();
    search.set_placeholder_text(Some("Search applications..."));
    search.add_css_class("launch-search");
    search.set_hexpand(true);
    search_row.append(&search);

    let hint_box = GtkBox::new(Orientation::Horizontal, 4);
    hint_box.set_valign(Align::Center);
    let esc_badge = Label::new(Some("esc"));
    esc_badge.add_css_class("launch-esc-badge");
    hint_box.append(&esc_badge);
    let hint_text = Label::new(Some("to close"));
    hint_text.add_css_class("launch-hint-text");
    hint_box.append(&hint_text);
    search_row.append(&hint_box);
    search_wrapper.append(&search_row);

    container.append(&search_wrapper);

    // expandable content
    let section_label = Label::new(Some("Applications"));
    section_label.set_xalign(0.0);
    section_label.add_css_class("launch-section-label");
    section_label.set_visible(false); // Start hidden
    container.append(&section_label);

    // list
    let scroll = ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);
    scroll.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
    scroll.set_visible(false); // Start hidden
    let listbox = ListBox::new();
    listbox.add_css_class("launch-list");
    listbox.set_selection_mode(gtk4::SelectionMode::Single);
    scroll.set_child(Some(&listbox));
    container.append(&scroll);
    let scroll_k = scroll.clone();

    // status bar
    let status_bar = GtkBox::new(Orientation::Horizontal, 0);
    status_bar.add_css_class("launch-status-bar");
    status_bar.set_visible(false); // Start hidden

    let mode_label = Label::new(Some(""));
    mode_label.add_css_class("vim-mode-indicator");
    mode_label.set_halign(Align::Start);
    if cfg.vim_mode {
        update_mode_display(&mode_label, VimMode::Normal);
        mode_label.set_visible(true);
    } else {
        mode_label.set_visible(false);
    }
    status_bar.append(&mode_label);

    let status = Label::new(Some("0 apps"));
    status.add_css_class("launch-status-left");
    status.set_halign(Align::Start);
    status.set_hexpand(true);
    status_bar.append(&status);

    let hints = GtkBox::new(Orientation::Horizontal, 12);
    hints.set_halign(Align::End);

    if cfg.vim_mode {
        for (k, h) in [("i", "insert"), ("j/k", "nav"), ("Enter", "launch")] {
            let b = GtkBox::new(Orientation::Horizontal, 0);
            let kl = Label::new(Some(k));
            kl.add_css_class("launch-status-key");
            b.append(&kl);
            let hl = Label::new(Some(h));
            hl.add_css_class("launch-status-hint");
            b.append(&hl);
            hints.append(&b);
        }
    } else {
        for (k, h) in [("Enter", "launch"), ("=", "calc")] {
            let b = GtkBox::new(Orientation::Horizontal, 0);
            let kl = Label::new(Some(k));
            kl.add_css_class("launch-status-key");
            b.append(&kl);
            let hl = Label::new(Some(h));
            hl.add_css_class("launch-status-hint");
            b.append(&hl);
            hints.append(&b);
        }
    }
    status_bar.append(&hints);
    container.append(&status_bar);
    window.set_child(Some(&container));

    // search handler - handles expand/collapse
    let entries_f = entries.clone();
    let listbox_f = listbox.clone();
    let status_f = status.clone();
    let cfg_f = cfg.clone();
    search.connect_changed(move |s| {
        let q = s.text().to_string();
        let ents = entries_f.borrow();
        let n = populate_list(&listbox_f, &ents, &q, cfg_f.calculator);

        if q.starts_with('=') {
            status_f.set_text("Calculator");
        } else {
            status_f.set_text(&format!("{} apps", n));
        }

        // Expand/collapse based on search text
        if !q.is_empty() && !is_expanded() {
            expand(&cfg_f);
        } else if q.is_empty() && is_expanded() {
            collapse(&cfg_f);
        }
    });

    // keybinds
    let key_ctrl = EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let ek = entries.clone();
    let lk = listbox.clone();
    let wk = window.clone();
    let sk = search.clone();
    let mode_k = mode_label.clone();
    let cfg_k = cfg.clone();

    key_ctrl.connect_key_pressed(move |_, key, _, mods| {
        let vim_enabled = CONFIG.with(|c| c.borrow().vim_mode);
        let terminal = CONFIG.with(|c| c.borrow().terminal.clone());
        let calc = CONFIG.with(|c| c.borrow().calculator);

        if vim_enabled {
            let current_mode = get_vim_mode();

            match current_mode {
                VimMode::Normal => {
                    if let Some(action) = handle_vim_normal_key(key, mods, false) {
                        match action {
                            VimAction::Close => {
                                wk.set_visible(false);
                            }
                            VimAction::Select => {
                                let q = sk.text().to_string();
                                if let Some(row) = lk.selected_row() {
                                    let ents = ek.borrow();
                                    if let Some(e) =
                                        get_filtered_entry(&ents, &q, row.index() as usize)
                                    {
                                        launch_app(&e, &terminal);
                                        wk.set_visible(false);
                                    }
                                }
                            }
                            VimAction::EnterInsert => {
                                set_vim_mode(VimMode::Insert);
                                update_mode_display(&mode_k, VimMode::Insert);
                                sk.grab_focus();

                                // Expand when entering insert mode
                                expand(&cfg_k);

                                let key_char = common::keys::key_to_char(key);
                                if let Some(c) = key_char {
                                    if c == 'A' || c == 'a' {
                                        sk.set_position(-1);
                                    } else if c == 'I' {
                                        sk.set_position(0);
                                    }
                                }
                            }
                            VimAction::Down => {
                                if let Some(r) = lk.selected_row() {
                                    if let Some(n) = lk.row_at_index(r.index() + 1) {
                                        lk.select_row(Some(&n));
                                        common::css::scroll_to_selected(&lk, &scroll_k);
                                    }
                                }
                            }
                            VimAction::Up => {
                                if let Some(r) = lk.selected_row() {
                                    if r.index() > 0 {
                                        if let Some(p) = lk.row_at_index(r.index() - 1) {
                                            lk.select_row(Some(&p));
                                            common::css::scroll_to_selected(&lk, &scroll_k);
                                        }
                                    }
                                }
                            }
                            VimAction::Top => {
                                if let Some(r) = lk.row_at_index(0) {
                                    lk.select_row(Some(&r));
                                    common::css::scroll_to_selected(&lk, &scroll_k);
                                }
                            }
                            VimAction::Bottom => {
                                let n = lk.observe_children().n_items();
                                if n > 0 {
                                    if let Some(r) = lk.row_at_index(n as i32 - 1) {
                                        lk.select_row(Some(&r));
                                        common::css::scroll_to_selected(&lk, &scroll_k);
                                    }
                                }
                            }
                            VimAction::HalfPageDown => {
                                if let Some(r) = lk.selected_row() {
                                    let t = (r.index() + 10)
                                        .min(lk.observe_children().n_items() as i32 - 1);
                                    if let Some(nr) = lk.row_at_index(t) {
                                        lk.select_row(Some(&nr));
                                        common::css::scroll_to_selected(&lk, &scroll_k);
                                    }
                                }
                            }
                            VimAction::HalfPageUp => {
                                if let Some(r) = lk.selected_row() {
                                    let t = (r.index() - 10).max(0);
                                    if let Some(nr) = lk.row_at_index(t) {
                                        lk.select_row(Some(&nr));
                                        common::css::scroll_to_selected(&lk, &scroll_k);
                                    }
                                }
                            }
                            VimAction::Delete => {} // Not used in launcher
                            _ => {}
                        }
                        return glib::Propagation::Stop;
                    }
                    return glib::Propagation::Stop;
                }
                VimMode::Insert => {
                    if let Some(action) = handle_vim_insert_key(key) {
                        if action == VimAction::ExitInsert {
                            set_vim_mode(VimMode::Normal);
                            update_mode_display(&mode_k, VimMode::Normal);
                            lk.grab_focus();

                            // Collapse when exiting insert mode if search is empty
                            if sk.text().is_empty() {
                                collapse(&cfg_k);
                            }
                        }
                    }
                    // Enter in insert mode -> select
                    if key == gdk4::Key::Return {
                        let q = sk.text().to_string();

                        if calc && q.starts_with('=') {
                            if let Some(result) = calc_eval(&q[1..]) {
                                let _ = Command::new("sh")
                                    .arg("-c")
                                    .arg(format!("echo -n '{}' | wl-copy", result))
                                    .spawn();
                                log(APP_NAME, &format!("copied math result: {}", result));
                                wk.set_visible(false);
                                return glib::Propagation::Stop;
                            }
                        }

                        if let Some(row) = lk.selected_row() {
                            let ents = ek.borrow();
                            if let Some(e) = get_filtered_entry(&ents, &q, row.index() as usize) {
                                launch_app(&e, &terminal);
                                wk.set_visible(false);
                            }
                        }
                        return glib::Propagation::Stop;
                    }

                    return glib::Propagation::Proceed;
                }
            }
        } else {
            // Non-vim mode
            let action = CONFIG.with(|c| match_action(&c.borrow().base.keybinds, key, mods));

            if let Some(action) = action {
                match action {
                    common::Action::Close => {
                        wk.set_visible(false);
                    }
                    common::Action::Select => {
                        let q = sk.text().to_string();

                        if calc && q.starts_with('=') {
                            if let Some(result) = calc_eval(&q[1..]) {
                                let _ = Command::new("sh")
                                    .arg("-c")
                                    .arg(format!("echo -n '{}' | wl-copy", result))
                                    .spawn();
                                log(APP_NAME, &format!("copied math result: {}", result));
                                wk.set_visible(false);
                                return glib::Propagation::Stop;
                            }
                        }

                        if let Some(row) = lk.selected_row() {
                            let ents = ek.borrow();
                            if let Some(e) = get_filtered_entry(&ents, &q, row.index() as usize) {
                                launch_app(&e, &terminal);
                                wk.set_visible(false);
                            }
                        }
                    }
                    common::Action::ClearSearch => {
                        sk.set_text("");
                    }
                    common::Action::Next => {
                        if let Some(r) = lk.selected_row() {
                            if let Some(n) = lk.row_at_index(r.index() + 1) {
                                lk.select_row(Some(&n));
                                common::css::scroll_to_selected(&lk, &scroll_k);
                            }
                        }
                    }
                    common::Action::Prev => {
                        if let Some(r) = lk.selected_row() {
                            if r.index() > 0 {
                                if let Some(p) = lk.row_at_index(r.index() - 1) {
                                    lk.select_row(Some(&p));
                                    common::css::scroll_to_selected(&lk, &scroll_k);
                                }
                            }
                        }
                    }
                    common::Action::PageDown => {
                        if let Some(r) = lk.selected_row() {
                            let t =
                                (r.index() + 10).min(lk.observe_children().n_items() as i32 - 1);
                            if let Some(nr) = lk.row_at_index(t) {
                                lk.select_row(Some(&nr));
                                common::css::scroll_to_selected(&lk, &scroll_k);
                            }
                        }
                    }
                    common::Action::PageUp => {
                        if let Some(r) = lk.selected_row() {
                            let t = (r.index() - 10).max(0);
                            if let Some(nr) = lk.row_at_index(t) {
                                lk.select_row(Some(&nr));
                                common::css::scroll_to_selected(&lk, &scroll_k);
                            }
                        }
                    }
                    common::Action::First => {
                        if let Some(r) = lk.row_at_index(0) {
                            lk.select_row(Some(&r));
                            common::css::scroll_to_selected(&lk, &scroll_k);
                        }
                    }
                    common::Action::Last => {
                        let n = lk.observe_children().n_items();
                        if n > 0 {
                            if let Some(r) = lk.row_at_index(n as i32 - 1) {
                                lk.select_row(Some(&r));
                                common::css::scroll_to_selected(&lk, &scroll_k);
                            }
                        }
                    }
                    _ => {}
                }
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });
    window.add_controller(key_ctrl);

    // click to launch
    let ec = entries.clone();
    let wc = window.clone();
    let sc = search.clone();
    let cfg_c = cfg.clone();
    listbox.connect_row_activated(move |_, row| {
        let q = sc.text().to_string();

        if cfg_c.calculator && q.starts_with('=') {
            if let Some(result) = calc_eval(&q[1..]) {
                let _ = Command::new("wl-copy").arg(&result).spawn();
                wc.set_visible(false);
                return;
            }
        }

        let ents = ec.borrow();
        if let Some(e) = get_filtered_entry(&ents, &q, row.index() as usize) {
            launch_app(&e, &cfg_c.terminal);
            wc.set_visible(false);
        }
    });

    WIDGETS.with(|w| {
        *w.borrow_mut() = Some(AppWidgets {
            search: search.clone(),
            listbox: listbox.clone(),
            scroll: scroll.clone(),
            section_label: section_label.clone(),
            status_bar: status_bar.clone(),
            status: status.clone(),
            mode_label: mode_label.clone(),
            container: container.clone(),
            entries: entries.clone(),
        });
    });

    {
        let mut ents = entries.borrow_mut();
        *ents = load_entries();
        let n = populate_list(&listbox, &ents, "", cfg.calculator);
        status.set_text(&format!("{} apps", n));
    }

    window.present();

    if cfg.vim_mode {
        listbox.grab_focus();
    } else {
        search.grab_focus();
    }

    log(
        APP_NAME,
        &format!(
            "daemon started ({}x{}, collapsed={}, anchor={:?}, vim={})",
            cfg.base.width, cfg.base.height, cfg.search_height, cfg.base.anchor, cfg.vim_mode
        ),
    );
}

pub fn setup_signals(app: &Application) {
    glib::unix_signal_add_local(libc::SIGUSR1, {
        let app = app.clone();
        move || {
            let cfg = Config::load();
            CONFIG.with(|c| *c.borrow_mut() = cfg.clone());

            if let Some(win) = app.active_window() {
                if win.is_visible() {
                    win.set_visible(false);
                } else {
                    if cfg.base.anchor == Anchor::Cursor {
                        update_cursor_position(&win);
                    }

                    if cfg.vim_mode {
                        set_vim_mode(VimMode::Normal);
                    }

                    // Reset to collapsed
                    set_expanded(false);

                    WIDGETS.with(|w| {
                        if let Some(ref wg) = *w.borrow() {
                            let ents = wg.entries.borrow();
                            let _ = populate_list(&wg.listbox, &ents, "", cfg.calculator);
                            wg.status.set_text(&format!("{} apps", ents.len()));
                            wg.search.set_text("");

                            // Start collapsed
                            wg.container
                                .set_size_request(cfg.base.width, cfg.search_height);
                            wg.scroll.set_visible(false);
                            wg.section_label.set_visible(false);
                            wg.status_bar.set_visible(false);

                            if cfg.vim_mode {
                                update_mode_display(&wg.mode_label, VimMode::Normal);
                                wg.listbox.grab_focus();
                            } else {
                                wg.search.grab_focus();
                            }
                        }
                    });
                    win.set_visible(true);
                    win.present();
                }
            }
            glib::ControlFlow::Continue
        }
    });

    glib::unix_signal_add_local(libc::SIGUSR2, {
        move || {
            let cfg = Config::load();
            CONFIG.with(|c| *c.borrow_mut() = cfg.clone());

            let provider = CssProvider::new();
            provider.load_from_data(&load_css(APP_NAME, &cfg.base.theme, default_css()));
            gtk4::style_context_add_provider_for_display(
                &gdk4::Display::default().expect("no display"),
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            log(APP_NAME, "config + css reloaded");
            glib::ControlFlow::Continue
        }
    });
}
