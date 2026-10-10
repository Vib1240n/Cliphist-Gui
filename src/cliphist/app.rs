use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Entry, EventControllerKey, Label,
    ListBox, ListBoxRow, Orientation, ScrolledWindow,
};

use common::{
    css::{apply_css, load_css, scroll_to_selected},
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
    self, delete_entry, fetch_entries, get_pinned_hash, load_pinned, move_pinned, pin_entry,
    select_entry, select_pinned, unpin_entry, ClipEntry, PinnedClipEntry,
};
use crate::ui::{
    create_tab_bar, populate_list, populate_pinned_list, refresh_thumbnails,
    update_pinned_count, update_tab_active, Tab, PINNED_PREFIX,
};

/// More pins than this and they move from the top of Recent to their own tab.
const INLINE_PIN_LIMIT: usize = 5;
const PAGE: i32 = 10;

pub struct AppWidgets {
    pub window: ApplicationWindow,
    pub search: Entry,
    pub listbox: ListBox,
    pub scroll: ScrolledWindow,
    pub status: Label,
    pub mode_label: Label,
    pub tab_bar: GtkBox,
    pub section_label: Label,
    pub entries: Rc<RefCell<Vec<ClipEntry>>>,
    pub pinned: Rc<RefCell<Vec<PinnedClipEntry>>>,
}

thread_local! {
    static WIDGETS: RefCell<Option<AppWidgets>> = const { RefCell::new(None) };
    static CONFIG: RefCell<Config> = RefCell::new(Config::default());
    static CURRENT_TAB: RefCell<Tab> = const { RefCell::new(Tab::Recent) };
}

/// One thumbnail worker at a time
static THUMB_WORKER: AtomicBool = AtomicBool::new(false);

fn with_wg<R>(f: impl FnOnce(&AppWidgets) -> R) -> Option<R> {
    WIDGETS.with(|w| w.borrow().as_ref().map(f))
}

fn cfg() -> Config {
    CONFIG.with(|c| c.borrow().clone())
}

fn current_tab() -> Tab {
    CURRENT_TAB.with(|t| *t.borrow())
}

fn set_current_tab(tab: Tab) {
    CURRENT_TAB.with(|t| *t.borrow_mut() = tab);
}

// ---------------------------------------------------------------------------
// What the selected row refers to
// ---------------------------------------------------------------------------

enum Target {
    Pinned(String),
    Entry(ClipEntry),
}

/// Resolve a row by its name (cliphist id or "pinned:<hash>"), never by its
/// position: pinned rows shown above the history used to shift every index.
fn target_for_row(wg: &AppWidgets, row: &ListBoxRow) -> Option<Target> {
    let name = row.widget_name();
    if let Some(hash) = name.strip_prefix(PINNED_PREFIX) {
        return Some(Target::Pinned(hash.to_string()));
    }
    wg.entries
        .borrow()
        .iter()
        .find(|e| e.id == name.as_str())
        .cloned()
        .map(Target::Entry)
}

fn selected_target(wg: &AppWidgets) -> Option<Target> {
    target_for_row(wg, &wg.listbox.selected_row()?)
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

/// Rebuild the list. `keep`: row index to reselect (clamped), e.g. after delete.
fn refresh_list(keep: Option<i32>) {
    with_wg(|wg| {
        let query = wg.search.text().to_string();
        let pinned = wg.pinned.borrow();
        let pinned_count = pinned.len();
        let tabs = pinned_count > INLINE_PIN_LIMIT;

        wg.tab_bar.set_visible(tabs);
        wg.section_label.set_visible(!tabs);
        if tabs {
            update_pinned_count(&wg.tab_bar, pinned_count);
        } else if current_tab() == Tab::Pinned {
            set_current_tab(Tab::Recent);
            update_tab_active(&wg.tab_bar, Tab::Recent);
        }

        let n = match current_tab() {
            Tab::Recent => {
                populate_list(&wg.listbox, &wg.entries.borrow(), &pinned, &query, !tabs)
            }
            Tab::Pinned => populate_pinned_list(&wg.listbox, &pinned, &query, on_pinned_moved),
        };

        let status = match current_tab() {
            Tab::Recent if tabs => format!("{n} items · {pinned_count} pinned (Ctrl+Tab)"),
            Tab::Recent if pinned_count > 0 => format!("{n} items ({pinned_count} pinned)"),
            Tab::Recent => format!("{n} items"),
            Tab::Pinned => format!("{n} pinned · drag to reorder"),
        };
        wg.status.set_text(&status);

        if let Some(i) = keep {
            select_index(wg, i);
        }
    });
}

fn reload_pinned() {
    with_wg(|wg| *wg.pinned.borrow_mut() = load_pinned());
}

fn on_pinned_moved(source: String, target: String) {
    move_pinned(&source, &target);
    reload_pinned();
    refresh_list(None);
}

fn switch_tab(tab: Tab) {
    set_current_tab(tab);
    with_wg(|wg| update_tab_active(&wg.tab_bar, tab));
    refresh_list(None);
}

fn toggle_tab() {
    let tabs_visible = with_wg(|wg| wg.pinned.borrow().len() > INLINE_PIN_LIMIT).unwrap_or(false);
    if tabs_visible {
        switch_tab(match current_tab() {
            Tab::Recent => Tab::Pinned,
            Tab::Pinned => Tab::Recent,
        });
    }
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Nav {
    By(i32),
    First,
    Last,
}

fn row_count(wg: &AppWidgets) -> i32 {
    wg.listbox.observe_children().n_items() as i32
}

fn select_index(wg: &AppWidgets, idx: i32) {
    let n = row_count(wg);
    if n == 0 {
        return;
    }
    if let Some(r) = wg.listbox.row_at_index(idx.clamp(0, n - 1)) {
        wg.listbox.select_row(Some(&r));
        scroll_to_selected(&wg.listbox, &wg.scroll);
    }
}

fn navigate(nav: Nav) {
    with_wg(|wg| {
        let cur = wg.listbox.selected_row().map_or(0, |r| r.index());
        let target = match nav {
            Nav::By(d) => cur + d,
            Nav::First => 0,
            Nav::Last => row_count(wg) - 1,
        };
        select_index(wg, target);
    });
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn copy_target(target: &Target, notify: bool) -> bool {
    match target {
        Target::Pinned(hash) => with_wg(|wg| {
            wg.pinned
                .borrow()
                .iter()
                .find(|p| &p.meta.hash == hash)
                .is_some_and(|p| select_pinned(p, notify))
        })
        .unwrap_or(false),
        Target::Entry(e) => select_entry(e, notify),
    }
}

fn hide() {
    with_wg(|wg| wg.window.set_visible(false));
}

fn handle_select() {
    let cfg = cfg();
    let copied = with_wg(selected_target)
        .flatten()
        .is_some_and(|t| copy_target(&t, cfg.notify_on_copy));
    if copied && cfg.close_on_select {
        hide();
    }
}

fn handle_row_activated(row: &ListBoxRow) {
    let cfg = cfg();
    let copied = with_wg(|wg| target_for_row(wg, row))
        .flatten()
        .is_some_and(|t| copy_target(&t, cfg.notify_on_copy));
    if copied && cfg.close_on_select {
        hide();
    }
}

fn selected_index() -> Option<i32> {
    with_wg(|wg| wg.listbox.selected_row().map(|r| r.index())).flatten()
}

/// Show a message in the status bar until the next refresh
fn flash_status(msg: &str) {
    with_wg(|wg| wg.status.set_text(msg));
}

fn handle_pin() {
    if current_tab() == Tab::Pinned {
        return; // already pinned
    }
    let Some(Target::Entry(entry)) = with_wg(selected_target).flatten() else {
        return;
    };
    let keep = selected_index();
    match pin_entry(&entry, cfg().max_pinned) {
        Ok(()) => {
            log(APP_NAME, &format!("pinned entry {}", entry.id));
            reload_pinned();
            refresh_list(keep);
        }
        Err(e) => {
            log(APP_NAME, &format!("pin failed: {e}"));
            flash_status(&e);
        }
    }
}

fn handle_unpin() {
    let hash = match with_wg(selected_target).flatten() {
        Some(Target::Pinned(h)) => Some(h),
        Some(Target::Entry(e)) => get_pinned_hash(&e),
        None => None,
    };
    let Some(hash) = hash else {
        flash_status("Not pinned");
        return;
    };
    let keep = selected_index();
    unpin_entry(&hash);
    log(APP_NAME, &format!("unpinned {hash}"));
    reload_pinned();
    refresh_list(keep);
}

fn handle_delete() {
    let keep = selected_index();
    match with_wg(selected_target).flatten() {
        // deleting a pin = unpinning it
        Some(Target::Pinned(hash)) => {
            unpin_entry(&hash);
            reload_pinned();
        }
        Some(Target::Entry(e)) => {
            delete_entry(&e);
            with_wg(|wg| wg.entries.borrow_mut().retain(|x| x.id != e.id));
        }
        None => return,
    }
    refresh_list(keep);
}

fn clear_search() {
    with_wg(|wg| wg.search.set_text(""));
}

// ---------------------------------------------------------------------------
// Thumbnails
// ---------------------------------------------------------------------------

/// Generate missing thumbnails (and prune stale ones) off the UI thread, then
/// swap them into the visible rows.
fn start_thumbnail_worker() {
    let Some((jobs, keep)) = with_wg(|wg| {
        let entries = wg.entries.borrow();
        let pinned = wg.pinned.borrow();
        (
            entries::thumb_jobs(&entries, &pinned),
            entries::thumb_keep_set(&entries, &pinned),
        )
    }) else {
        return;
    };
    if THUMB_WORKER.swap(true, Ordering::SeqCst) {
        return; // one is already running; the next open picks up the rest
    }
    std::thread::spawn(move || {
        let made = entries::generate_thumbs(&jobs);
        entries::prune_thumb_cache(&keep);
        THUMB_WORKER.store(false, Ordering::SeqCst);
        if made > 0 {
            glib::MainContext::default().invoke(|| {
                with_wg(|wg| refresh_thumbnails(&wg.listbox));
            });
        }
    });
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

fn css_for(cfg: &Config) -> String {
    if let Ok(theme) = std::env::var("GUI_THEME_OVERRIDE") {
        if let Some(css) = common::paths::get_theme_css(&theme) {
            return css;
        }
    }
    if !cfg.base.theme.contains('/') && !cfg.base.theme.ends_with(".css") {
        return common::paths::get_theme_css(&cfg.base.theme)
            .unwrap_or_else(|| default_css().to_string());
    }
    load_css(APP_NAME, &cfg.base.theme, default_css())
}

/// Show if hidden, hide if shown. Shared by first activation and SIGUSR1.
fn toggle_window() {
    let Some(visible) = with_wg(|wg| wg.window.is_visible()) else {
        return;
    };
    if visible {
        hide();
    } else {
        show_window();
    }
}

fn show_window() {
    let cfg = Config::load();
    CONFIG.with(|c| *c.borrow_mut() = cfg.clone());
    if cfg.vim_mode {
        set_vim_mode(VimMode::Normal);
    }
    set_current_tab(Tab::Recent);

    with_wg(|wg| {
        if cfg.base.anchor == Anchor::Cursor {
            update_cursor_position(wg.window.upcast_ref());
        }
        *wg.entries.borrow_mut() = fetch_entries(cfg.max_items);
        *wg.pinned.borrow_mut() = load_pinned();
        update_tab_active(&wg.tab_bar, Tab::Recent);
        wg.search.set_text("");
    });

    refresh_list(None);

    with_wg(|wg| {
        if cfg.vim_mode {
            update_mode_display(&wg.mode_label, VimMode::Normal);
            wg.mode_label.set_visible(true);
        } else {
            wg.mode_label.set_visible(false);
        }
        wg.window.set_visible(true);
        wg.window.present();
        if cfg.vim_mode {
            wg.listbox.grab_focus();
        } else {
            wg.search.grab_focus();
        }
    });

    start_thumbnail_worker();
}

fn hint(k: &str, h: &str) -> GtkBox {
    let b = GtkBox::new(Orientation::Horizontal, 0);
    let kl = Label::new(Some(k));
    kl.add_css_class("clip-status-key");
    b.append(&kl);
    let hl = Label::new(Some(h));
    hl.add_css_class("clip-status-hint");
    b.append(&hl);
    b
}

fn build_window(app: &Application, cfg: &Config) {
    apply_css(&css_for(cfg));

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

    // Tab bar (only when there are more than INLINE_PIN_LIMIT pins)
    let tab_bar = create_tab_bar(switch_tab);
    tab_bar.set_visible(false);
    header.append(&tab_bar);

    let section_label = Label::new(Some("Recent"));
    section_label.set_xalign(0.0);
    section_label.add_css_class("clip-section-label");
    header.append(&section_label);
    container.append(&header);

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

    // Status bar
    let status_bar = GtkBox::new(Orientation::Horizontal, 0);
    status_bar.add_css_class("clip-status-bar");

    let mode_label = Label::new(Some(""));
    mode_label.add_css_class("vim-mode-indicator");
    mode_label.set_halign(Align::Start);
    mode_label.set_visible(false);
    status_bar.append(&mode_label);

    let status = Label::new(Some("0 items"));
    status.add_css_class("clip-status-left");
    status.set_halign(Align::Start);
    status.set_hexpand(true);
    status_bar.append(&status);

    let hints = GtkBox::new(Orientation::Horizontal, 12);
    hints.set_halign(Align::End);
    let hint_list: &[(&str, &str)] = if cfg.vim_mode {
        &[("i", "insert"), ("p", "pin"), ("u", "unpin"), ("dd", "delete")]
    } else {
        &[("Enter", "select"), ("Ctrl+p", "pin")]
    };
    for (k, h) in hint_list {
        hints.append(&hint(k, h));
    }
    status_bar.append(&hints);
    container.append(&status_bar);
    window.set_child(Some(&container));

    search.connect_changed(|_| refresh_list(None));
    listbox.connect_row_activated(|_, row| handle_row_activated(row));

    let key_ctrl = EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
    key_ctrl.connect_key_pressed(|_, key, _, mods| on_key(key, mods));
    window.add_controller(key_ctrl);

    WIDGETS.with(|w| {
        *w.borrow_mut() = Some(AppWidgets {
            window,
            search,
            listbox,
            scroll,
            status,
            mode_label,
            tab_bar,
            section_label,
            entries: Rc::new(RefCell::new(Vec::new())),
            pinned: Rc::new(RefCell::new(Vec::new())),
        });
    });
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

fn on_key(key: gdk4::Key, mods: gdk4::ModifierType) -> glib::Propagation {
    let cfg = cfg();
    if cfg.vim_mode {
        on_key_vim(key, mods)
    } else {
        on_key_normal(&cfg, key, mods)
    }
}

/// Non-vim: only configured combos act. Plain letters (p, u, ...) are never
/// actions here (see common::keys::match_action), they type into search.
fn on_key_normal(cfg: &Config, key: gdk4::Key, mods: gdk4::ModifierType) -> glib::Propagation {
    let Some(action) = match_action(&cfg.base.keybinds, key, mods) else {
        return glib::Propagation::Proceed;
    };
    match action {
        Action::Close => hide(),
        Action::Select => handle_select(),
        Action::Delete => handle_delete(),
        Action::Pin => handle_pin(),
        Action::Unpin => handle_unpin(),
        Action::ClearSearch => clear_search(),
        Action::Next => navigate(Nav::By(1)),
        Action::Prev => navigate(Nav::By(-1)),
        Action::PageDown => navigate(Nav::By(PAGE)),
        Action::PageUp => navigate(Nav::By(-PAGE)),
        Action::First => navigate(Nav::First),
        Action::Last => navigate(Nav::Last),
        Action::TabSwitch => toggle_tab(),
    }
    glib::Propagation::Stop
}

/// Vim: Normal mode owns single-letter commands (p/u/dd/j/k/gg/G), Insert mode
/// types into search.
fn on_key_vim(key: gdk4::Key, mods: gdk4::ModifierType) -> glib::Propagation {
    let tab_switch = (key == gdk4::Key::Tab || key == gdk4::Key::ISO_Left_Tab)
        && mods.intersects(gdk4::ModifierType::CONTROL_MASK | gdk4::ModifierType::ALT_MASK);
    if tab_switch {
        toggle_tab();
        return glib::Propagation::Stop;
    }

    match get_vim_mode() {
        VimMode::Normal => {
            if let Some(action) = handle_vim_normal_key(key, mods, true, true) {
                match action {
                    VimAction::Close => hide(),
                    VimAction::Select => handle_select(),
                    VimAction::Delete => handle_delete(),
                    VimAction::Pin => handle_pin(),
                    VimAction::Unpin => handle_unpin(),
                    VimAction::TabSwitch => toggle_tab(),
                    VimAction::EnterInsert => {
                        set_vim_mode(VimMode::Insert);
                        with_wg(|wg| {
                            update_mode_display(&wg.mode_label, VimMode::Insert);
                            wg.search.grab_focus();
                        });
                    }
                    VimAction::Down => navigate(Nav::By(1)),
                    VimAction::Up => navigate(Nav::By(-1)),
                    VimAction::Top => navigate(Nav::First),
                    VimAction::Bottom => navigate(Nav::Last),
                    VimAction::HalfPageDown => navigate(Nav::By(PAGE)),
                    VimAction::HalfPageUp => navigate(Nav::By(-PAGE)),
                    VimAction::ExitInsert => {}
                }
            }
            // Normal mode swallows everything else (no typing into search)
            glib::Propagation::Stop
        }
        VimMode::Insert => {
            if handle_vim_insert_key(key) == Some(VimAction::ExitInsert) {
                set_vim_mode(VimMode::Normal);
                with_wg(|wg| {
                    update_mode_display(&wg.mode_label, VimMode::Normal);
                    wg.listbox.grab_focus();
                });
                return glib::Propagation::Stop;
            }
            if key == gdk4::Key::Return || key == gdk4::Key::KP_Enter {
                handle_select();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

pub fn activate(app: &Application) {
    if WIDGETS.with(|w| w.borrow().is_some()) {
        toggle_window();
        return;
    }
    let cfg = Config::load();
    CONFIG.with(|c| *c.borrow_mut() = cfg.clone());
    build_window(app, &cfg);
    show_window();
    log(
        APP_NAME,
        &format!(
            "daemon started ({}x{}, anchor={:?}, vim={})",
            cfg.base.width, cfg.base.height, cfg.base.anchor, cfg.vim_mode
        ),
    );
}

pub fn setup_signals(_app: &Application) {
    glib::unix_signal_add_local(libc::SIGUSR1, || {
        toggle_window();
        glib::ControlFlow::Continue
    });

    glib::unix_signal_add_local(libc::SIGUSR2, || {
        let cfg = Config::load();
        apply_css(&css_for(&cfg));
        CONFIG.with(|c| *c.borrow_mut() = cfg);
        log(APP_NAME, "config + css reloaded");
        glib::ControlFlow::Continue
    });
}
