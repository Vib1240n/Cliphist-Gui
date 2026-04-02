use std::cell::RefCell;
use std::rc::Rc;

use gdk4::prelude::*;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, CssProvider, Entry, EventControllerKey,
    GestureClick, Label, ListBox, Orientation, ScrolledWindow,
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
use crate::entries::{
    delete_entry, fetch_entries, get_filtered_entry, get_pinned_hash, load_pinned, pin_entry,
    select_entry, select_pinned, unpin_entry, ClipEntry, PinnedClipEntry,
};
use crate::ui::{
    create_tab_bar, populate_list, populate_pinned_list, update_pinned_count, update_tab_active,
    Tab,
};

pub struct AppWidgets {
    pub search: Entry,
    pub listbox: ListBox,
    pub status: Label,
    pub mode_label: Label,
    pub tab_bar: GtkBox,
    pub section_label: Label,
    pub entries: Rc<RefCell<Vec<ClipEntry>>>,
    pub pinned: Rc<RefCell<Vec<PinnedClipEntry>>>,
}

thread_local! {
    pub static WIDGETS: RefCell<Option<AppWidgets>> = const { RefCell::new(None) };
    pub static CONFIG: RefCell<Config> = RefCell::new(Config::default());
    pub static CURRENT_TAB: RefCell<Tab> = const { RefCell::new(Tab::Recent) };
}

fn get_current_tab() -> Tab {
    CURRENT_TAB.with(|t| *t.borrow())
}

fn set_current_tab(tab: Tab) {
    CURRENT_TAB.with(|t| *t.borrow_mut() = tab);
}

fn refresh_list(cfg: &Config) {
    WIDGETS.with(|w| {
        if let Some(ref wg) = *w.borrow() {
            let query = wg.search.text().to_string();
            let pinned = wg.pinned.borrow();
            let pinned_count = pinned.len();

            // Update tab bar visibility and count
            if pinned_count > 5 {
                wg.tab_bar.set_visible(true);
                wg.section_label.set_visible(false);
                update_pinned_count(&wg.tab_bar, pinned_count);
            } else {
                wg.tab_bar.set_visible(false);
                wg.section_label.set_visible(true);
            }

            let current_tab = get_current_tab();

            // If on pinned tab but pinned count dropped to <= 5, switch to recent
            if current_tab == Tab::Pinned && pinned_count <= 5 {
                set_current_tab(Tab::Recent);
            }

            let n = match get_current_tab() {
                Tab::Recent => {
                    let entries = wg.entries.borrow();
                    populate_list(&wg.listbox, &entries, &pinned, &query, cfg)
                }
                Tab::Pinned => populate_pinned_list(&wg.listbox, &pinned, &query, cfg, true),
            };

            let status_text = match get_current_tab() {
                Tab::Recent => {
                    if pinned_count > 0 && pinned_count <= 5 {
                        format!("{} items ({} pinned)", n, pinned_count)
                    } else {
                        format!("{} items", n)
                    }
                }
                Tab::Pinned => format!("{} pinned", n),
            };
            wg.status.set_text(&status_text);
        }
    });
}

fn switch_tab(tab: Tab, cfg: &Config) {
    set_current_tab(tab);
    WIDGETS.with(|w| {
        if let Some(ref wg) = *w.borrow() {
            update_tab_active(&wg.tab_bar, tab);
        }
    });
    refresh_list(cfg);
}

fn toggle_tab(cfg: &Config) {
    let current = get_current_tab();
    let pinned_count = WIDGETS.with(|w| {
        w.borrow()
            .as_ref()
            .map(|wg| wg.pinned.borrow().len())
            .unwrap_or(0)
    });

    // Only toggle if we have > 5 pinned (tabs visible)
    if pinned_count > 5 {
        let new_tab = match current {
            Tab::Recent => Tab::Pinned,
            Tab::Pinned => Tab::Recent,
        };
        switch_tab(new_tab, cfg);
    }
}

fn handle_pin(cfg: &Config) {
    WIDGETS.with(|w| {
        if let Some(ref wg) = *w.borrow() {
            let current_tab = get_current_tab();

            if current_tab == Tab::Pinned {
                // Can't pin from pinned tab (already pinned)
                return;
            }

            if let Some(row) = wg.listbox.selected_row() {
                let query = wg.search.text().to_string();
                let entries = wg.entries.borrow();

                if let Some(entry) = get_filtered_entry(&entries, &query, row.index() as usize) {
                    match pin_entry(&entry, cfg.max_pinned) {
                        Ok(()) => {
                            log(APP_NAME, &format!("pinned entry: {}", entry.id));
                            // Reload pinned
                            drop(entries);
                            let mut pinned = wg.pinned.borrow_mut();
                            *pinned = load_pinned();
                        }
                        Err(e) => {
                            log(APP_NAME, &format!("pin failed: {}", e));
                        }
                    }
                }
            }
        }
    });
    refresh_list(cfg);
}

fn handle_unpin(cfg: &Config) {
    WIDGETS.with(|w| {
        if let Some(ref wg) = *w.borrow() {
            if let Some(row) = wg.listbox.selected_row() {
                let widget_name = row.widget_name().to_string();

                let hash = if widget_name.starts_with("pinned:") {
                    // Direct pinned row
                    widget_name.strip_prefix("pinned:").map(|s| s.to_string())
                } else {
                    // Regular entry - check if pinned
                    let query = wg.search.text().to_string();
                    let entries = wg.entries.borrow();
                    get_filtered_entry(&entries, &query, row.index() as usize)
                        .and_then(|e| get_pinned_hash(&e))
                };

                if let Some(hash) = hash {
                    unpin_entry(&hash);
                    log(APP_NAME, &format!("unpinned: {}", hash));
                    // Reload pinned
                    let mut pinned = wg.pinned.borrow_mut();
                    *pinned = load_pinned();
                }
            }
        }
    });
    refresh_list(cfg);
}

fn handle_select(cfg: &Config) -> bool {
    let mut selected = false;
    WIDGETS.with(|w| {
        if let Some(ref wg) = *w.borrow() {
            if let Some(row) = wg.listbox.selected_row() {
                let widget_name = row.widget_name().to_string();

                if widget_name.starts_with("pinned:") {
                    // Pinned entry
                    let hash = widget_name.strip_prefix("pinned:").unwrap();
                    let pinned = wg.pinned.borrow();
                    if let Some(entry) = pinned.iter().find(|p| p.meta.hash == hash) {
                        select_pinned(entry, cfg.notify_on_copy);
                        selected = true;
                    }
                } else {
                    // Regular entry
                    let query = wg.search.text().to_string();
                    let entries = wg.entries.borrow();
                    if let Some(entry) = get_filtered_entry(&entries, &query, row.index() as usize)
                    {
                        select_entry(&entry, cfg.notify_on_copy);
                        selected = true;
                    }
                }
            }
        }
    });
    selected
}

fn handle_delete(cfg: &Config) {
    WIDGETS.with(|w| {
        if let Some(ref wg) = *w.borrow() {
            let current_tab = get_current_tab();

            if let Some(row) = wg.listbox.selected_row() {
                let widget_name = row.widget_name().to_string();

                if widget_name.starts_with("pinned:") {
                    // Deleting a pinned entry = unpin it
                    let hash = widget_name.strip_prefix("pinned:").unwrap();
                    unpin_entry(hash);
                    log(APP_NAME, &format!("deleted (unpinned): {}", hash));
                    let mut pinned = wg.pinned.borrow_mut();
                    *pinned = load_pinned();
                } else if current_tab == Tab::Recent {
                    // Regular delete from cliphist
                    let query = wg.search.text().to_string();
                    let entries = wg.entries.borrow();
                    if let Some(entry) = get_filtered_entry(&entries, &query, row.index() as usize)
                    {
                        delete_entry(&entry);
                        drop(entries);
                        let mut entries = wg.entries.borrow_mut();
                        *entries = fetch_entries(cfg.max_items);
                    }
                }
            }
        }
    });
    refresh_list(cfg);
}

pub fn activate(app: &Application) {
    let cfg = Config::load();
    CONFIG.with(|c| *c.borrow_mut() = cfg.clone());

    if cfg.vim_mode {
        set_vim_mode(VimMode::Normal);
    }

    set_current_tab(Tab::Recent);

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

            set_current_tab(Tab::Recent);

            WIDGETS.with(|w| {
                if let Some(ref wg) = *w.borrow() {
                    let mut entries = wg.entries.borrow_mut();
                    *entries = fetch_entries(cfg.max_items);
                    drop(entries);

                    let mut pinned = wg.pinned.borrow_mut();
                    *pinned = load_pinned();
                    drop(pinned);

                    wg.search.set_text("");

                    if cfg.vim_mode {
                        update_mode_display(&wg.mode_label, VimMode::Normal);
                        wg.listbox.grab_focus();
                    } else {
                        wg.search.grab_focus();
                    }
                }
            });

            refresh_list(&cfg);
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
    let pinned: Rc<RefCell<Vec<PinnedClipEntry>>> = Rc::new(RefCell::new(Vec::new()));

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

    // Header
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

    // Tab bar (hidden initially, shown when > 5 pinned)
    let tab_bar = create_tab_bar(0);
    tab_bar.set_visible(false);
    header.append(&tab_bar);

    // Section label (shown when <= 5 pinned)
    let section_label = Label::new(Some("Recent"));
    section_label.set_xalign(0.0);
    section_label.add_css_class("clip-section-label");
    header.append(&section_label);

    container.append(&header);

    // Tab click handlers
    let cfg_tab = cfg.clone();
    let tab_bar_click = tab_bar.clone();
    let click_ctrl = GestureClick::new();
    click_ctrl.connect_released(move |_, _, x, _| {
        let mut child = tab_bar_click.first_child();
        let mut offset = 0.0;
        while let Some(widget) = child {
            let width = widget.width() as f64;
            if x >= offset && x < offset + width {
                if let Some(label) = widget.downcast_ref::<Label>() {
                    let name = label.widget_name();
                    if name == "tab-recent" {
                        switch_tab(Tab::Recent, &cfg_tab);
                    } else if name == "tab-pinned" {
                        switch_tab(Tab::Pinned, &cfg_tab);
                    }
                }
                break;
            }
            offset += width;
            child = widget.next_sibling();
        }
    });
    tab_bar.add_controller(click_ctrl);

    // List
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

    // Status bar
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
            ("p", "pin"),
            ("u", "unpin"),
            ("dd", "delete"),
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
        for (k, h) in [("Enter", "select"), ("Ctrl+p", "pin"), ("Del", "delete")] {
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

    // Search handler
    let cfg_search = cfg.clone();
    search.connect_changed(move |_| {
        refresh_list(&cfg_search);
    });

    // Key controller
    let key_ctrl = EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let wk = window.clone();
    let sk = search.clone();
    let lk = listbox.clone();
    let mode_k = mode_label.clone();

    key_ctrl.connect_key_pressed(move |_, key, _, mods| {
        let cfg = CONFIG.with(|c| c.borrow().clone());
        let vim_enabled = cfg.vim_mode;

        // Check for tab_switch keybind (handled via Action system below)
        // Alt+Tab default for switching tabs
        if key == gdk4::Key::Tab && mods.contains(gdk4::ModifierType::ALT_MASK) {
            toggle_tab(&cfg);
            return glib::Propagation::Stop;
        }

        if vim_enabled {
            let current_mode = get_vim_mode();

            match current_mode {
                VimMode::Normal => {
                    // allow_delete = true, allow_pin = true for cliphist
                    if let Some(action) = handle_vim_normal_key(key, mods, true, true) {
                        match action {
                            VimAction::Close => {
                                wk.set_visible(false);
                            }
                            VimAction::Select => {
                                if handle_select(&cfg) && cfg.close_on_select {
                                    wk.set_visible(false);
                                }
                            }
                            VimAction::Delete => {
                                handle_delete(&cfg);
                            }
                            VimAction::Pin => {
                                handle_pin(&cfg);
                            }
                            VimAction::Unpin => {
                                handle_unpin(&cfg);
                            }
                            VimAction::TabSwitch => {
                                toggle_tab(&cfg);
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
                        if action == VimAction::ExitInsert {
                            set_vim_mode(VimMode::Normal);
                            update_mode_display(&mode_k, VimMode::Normal);
                            lk.grab_focus();
                        }
                    }
                    if key == gdk4::Key::Return {
                        if handle_select(&cfg) && cfg.close_on_select {
                            wk.set_visible(false);
                        }
                        return glib::Propagation::Stop;
                    }
                    return glib::Propagation::Proceed;
                }
            }
        } else {
            // Non-vim mode
            let action = match_action(&cfg.base.keybinds, key, mods);

            if let Some(action) = action {
                match action {
                    Action::Close => {
                        wk.set_visible(false);
                    }
                    Action::Select => {
                        if handle_select(&cfg) && cfg.close_on_select {
                            wk.set_visible(false);
                        }
                    }
                    Action::Delete => {
                        handle_delete(&cfg);
                    }
                    Action::Pin => {
                        handle_pin(&cfg);
                    }
                    Action::Unpin => {
                        handle_unpin(&cfg);
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
                    Action::TabSwitch => {
                        toggle_tab(&cfg);
                    }
                }
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });
    window.add_controller(key_ctrl);

    // Click to select
    let cfg_click = cfg.clone();
    let wc = window.clone();
    listbox.connect_row_activated(move |_, row| {
        let widget_name = row.widget_name().to_string();

        WIDGETS.with(|w| {
            if let Some(ref wg) = *w.borrow() {
                if widget_name.starts_with("pinned:") {
                    let hash = widget_name.strip_prefix("pinned:").unwrap();
                    let pinned = wg.pinned.borrow();
                    if let Some(entry) = pinned.iter().find(|p| p.meta.hash == hash) {
                        select_pinned(entry, cfg_click.notify_on_copy);
                        if cfg_click.close_on_select {
                            wc.set_visible(false);
                        }
                    }
                } else {
                    let query = wg.search.text().to_string();
                    let entries = wg.entries.borrow();
                    if let Some(entry) = get_filtered_entry(&entries, &query, row.index() as usize)
                    {
                        select_entry(&entry, cfg_click.notify_on_copy);
                        if cfg_click.close_on_select {
                            wc.set_visible(false);
                        }
                    }
                }
            }
        });
    });

    WIDGETS.with(|w| {
        *w.borrow_mut() = Some(AppWidgets {
            search: search.clone(),
            listbox: listbox.clone(),
            status: status.clone(),
            mode_label: mode_label.clone(),
            tab_bar: tab_bar.clone(),
            section_label: section_label.clone(),
            entries: entries.clone(),
            pinned: pinned.clone(),
        });
    });

    {
        let mut ents = entries.borrow_mut();
        *ents = fetch_entries(cfg.max_items);
        drop(ents);

        let mut pins = pinned.borrow_mut();
        *pins = load_pinned();
        drop(pins);
    }

    refresh_list(&cfg);

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

                    set_current_tab(Tab::Recent);

                    WIDGETS.with(|w| {
                        if let Some(ref wg) = *w.borrow() {
                            let mut entries = wg.entries.borrow_mut();
                            *entries = fetch_entries(cfg.max_items);
                            drop(entries);

                            let mut pinned = wg.pinned.borrow_mut();
                            *pinned = load_pinned();
                            drop(pinned);

                            wg.search.set_text("");

                            if cfg.vim_mode {
                                update_mode_display(&wg.mode_label, VimMode::Normal);
                                wg.listbox.grab_focus();
                            } else {
                                wg.search.grab_focus();
                            }
                        }
                    });

                    refresh_list(&cfg);
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
