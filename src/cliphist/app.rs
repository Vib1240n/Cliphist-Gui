use std::cell::RefCell;
use std::rc::Rc;

use gdk4::prelude::*;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, CssProvider, Entry, EventControllerKey,
    Label, ListBox, Orientation, ScrolledWindow,
};

use common::{
    css::load_css,
    keys::match_action,
    layer::{apply_layer_shell, update_cursor_position},
    logging::log,
    vim::{
        get_vim_mode, handle_vim_insert_key, handle_vim_normal_key, set_vim_mode,
        update_mode_display,
    },
    Action, Anchor, VimAction, VimMode,
};

use crate::config::{default_css, Config, APP_NAME};
use crate::entries::{delete_entry, fetch_entries, get_filtered_entry, select_entry, ClipEntry};
use crate::ui::populate_list;

pub struct AppWidgets {
    pub search: Entry,
    pub listbox: ListBox,
    pub status: Label,
    pub mode_label: Label,
    pub entries: Rc<RefCell<Vec<ClipEntry>>>,
}

thread_local! {
    pub static WIDGETS: RefCell<Option<AppWidgets>> = RefCell::new(None);
    pub static CONFIG: RefCell<Config> = RefCell::new(Config::default());
}

pub fn activate(app: &Application) {
    let cfg = Config::load();
    CONFIG.with(|c| *c.borrow_mut() = cfg.clone());

    if cfg.vim_mode {
        set_vim_mode(VimMode::Normal);
    }

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

            WIDGETS.with(|w| {
                if let Some(ref wg) = *w.borrow() {
                    let mut ents = wg.entries.borrow_mut();
                    *ents = fetch_entries(cfg.max_items);
                    let n = populate_list(&wg.listbox, &ents, "");
                    wg.status.set_text(&format!("{} items", n));
                    wg.search.set_text("");

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

    let entries: Rc<RefCell<Vec<ClipEntry>>> = Rc::new(RefCell::new(Vec::new()));

    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(cfg.base.width)
        .default_height(cfg.base.height)
        .resizable(false)
        .build();

    apply_layer_shell(&window, &cfg.base, APP_NAME);
    window.set_default_size(cfg.base.width, cfg.base.height);

    let container = GtkBox::new(Orientation::Vertical, 0);
    container.add_css_class("clip-container");
    container.set_size_request(cfg.base.width, cfg.base.height);

    // header
    let header = GtkBox::new(Orientation::Vertical, 0);
    header.add_css_class("clip-header");

    let search_row = GtkBox::new(Orientation::Horizontal, 8);
    search_row.add_css_class("clip-search-row");
    let search = Entry::new();
    search.set_placeholder_text(Some("Search clipboard history..."));
    search.add_css_class("clip-search");
    search.set_hexpand(true);
    search_row.append(&search);

    let hint_box = GtkBox::new(Orientation::Horizontal, 4);
    hint_box.set_valign(Align::Center);
    let esc_badge = Label::new(Some("esc"));
    esc_badge.add_css_class("clip-esc-badge");
    hint_box.append(&esc_badge);
    let hint_text = Label::new(Some("to close"));
    hint_text.add_css_class("clip-hint-text");
    hint_box.append(&hint_text);
    search_row.append(&hint_box);
    header.append(&search_row);

    let recent_label = Label::new(Some("Recent"));
    recent_label.set_xalign(0.0);
    recent_label.add_css_class("clip-section-label");
    header.append(&recent_label);
    container.append(&header);

    // list
    let scroll = ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);
    scroll.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
    let listbox = ListBox::new();
    listbox.add_css_class("clip-list");
    listbox.set_selection_mode(gtk4::SelectionMode::Single);
    scroll.set_child(Some(&listbox));
    container.append(&scroll);
    let scroll_k = scroll.clone();

    // status bar
    let status_bar = GtkBox::new(Orientation::Horizontal, 0);
    status_bar.add_css_class("clip-status-bar");

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

    let status = Label::new(Some("0 items"));
    status.add_css_class("clip-status-left");
    status.set_halign(Align::Start);
    status.set_hexpand(true);
    status_bar.append(&status);

    let hints = GtkBox::new(Orientation::Horizontal, 12);
    hints.set_halign(Align::End);

    if cfg.vim_mode {
        for (k, h) in [
            ("i", "insert"),
            ("j/k", "nav"),
            ("dd", "delete"),
            ("Enter", "select"),
        ] {
            let b = GtkBox::new(Orientation::Horizontal, 0);
            let kl = Label::new(Some(k));
            kl.add_css_class("clip-status-key");
            b.append(&kl);
            let hl = Label::new(Some(h));
            hl.add_css_class("clip-status-hint");
            b.append(&hl);
            hints.append(&b);
        }
    } else {
        for (k, h) in [("Enter", "select"), ("Del", "delete")] {
            let b = GtkBox::new(Orientation::Horizontal, 0);
            let kl = Label::new(Some(k));
            kl.add_css_class("clip-status-key");
            b.append(&kl);
            let hl = Label::new(Some(h));
            hl.add_css_class("clip-status-hint");
            b.append(&hl);
            hints.append(&b);
        }
    }
    status_bar.append(&hints);
    container.append(&status_bar);
    window.set_child(Some(&container));

    // search handler
    let entries_f = entries.clone();
    let listbox_f = listbox.clone();
    let status_f = status.clone();
    search.connect_changed(move |s| {
        let q = s.text().to_string();
        let ents = entries_f.borrow();
        let n = populate_list(&listbox_f, &ents, &q);
        status_f.set_text(&format!("{} items", n));
    });

    // keybinds
    let key_ctrl = EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let ek = entries.clone();
    let lk = listbox.clone();
    let wk = window.clone();
    let sk = search.clone();
    let stk = status.clone();
    let mode_k = mode_label.clone();

    key_ctrl.connect_key_pressed(move |_, key, _, mods| {
        let vim_enabled = CONFIG.with(|c| c.borrow().vim_mode);
        let close_on_select = CONFIG.with(|c| c.borrow().close_on_select);
        let notify = CONFIG.with(|c| c.borrow().notify_on_copy);
        let max_items = CONFIG.with(|c| c.borrow().max_items);

        if vim_enabled {
            let current_mode = get_vim_mode();

            match current_mode {
                VimMode::Normal => {
                    // allow_delete = true for cliphist (dd works)
                    if let Some(action) = handle_vim_normal_key(key, mods, true) {
                        match action {
                            VimAction::Close => {
                                wk.set_visible(false);
                            }
                            VimAction::Select => {
                                if let Some(row) = lk.selected_row() {
                                    let ents = ek.borrow();
                                    if let Some(e) =
                                        get_filtered_entry(&ents, &sk.text(), row.index() as usize)
                                    {
                                        select_entry(&e, notify);
                                        if close_on_select {
                                            wk.set_visible(false);
                                        }
                                    }
                                }
                            }
                            VimAction::Delete => {
                                if let Some(row) = lk.selected_row() {
                                    let ents = ek.borrow();
                                    if let Some(e) =
                                        get_filtered_entry(&ents, &sk.text(), row.index() as usize)
                                    {
                                        delete_entry(&e);
                                    }
                                    drop(ents);
                                    let mut ents = ek.borrow_mut();
                                    *ents = fetch_entries(max_items);
                                    let n = populate_list(&lk, &ents, &sk.text());
                                    stk.set_text(&format!("{} items", n));
                                }
                            }
                            VimAction::EnterInsert => {
                                set_vim_mode(VimMode::Insert);
                                update_mode_display(&mode_k, VimMode::Insert);
                                sk.grab_focus();
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
                            _ => {}
                        }
                        return glib::Propagation::Stop;
                    }
                    return glib::Propagation::Stop;
                }
                VimMode::Insert => {
                    if let Some(action) = handle_vim_insert_key(key) {
                        match action {
                            VimAction::ExitInsert => {
                                set_vim_mode(VimMode::Normal);
                                update_mode_display(&mode_k, VimMode::Normal);
                                lk.grab_focus();
                            }
                            _ => {}
                        }
                        return glib::Propagation::Stop;
                    }

                    // Enter in insert mode -> select
                    if key == gdk4::Key::Return {
                        if let Some(row) = lk.selected_row() {
                            let ents = ek.borrow();
                            if let Some(e) =
                                get_filtered_entry(&ents, &sk.text(), row.index() as usize)
                            {
                                select_entry(&e, notify);
                                if close_on_select {
                                    wk.set_visible(false);
                                }
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
                    Action::Close => {
                        wk.set_visible(false);
                    }
                    Action::Select => {
                        if let Some(row) = lk.selected_row() {
                            let ents = ek.borrow();
                            if let Some(e) =
                                get_filtered_entry(&ents, &sk.text(), row.index() as usize)
                            {
                                select_entry(&e, notify);
                                if close_on_select {
                                    wk.set_visible(false);
                                }
                            }
                        }
                    }
                    Action::Delete => {
                        if let Some(row) = lk.selected_row() {
                            let ents = ek.borrow();
                            if let Some(e) =
                                get_filtered_entry(&ents, &sk.text(), row.index() as usize)
                            {
                                delete_entry(&e);
                            }
                            drop(ents);
                            let mut ents = ek.borrow_mut();
                            *ents = fetch_entries(max_items);
                            let n = populate_list(&lk, &ents, &sk.text());
                            stk.set_text(&format!("{} items", n));
                        }
                    }
                    Action::ClearSearch => {
                        sk.set_text("");
                    }
                    Action::Next => {
                        if let Some(r) = lk.selected_row() {
                            if let Some(n) = lk.row_at_index(r.index() + 1) {
                                lk.select_row(Some(&n));
                                common::css::scroll_to_selected(&lk, &scroll_k);
                            }
                        }
                    }
                    Action::Prev => {
                        if let Some(r) = lk.selected_row() {
                            if r.index() > 0 {
                                if let Some(p) = lk.row_at_index(r.index() - 1) {
                                    lk.select_row(Some(&p));
                                    common::css::scroll_to_selected(&lk, &scroll_k);
                                }
                            }
                        }
                    }
                    Action::PageDown => {
                        if let Some(r) = lk.selected_row() {
                            let t =
                                (r.index() + 10).min(lk.observe_children().n_items() as i32 - 1);
                            if let Some(nr) = lk.row_at_index(t) {
                                lk.select_row(Some(&nr));
                                common::css::scroll_to_selected(&lk, &scroll_k);
                            }
                        }
                    }
                    Action::PageUp => {
                        if let Some(r) = lk.selected_row() {
                            let t = (r.index() - 10).max(0);
                            if let Some(nr) = lk.row_at_index(t) {
                                lk.select_row(Some(&nr));
                                common::css::scroll_to_selected(&lk, &scroll_k);
                            }
                        }
                    }
                    Action::First => {
                        if let Some(r) = lk.row_at_index(0) {
                            lk.select_row(Some(&r));
                            common::css::scroll_to_selected(&lk, &scroll_k);
                        }
                    }
                    Action::Last => {
                        let n = lk.observe_children().n_items();
                        if n > 0 {
                            if let Some(r) = lk.row_at_index(n as i32 - 1) {
                                lk.select_row(Some(&r));
                                common::css::scroll_to_selected(&lk, &scroll_k);
                            }
                        }
                    }
                }
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });
    window.add_controller(key_ctrl);

    // click to select
    let ec = entries.clone();
    let wc = window.clone();
    let sc = search.clone();
    let cfg_c = cfg.clone();
    listbox.connect_row_activated(move |_, row| {
        let ents = ec.borrow();
        if let Some(e) = get_filtered_entry(&ents, &sc.text(), row.index() as usize) {
            select_entry(&e, cfg_c.notify_on_copy);
            if cfg_c.close_on_select {
                wc.set_visible(false);
            }
        }
    });

    WIDGETS.with(|w| {
        *w.borrow_mut() = Some(AppWidgets {
            search: search.clone(),
            listbox: listbox.clone(),
            status: status.clone(),
            mode_label: mode_label.clone(),
            entries: entries.clone(),
        });
    });

    {
        let mut ents = entries.borrow_mut();
        *ents = fetch_entries(cfg.max_items);
        let n = populate_list(&listbox, &ents, "");
        status.set_text(&format!("{} items", n));
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
            "daemon started ({}x{}, anchor={:?}, vim={})",
            cfg.base.width, cfg.base.height, cfg.base.anchor, cfg.vim_mode
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

                    WIDGETS.with(|w| {
                        if let Some(ref wg) = *w.borrow() {
                            let mut ents = wg.entries.borrow_mut();
                            *ents = fetch_entries(cfg.max_items);
                            let n = populate_list(&wg.listbox, &ents, "");
                            wg.status.set_text(&format!("{} items", n));
                            wg.search.set_text("");

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
