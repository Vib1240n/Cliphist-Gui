//! launch-gui: apps, calculator, files, folder browsing, snippets.
//!
//! The search text *is* the mode (no hidden state to get out of sync):
//!   <files.shortcut> query   file search (default f')
//!   ~/... or /...            browse that folder (Tab gets you there)
//!   <snippets.prefix> query  snippets (default s', when enabled)
//!   \p name | trig = text    create a plain snippet (\e = encrypted)
//!   =expr                    calculator
//!   anything else            applications

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Entry, EventControllerKey, Label,
    ListBox, Orientation, ScrolledWindow,
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

use crate::calc::calc_eval;
use crate::config::{default_css, Config, APP_NAME};
use crate::desktop::{launch_app, load_entries, needs_reload, spawn_detached, DesktopEntry};
use crate::files::{self, display, FileHit};
use crate::search::filter_entries;
use crate::ui::{populate, Item, MAX_ROWS};

const PAGE: i32 = 10;

pub struct AppWidgets {
    pub window: ApplicationWindow,
    pub container: GtkBox,
    pub search: Entry,
    pub listbox: ListBox,
    pub scroll: ScrolledWindow,
    pub section_label: Label,
    pub status_bar: GtkBox,
    pub status: Label,
    pub hints: GtkBox,
    pub mode_label: Label,
    /// Right half in file/browse modes: folder contents or file preview
    pub preview_scroll: ScrolledWindow,
    pub preview_box: GtkBox,
    pub entries: Rc<RefCell<Vec<DesktopEntry>>>,
}

thread_local! {
    static WIDGETS: RefCell<Option<AppWidgets>> = const { RefCell::new(None) };
    static CONFIG: RefCell<Config> = RefCell::new(Config::default());
    static ITEMS: RefCell<Vec<Item>> = const { RefCell::new(Vec::new()) };
    static EXPANDED: Cell<bool> = const { Cell::new(false) };
    static ANIM_GEN: Cell<u64> = const { Cell::new(0) };
    /// bumps on every query change; stale async file results are dropped
    static SEARCH_GEN: Cell<u64> = const { Cell::new(0) };
    static DEBOUNCE: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
    static SNIPPET_KEY: RefCell<Option<[u8; 32]>> = const { RefCell::new(None) };
}

fn with_wg<R>(f: impl FnOnce(&AppWidgets) -> R) -> Option<R> {
    WIDGETS.with(|w| w.borrow().as_ref().map(f))
}

fn cfg() -> Config {
    CONFIG.with(|c| c.borrow().clone())
}

// ---------------------------------------------------------------------------
// Mode from query text
// ---------------------------------------------------------------------------

enum Mode {
    Apps(String),
    Calc(String),
    Files(String),
    Browse { dir: PathBuf, filter: String },
    Snippets(String),
    NewSnippet(String),
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".into())
}

fn mode_of(q: &str, cfg: &Config) -> Mode {
    if cfg.snippets.enabled && (q.starts_with("\\p ") || q.starts_with("\\e ")) {
        return Mode::NewSnippet(q.to_string());
    }
    if cfg.files.enabled && !cfg.files.shortcut.is_empty() {
        if let Some(rest) = q.strip_prefix(cfg.files.shortcut.as_str()) {
            return Mode::Files(rest.trim_start().to_string());
        }
    }
    if cfg.snippets.enabled && !cfg.snippets.prefix.is_empty() {
        if let Some(rest) = q.strip_prefix(cfg.snippets.prefix.as_str()) {
            return Mode::Snippets(rest.trim_start().to_string());
        }
    }
    if q == "~" || q.starts_with("~/") || q.starts_with('/') {
        let expanded = if q == "~" { format!("{}/", home()) } else if let Some(r) = q.strip_prefix('~') {
            format!("{}{r}", home())
        } else {
            q.to_string()
        };
        let cut = expanded.rfind('/').map_or(0, |i| i + 1);
        return Mode::Browse {
            dir: PathBuf::from(&expanded[..cut]),
            filter: expanded[cut..].to_string(),
        };
    }
    if cfg.calculator && q.len() > 1 && q.starts_with('=') {
        return Mode::Calc(q[1..].to_string());
    }
    Mode::Apps(q.to_string())
}

/// Path -> search text that browses it ("~/x/" form, trailing slash)
fn browse_text(dir: &Path) -> String {
    let d = display(&dir.to_string_lossy());
    if d.ends_with('/') { d } else { format!("{d}/") }
}

// ---------------------------------------------------------------------------
// Refresh
// ---------------------------------------------------------------------------

/// Store first, then build rows: selecting the first row fires the preview,
/// which reads ITEMS.
fn set_items(items: Vec<Item>) {
    ITEMS.with(|i| *i.borrow_mut() = items);
    ITEMS.with(|i| {
        let items = i.borrow();
        with_wg(|wg| populate(&wg.listbox, &items));
    });
}

fn set_preview_visible(on: bool) {
    with_wg(|wg| {
        wg.preview_scroll.set_visible(on);
        if !on {
            clear_box(&wg.preview_box);
        }
    });
}

fn clear_box(b: &GtkBox) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// Text-ish file (code, config, logs, markdown...) by name: opens in the editor
fn is_text(path: &Path) -> bool {
    let (ctype, _) = gio::content_type_guess(Some(path), &[]);
    gio::content_type_is_a(&ctype, "text/plain")
        || matches!(
            ctype.as_str(),
            "application/json" | "application/toml" | "application/x-yaml" | "application/xml"
                | "application/x-shellscript" | "application/javascript" | "application/x-desktop"
        )
}

fn preview_label(text: &str, class: &str) -> Label {
    let l = Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    l.add_css_class(class);
    l
}

/// Right pane: a folder's contents, or the head of a text file / file info.
/// Reads at most 8 KB of a file and 60 folder entries.
fn update_preview(index: Option<i32>) {
    let Some(Item::File(hit)) =
        index.and_then(|i| ITEMS.with(|it| it.borrow().get(i as usize).cloned()))
    else {
        with_wg(|wg| clear_box(&wg.preview_box));
        return;
    };
    with_wg(|wg| {
        if !wg.preview_scroll.is_visible() {
            return;
        }
        let pb = &wg.preview_box;
        clear_box(pb);
        let name = hit.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        pb.append(&preview_label(&name, "launch-preview-title"));

        if hit.is_dir {
            let entries = files::list_dir(&hit.path, "", 60);
            if entries.is_empty() {
                pb.append(&preview_label("Empty folder", "launch-preview-meta"));
            }
            for e in entries {
                let row = GtkBox::new(Orientation::Horizontal, 8);
                row.add_css_class("launch-preview-row");
                let icon = if e.is_dir {
                    gtk4::Image::from_icon_name("folder")
                } else {
                    let (ct, _) = gio::content_type_guess(Some(&e.path), &[]);
                    gtk4::Image::from_gicon(&gio::content_type_get_icon(&ct))
                };
                icon.set_pixel_size(16);
                row.append(&icon);
                let n = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                row.append(&preview_label(&if e.is_dir { format!("{n}/") } else { n }, "launch-preview-name"));
                pb.append(&row);
            }
            return;
        }

        let meta = std::fs::metadata(&hit.path).ok();
        let size = meta.as_ref().map_or(0, |m| m.len());
        let size_txt = match size {
            s if s >= 1 << 20 => format!("{:.1} MB", s as f64 / (1u64 << 20) as f64),
            s if s >= 1 << 10 => format!("{:.1} KB", s as f64 / 1024.0),
            s => format!("{s} B"),
        };
        let (ctype, _) = gio::content_type_guess(Some(&hit.path), &[]);
        pb.append(&preview_label(&format!("{size_txt} · {ctype}"), "launch-preview-meta"));

        if is_text(&hit.path) {
            use std::io::Read;
            let mut buf = Vec::with_capacity(8192);
            if let Ok(f) = std::fs::File::open(&hit.path) {
                let _ = f.take(8192).read_to_end(&mut buf);
            }
            let text: String = String::from_utf8_lossy(&buf).lines().take(80).collect::<Vec<_>>().join("\n");
            let l = Label::new(Some(&text));
            l.set_xalign(0.0);
            l.set_yalign(0.0);
            l.set_wrap(false);
            l.set_selectable(false);
            l.add_css_class("launch-preview-text");
            pb.append(&l);
        }
    });
}

/// Called (on the UI thread) when a scan or cache load finishes
pub fn on_index_updated() {
    let Some((visible, q)) = with_wg(|wg| (wg.window.is_visible(), wg.search.text().to_string())) else {
        return;
    };
    if visible && matches!(mode_of(&q, &cfg()), Mode::Files(_)) {
        on_query_changed(q);
    }
}

fn set_header(section: &str, status: &str, hints: &[(&str, &str)]) {
    with_wg(|wg| {
        wg.section_label.set_text(section);
        wg.status.set_text(status);
        while let Some(c) = wg.hints.first_child() {
            wg.hints.remove(&c);
        }
        for (k, h) in hints {
            let b = GtkBox::new(Orientation::Horizontal, 0);
            let kl = Label::new(Some(k));
            kl.add_css_class("launch-status-key");
            b.append(&kl);
            let hl = Label::new(Some(h));
            hl.add_css_class("launch-status-hint");
            b.append(&hl);
            wg.hints.append(&b);
        }
    });
}

fn on_query_changed(q: String) {
    // "\t" typed literally = Tab
    if q == "\\t" {
        with_wg(|wg| {
            wg.search.set_text("~/");
            wg.search.set_position(-1);
        });
        return;
    }
    let cfg = cfg();
    SEARCH_GEN.with(|g| g.set(g.get() + 1));

    let mode = mode_of(&q, &cfg);
    // split view (results | preview) only for files and browsing
    set_preview_visible(matches!(mode, Mode::Files(_) | Mode::Browse { .. }));

    match mode {
        Mode::Apps(query) => {
            let items: Vec<Item> = with_wg(|wg| {
                filter_entries(&wg.entries.borrow(), &query)
                    .into_iter()
                    .take(MAX_ROWS)
                    .map(Item::App)
                    .collect()
            })
            .unwrap_or_default();
            let n = items.len();
            set_items(items);
            set_header("Applications", &format!("{n} apps"), &[("Enter", "launch"), ("Tab", "browse")]);
        }
        Mode::Calc(expr) => {
            let items = match calc_eval(&expr) {
                Some(result) => vec![Item::Calc { expr, result }],
                None => vec![Item::Info("…".into())],
            };
            set_items(items);
            set_header("Calculator", "Enter copies the result", &[("Enter", "copy")]);
        }
        Mode::Files(query) => {
            files::ensure_fresh(&cfg.files);
            let count = files::indexed_count();
            let state = match (files::is_scanning(), count) {
                (true, 0) => "indexing…".to_string(),
                (true, n) => format!("{n} indexed · refreshing…"),
                (false, n) => format!("{n} indexed"),
            };
            set_header(
                "Files",
                &state,
                &[("Enter", "open"), ("Super+Enter", "file manager"), ("Tab", "browse")],
            );
            if count == 0 {
                // results arrive via on_index_updated when the scan finishes
                set_items(vec![Item::Info("Indexing files… results appear when done".into())]);
            } else if query.is_empty() {
                let hits = files::suggestions(cfg.files.max_results);
                set_items(if hits.is_empty() {
                    vec![Item::Info("Type to search files".into())]
                } else {
                    hits.into_iter().map(Item::File).collect()
                });
            } else {
                schedule_file_search(query, cfg.files.max_results);
            }
        }
        Mode::Browse { dir, filter } => {
            let items: Vec<Item> = if dir.is_dir() {
                let hits = files::list_dir(&dir, &filter, MAX_ROWS);
                if hits.is_empty() {
                    vec![Item::Info(if filter.is_empty() { "Empty folder" } else { "No match" }.into())]
                } else {
                    hits.into_iter().map(Item::File).collect()
                }
            } else {
                vec![Item::Info("Not a folder".into())]
            };
            let n = items.len();
            set_items(items);
            set_header(
                &browse_text(&dir),
                &format!("{n} items"),
                &[("Enter", "into/open"), ("←", "up"), ("Super+Enter", "file manager")],
            );
        }
        Mode::Snippets(query) => {
            let q = query.to_lowercase();
            let items: Vec<Item> = snippets::load(&cfg.snippets)
                .into_iter()
                .filter(|s| {
                    q.is_empty()
                        || s.name.to_lowercase().contains(&q)
                        || s.trigger.as_deref().is_some_and(|t| t.to_lowercase().contains(&q))
                })
                .map(Item::Snippet)
                .collect();
            let n = items.len();
            set_items(if items.is_empty() {
                vec![Item::Info("No snippets · create: \\p name | trigger = text".into())]
            } else {
                items
            });
            set_header("Snippets", &format!("{n} snippets"), &[("Enter", "type")]);
        }
        Mode::NewSnippet(line) => {
            set_items(vec![match snippets::parse_line(&line) {
                Some(s) => Item::NewSnippet(s),
                None => Item::Info("\\p name | trigger = content   (\\e encrypts)".into()),
            }]);
            set_header("New snippet", "", &[("Enter", "save")]);
        }
    }

    // grow on first character, shrink when cleared
    if !q.is_empty() && !EXPANDED.with(|e| e.get()) {
        expand(&cfg);
    } else if q.is_empty() && EXPANDED.with(|e| e.get()) {
        collapse(&cfg);
    }
}

/// Debounced (40 ms) file search on a worker thread; stale results dropped.
fn schedule_file_search(query: String, limit: usize) {
    let gen = SEARCH_GEN.with(|g| g.get());
    DEBOUNCE.with(|d| {
        if let Some(id) = d.borrow_mut().take() {
            id.remove();
        }
    });
    let id = glib::timeout_add_local_once(Duration::from_millis(40), move || {
        DEBOUNCE.with(|d| d.borrow_mut().take()); // fired: nothing to cancel anymore
        std::thread::spawn(move || {
            let candidates = files::search_index(&query);
            glib::MainContext::default().invoke(move || {
                if SEARCH_GEN.with(|g| g.get()) != gen {
                    return; // the query changed meanwhile
                }
                let hits = files::rank(&query, candidates, limit);
                set_items(if hits.is_empty() {
                    vec![Item::Info("No matching files".into())]
                } else {
                    hits.into_iter().map(Item::File).collect()
                });
            });
        });
    });
    DEBOUNCE.with(|d| *d.borrow_mut() = Some(id));
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn selected() -> Option<Item> {
    let idx = with_wg(|wg| wg.listbox.selected_row().map(|r| r.index()))??;
    ITEMS.with(|i| i.borrow().get(idx as usize).cloned())
}

fn hide() {
    with_wg(|wg| wg.window.set_visible(false));
}

fn flash(msg: &str) {
    with_wg(|wg| wg.status.set_text(msg));
}

fn copy_text(text: &str) {
    let mut cmd = Command::new("wl-copy");
    cmd.arg("--").arg(text);
    common::cli::spawn_reaped(cmd);
}

/// Type text into the previously focused window (after we hide). wtype uses
/// the virtual-keyboard protocol: no clipboard, works in terminals.
fn type_text(text: String) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        match Command::new("wtype").arg("-").stdin(Stdio::piped()).spawn() {
            Ok(mut c) => {
                if let Some(mut si) = c.stdin.take() {
                    let _ = si.write_all(text.as_bytes());
                }
                let _ = c.wait();
            }
            Err(e) => eprintln!("wtype: {e} (install wtype)"),
        }
    });
}

fn snippet_key() -> Option<[u8; 32]> {
    SNIPPET_KEY.with(|k| {
        if k.borrow().is_none() {
            *k.borrow_mut() = snippets::key_from_keyring();
        }
        *k.borrow()
    })
}

/// Text/code/config/log/markdown -> `<terminal> -e <editor> <file>`,
/// anything else -> the default app (xdg-open).
fn open_file(hit: &FileHit, cfg: &Config) {
    files::record_open(hit);
    let p = hit.path.to_string_lossy();
    if is_text(&hit.path) && !cfg.files.editor.is_empty() {
        spawn_detached(&cfg.terminal, &["-e", cfg.files.editor.as_str(), p.as_ref()]);
    } else {
        spawn_detached("xdg-open", &[p.as_ref()]);
    }
}

/// Super+Enter: folder (or a file's folder) in the file manager
fn open_in_file_manager(hit: &FileHit, cfg: &Config) {
    let dir = if hit.is_dir {
        hit.path.clone()
    } else {
        hit.path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    files::record_open(hit);
    let d = dir.to_string_lossy();
    spawn_detached(&cfg.files.file_manager, &[d.as_ref()]);
}

/// Enter (`alt` = Super+Enter)
fn activate_item(item: Item, alt: bool) {
    let cfg = cfg();
    match item {
        Item::App(e) => {
            launch_app(&e, &cfg.terminal);
            hide();
        }
        Item::Calc { result, .. } => {
            copy_text(&result);
            hide();
        }
        Item::File(hit) => {
            if alt {
                open_in_file_manager(&hit, &cfg);
                hide();
            } else if hit.is_dir {
                // Enter on a folder steps into it (stay in the launcher)
                let target = browse_text(&hit.path);
                with_wg(|wg| {
                    wg.search.set_text(&target);
                    wg.search.set_position(-1);
                });
            } else {
                open_file(&hit, &cfg);
                hide();
            }
        }
        Item::Snippet(s) => match s.content(snippet_key().as_ref()) {
            Ok(text) => {
                hide();
                type_text(text);
            }
            Err(e) => flash(&e),
        },
        Item::NewSnippet(s) => {
            let key = if s.encrypted {
                match snippet_key() {
                    Some(k) => Some(k),
                    None => {
                        flash("No key yet: run `launch-gui --snippet-key` in a terminal");
                        return;
                    }
                }
            } else {
                None
            };
            match snippets::save(&cfg.snippets, &s.name, s.trigger.as_deref(), &s.body, key.as_ref()) {
                Ok(()) => {
                    with_wg(|wg| wg.search.set_text(&cfg.snippets.prefix));
                    flash(&format!("Saved \"{}\"", s.name));
                }
                Err(e) => flash(&e),
            }
        }
        Item::Info(_) => {}
    }
}

fn activate_selected(alt: bool) {
    if let Some(item) = selected() {
        activate_item(item, alt);
    }
}

/// Tab: browse. Folder -> into it, file -> its folder, otherwise ~/
fn browse_from_selection() {
    let target = match selected() {
        Some(Item::File(h)) if h.is_dir => browse_text(&h.path),
        Some(Item::File(h)) => h.path.parent().map(browse_text).unwrap_or_else(|| "~/".into()),
        _ => "~/".into(),
    };
    with_wg(|wg| {
        wg.search.set_text(&target);
        wg.search.set_position(-1);
    });
}

/// ← with an empty filter in browse mode: parent folder. Returns handled.
fn browse_up() -> bool {
    let q = with_wg(|wg| wg.search.text().to_string()).unwrap_or_default();
    let Mode::Browse { dir, filter } = mode_of(&q, &cfg()) else { return false };
    if !filter.is_empty() {
        return false;
    }
    let Some(parent) = dir.parent() else { return false };
    let target = browse_text(parent);
    with_wg(|wg| {
        wg.search.set_text(&target);
        wg.search.set_position(-1);
    });
    true
}

fn copy_selected_path() {
    if let Some(Item::File(h)) = selected() {
        copy_text(&h.path.to_string_lossy());
        flash("Path copied");
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

fn navigate(nav: Nav) {
    with_wg(|wg| {
        let n = wg.listbox.observe_children().n_items() as i32;
        if n == 0 {
            return;
        }
        let cur = wg.listbox.selected_row().map_or(0, |r| r.index());
        let target = match nav {
            Nav::By(d) => cur + d,
            Nav::First => 0,
            Nav::Last => n - 1,
        }
        .clamp(0, n - 1);
        if let Some(r) = wg.listbox.row_at_index(target) {
            wg.listbox.select_row(Some(&r));
            scroll_to_selected(&wg.listbox, &wg.scroll);
        }
    });
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// Keys that mean the same thing in every mode
fn on_key_global(key: gdk4::Key, mods: gdk4::ModifierType) -> Option<glib::Propagation> {
    let ctrl = mods.contains(gdk4::ModifierType::CONTROL_MASK);
    let shift = mods.contains(gdk4::ModifierType::SHIFT_MASK);
    let sup = mods.intersects(gdk4::ModifierType::SUPER_MASK | gdk4::ModifierType::META_MASK);
    let enter = key == gdk4::Key::Return || key == gdk4::Key::KP_Enter;

    if key == gdk4::Key::Tab && !ctrl {
        browse_from_selection();
        return Some(glib::Propagation::Stop);
    }
    if enter && sup {
        activate_selected(true);
        return Some(glib::Propagation::Stop);
    }
    if ctrl && shift && (key == gdk4::Key::C || key == gdk4::Key::c) {
        copy_selected_path();
        return Some(glib::Propagation::Stop);
    }
    if key == gdk4::Key::Left && browse_up() {
        return Some(glib::Propagation::Stop);
    }
    None
}

fn on_key(key: gdk4::Key, mods: gdk4::ModifierType) -> glib::Propagation {
    let cfg = cfg();
    let vim_insert = cfg.vim_mode && get_vim_mode() == VimMode::Insert;
    if !(cfg.vim_mode && !vim_insert && key == gdk4::Key::Escape) {
        if let Some(p) = on_key_global(key, mods) {
            return p;
        }
    }
    if cfg.vim_mode {
        on_key_vim(key, mods)
    } else {
        on_key_normal(&cfg, key, mods)
    }
}

fn on_key_normal(cfg: &Config, key: gdk4::Key, mods: gdk4::ModifierType) -> glib::Propagation {
    let Some(action) = match_action(&cfg.base.keybinds, key, mods) else {
        return glib::Propagation::Proceed;
    };
    match action {
        Action::Close => hide(),
        Action::Select => activate_selected(false),
        Action::ClearSearch => {
            with_wg(|wg| wg.search.set_text(""));
        }
        Action::Next => navigate(Nav::By(1)),
        Action::Prev => navigate(Nav::By(-1)),
        Action::PageDown => navigate(Nav::By(PAGE)),
        Action::PageUp => navigate(Nav::By(-PAGE)),
        Action::First => navigate(Nav::First),
        Action::Last => navigate(Nav::Last),
        // not used by the launcher
        Action::Delete | Action::Pin | Action::Unpin | Action::TabSwitch => {
            return glib::Propagation::Proceed
        }
    }
    glib::Propagation::Stop
}

fn on_key_vim(key: gdk4::Key, mods: gdk4::ModifierType) -> glib::Propagation {
    match get_vim_mode() {
        VimMode::Normal => {
            if let Some(action) = handle_vim_normal_key(key, mods, false, false) {
                match action {
                    VimAction::Close => hide(),
                    VimAction::Select => activate_selected(false),
                    VimAction::EnterInsert => {
                        set_vim_mode(VimMode::Insert);
                        with_wg(|wg| {
                            update_mode_display(&wg.mode_label, VimMode::Insert);
                            wg.search.grab_focus();
                            wg.search.set_position(-1);
                        });
                    }
                    VimAction::Down => navigate(Nav::By(1)),
                    VimAction::Up => navigate(Nav::By(-1)),
                    VimAction::Top => navigate(Nav::First),
                    VimAction::Bottom => navigate(Nav::Last),
                    VimAction::HalfPageDown => navigate(Nav::By(PAGE)),
                    VimAction::HalfPageUp => navigate(Nav::By(-PAGE)),
                    _ => {}
                }
            }
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
            match key {
                gdk4::Key::Return | gdk4::Key::KP_Enter => {
                    activate_selected(false);
                    glib::Propagation::Stop
                }
                gdk4::Key::Down => {
                    navigate(Nav::By(1));
                    glib::Propagation::Stop
                }
                gdk4::Key::Up => {
                    navigate(Nav::By(-1));
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Expand / collapse animation
// ---------------------------------------------------------------------------

/// Spring easing: overshoots slightly then settles. t in [0,1].
fn spring(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (-6.0 * t).exp() * ((1.0 - t * t).sqrt().max(0.0) * std::f64::consts::PI * 3.5).cos()
}

fn ease_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Frame-clock driven height animation; a generation counter cancels stale ones.
fn animate_height(from: i32, to: i32, duration_ms: u64, expanding: bool) {
    let gen = ANIM_GEN.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    let Some((container, scroll, section, status_bar, window)) = with_wg(|wg| {
        (
            wg.container.clone(),
            wg.scroll.clone(),
            wg.section_label.clone(),
            wg.status_bar.clone(),
            wg.window.clone(),
        )
    }) else {
        return;
    };

    if expanding {
        container.remove_css_class("collapsed");
        container.add_css_class("expanded");
        scroll.set_visible(true);
        section.set_visible(true);
        status_bar.set_visible(true);
    } else {
        container.remove_css_class("expanded");
        container.add_css_class("collapsed");
    }

    let start = Instant::now();
    let duration = Duration::from_millis(duration_ms.max(1));
    container.add_tick_callback(move |widget, _| {
        if ANIM_GEN.with(|g| g.get()) != gen {
            return glib::ControlFlow::Break;
        }
        let t = (start.elapsed().as_secs_f64() / duration.as_secs_f64()).min(1.0);
        let eased = if expanding { spring(t) } else { ease_out(t) };
        let h = (from as f64 + (to - from) as f64 * eased).round() as i32;
        let w = if widget.allocated_width() > 1 { widget.allocated_width() } else { widget.width_request() };
        widget.set_size_request(w, h);
        window.set_default_size(w, h);
        if t >= 1.0 {
            widget.set_size_request(w, to);
            window.set_default_size(w, to);
            if !expanding {
                scroll.set_visible(false);
                section.set_visible(false);
                status_bar.set_visible(false);
            }
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

fn expand(cfg: &Config) {
    EXPANDED.with(|e| e.set(true));
    animate_height(cfg.search_height, cfg.base.height, cfg.animation_duration, true);
}

fn collapse(cfg: &Config) {
    EXPANDED.with(|e| e.set(false));
    animate_height(cfg.base.height, cfg.search_height, cfg.animation_duration, false);
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

fn toggle_window() {
    match with_wg(|wg| wg.window.is_visible()) {
        Some(true) => hide(),
        Some(false) => show_window(),
        None => {}
    }
}

fn show_window() {
    let cfg = Config::load();
    CONFIG.with(|c| *c.borrow_mut() = cfg.clone());
    if cfg.vim_mode {
        set_vim_mode(VimMode::Normal);
    }
    files::ensure_fresh(&cfg.files);
    SNIPPET_KEY.with(|k| *k.borrow_mut() = None); // re-read per session (key may change)

    EXPANDED.with(|e| e.set(false));
    ANIM_GEN.with(|g| g.set(g.get() + 1)); // cancel any running animation

    with_wg(|wg| {
        if needs_reload() {
            *wg.entries.borrow_mut() = load_entries();
        }
        if cfg.base.anchor == Anchor::Cursor {
            update_cursor_position(wg.window.upcast_ref());
        }
        wg.search.set_text("");
        wg.container.remove_css_class("expanded");
        wg.container.add_css_class("collapsed");
        wg.container.set_size_request(cfg.base.width, cfg.search_height);
        wg.window.set_default_size(cfg.base.width, cfg.search_height);
        wg.scroll.set_visible(false);
        wg.section_label.set_visible(false);
        wg.status_bar.set_visible(false);
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
}

fn build_window(app: &Application, cfg: &Config) {
    apply_css(&css_for(cfg));

    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(cfg.base.width)
        .default_height(cfg.search_height)
        .resizable(false)
        .build();
    apply_layer_shell(&window, &cfg.base, APP_NAME);
    window.set_default_size(cfg.base.width, cfg.search_height);

    let container = GtkBox::new(Orientation::Vertical, 0);
    container.add_css_class("launch-container");
    container.add_css_class("collapsed");
    container.set_size_request(cfg.base.width, cfg.search_height);

    let search_wrapper = GtkBox::new(Orientation::Vertical, 0);
    search_wrapper.add_css_class("launch-search-wrapper");
    let search_row = GtkBox::new(Orientation::Horizontal, 8);
    search_row.add_css_class("launch-search-row");
    let search = Entry::new();
    search.set_placeholder_text(Some("Search apps · f' files · Tab browse"));
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

    let section_label = Label::new(Some("Applications"));
    section_label.set_xalign(0.0);
    section_label.set_ellipsize(gtk4::pango::EllipsizeMode::Start);
    section_label.add_css_class("launch-section-label");
    section_label.set_visible(false);
    container.append(&section_label);

    // body: results | preview (preview only in file/browse modes)
    let body = GtkBox::new(Orientation::Horizontal, 0);
    body.set_vexpand(true);
    let scroll = ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);
    scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);
    scroll.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
    scroll.set_visible(false);
    let listbox = ListBox::new();
    listbox.add_css_class("launch-list");
    listbox.set_selection_mode(gtk4::SelectionMode::Single);
    scroll.set_child(Some(&listbox));
    body.append(&scroll);

    let preview_scroll = ScrolledWindow::new();
    preview_scroll.set_vexpand(true);
    preview_scroll.set_size_request(cfg.base.width / 2, -1);
    preview_scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);
    preview_scroll.add_css_class("launch-preview");
    preview_scroll.set_visible(false);
    let preview_box = GtkBox::new(Orientation::Vertical, 2);
    preview_scroll.set_child(Some(&preview_box));
    body.append(&preview_scroll);
    container.append(&body);

    let status_bar = GtkBox::new(Orientation::Horizontal, 0);
    status_bar.add_css_class("launch-status-bar");
    status_bar.set_visible(false);
    let mode_label = Label::new(Some(""));
    mode_label.add_css_class("vim-mode-indicator");
    mode_label.set_halign(Align::Start);
    mode_label.set_visible(false);
    status_bar.append(&mode_label);
    let status = Label::new(Some(""));
    status.add_css_class("launch-status-left");
    status.set_halign(Align::Start);
    status.set_hexpand(true);
    status_bar.append(&status);
    let hints = GtkBox::new(Orientation::Horizontal, 12);
    hints.set_halign(Align::End);
    status_bar.append(&hints);
    container.append(&status_bar);
    window.set_child(Some(&container));

    search.connect_changed(|s| on_query_changed(s.text().to_string()));
    listbox.connect_row_selected(|_, row| update_preview(row.map(|r| r.index())));
    listbox.connect_row_activated(|_, row| {
        let item = ITEMS.with(|i| i.borrow().get(row.index() as usize).cloned());
        if let Some(item) = item {
            activate_item(item, false);
        }
    });

    let key_ctrl = EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
    key_ctrl.connect_key_pressed(|_, key, _, mods| on_key(key, mods));
    window.add_controller(key_ctrl);

    WIDGETS.with(|w| {
        *w.borrow_mut() = Some(AppWidgets {
            window,
            container,
            search,
            listbox,
            scroll,
            section_label,
            status_bar,
            status,
            hints,
            mode_label,
            preview_scroll,
            preview_box,
            entries: Rc::new(RefCell::new(load_entries())),
        });
    });
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
    files::init(&cfg.files); // cached index now, full rescan if first start since boot
    show_window();
    log(
        APP_NAME,
        &format!(
            "daemon started ({}x{}, files={}, snippets={})",
            cfg.base.width, cfg.base.height, cfg.files.enabled, cfg.snippets.enabled
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
