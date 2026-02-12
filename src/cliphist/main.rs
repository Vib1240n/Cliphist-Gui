use gdk4::prelude::*;
use gio;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, CssProvider, Entry,
    EventControllerKey, Label, ListBox, ListBoxRow, Orientation, Picture, ScrolledWindow,
};
use gtk4_layer_shell::Edge;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

use common::{
    Action, Anchor, ConfigBase, KeyCombo,
    config::{parse_bool, parse_config_file},
    keys::{default_keybinds, match_action},
    layer::{apply_layer_shell, get_cursor_position, update_cursor_position},
    logging::log,
    paths::{cache_dir, config_dir},
    css::{load_css, char_truncate},
};

const APP_NAME: &str = "cliphist-gui";
const THUMB_SIZE: u32 = 64;
const MAX_TEXT_PREVIEW: usize = 120;
const MAX_SUB_PREVIEW: usize = 60;

fn default_config() -> &'static str { include_str!("config.default") }
fn default_css() -> &'static str { include_str!("style.css") }

#[derive(Clone, Debug)]
struct Config {
    base: ConfigBase,
    max_items: usize,
    close_on_select: bool,
    notify_on_copy: bool,
}

impl Config {
    fn default() -> Self {
        Self {
            base: ConfigBase::new(APP_NAME, 580, 520),
            max_items: 0,
            close_on_select: true,
            notify_on_copy: false,
        }
    }

    fn load() -> Self {
        let path = config_dir(APP_NAME).join("config");
        if !path.exists() { return Self::default(); }
        
        match std::fs::read_to_string(&path) {
            Ok(c) => {
                log(APP_NAME, &format!("loaded config from {}", path.display()));
                Self::parse(&c)
            }
            Err(e) => {
                log(APP_NAME, &format!("config read error: {}", e));
                Self::default()
            }
        }
    }

    fn parse(content: &str) -> Self {
        let mut cfg = Self::default();
        for (section, key, val) in parse_config_file(content) {
            cfg.base.parse_section(APP_NAME, &section, &key, &val);
            if section == "behavior" {
                match key.as_str() {
                    "max_items" => cfg.max_items = val.parse().unwrap_or(0),
                    "close_on_select" => cfg.close_on_select = parse_bool(&val, true),
                    "notify_on_copy" => cfg.notify_on_copy = parse_bool(&val, false),
                    _ => {}
                }
            }
        }
        cfg
    }
}

#[derive(Clone, Debug)]
struct ClipEntry {
    raw_line: String,
    #[allow(dead_code)] id: String,
    preview: String,
    is_image: bool,
    thumb_path: Option<PathBuf>,
}

struct AppWidgets {
    search: Entry,
    listbox: ListBox,
    status: Label,
    entries: Rc<RefCell<Vec<ClipEntry>>>,
}

thread_local! {
    static WIDGETS: RefCell<Option<AppWidgets>> = RefCell::new(None);
    static CONFIG: RefCell<Config> = RefCell::new(Config::default());
}

fn thumb_cache() -> PathBuf {
    let d = cache_dir(APP_NAME).join("thumbs");
    std::fs::create_dir_all(&d).ok();
    d
}

fn fetch_entries(max_items: usize) -> Vec<ClipEntry> {
    let output = match Command::new("cliphist").arg("list").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let cache = thumb_cache();

    let iter = stdout.lines().filter(|l| !l.is_empty());
    let iter: Box<dyn Iterator<Item = &str>> = if max_items > 0 {
        Box::new(iter.take(max_items))
    } else { Box::new(iter) };

    iter.map(|line| {
        let raw_line = line.to_string();
        let (id, preview) = match line.split_once('\t') {
            Some((i, p)) => (i.trim().to_string(), p.to_string()),
            None => (line.to_string(), line.to_string()),
        };
        let is_image = preview.contains("[[ binary data");
        let thumb_path = if is_image {
            let path = cache.join(format!("{}.png", id));
            if !path.exists() { generate_thumbnail(&raw_line, &path); }
            if path.exists() { Some(path) } else { None }
        } else { None };
        ClipEntry { raw_line, id, preview, is_image, thumb_path }
    }).collect()
}

fn generate_thumbnail(raw_line: &str, out_path: &PathBuf) {
    let mut child = match Command::new("cliphist").arg("decode")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn() { Ok(c) => c, Err(_) => return };

    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(raw_line.as_bytes());
        drop(si);
    }
    
    let out = match child.wait_with_output() { Ok(o) => o, Err(_) => return };
    if !out.status.success() || out.stdout.is_empty() { return; }

    let mut m = match Command::new("magick")
        .args(["png:-", "-resize", &format!("{}x{}>", THUMB_SIZE * 2, THUMB_SIZE * 2),
               &format!("png:{}", out_path.display())])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn() { Ok(c) => c, Err(_) => return };

    if let Some(mut si) = m.stdin.take() {
        let _ = si.write_all(&out.stdout);
        drop(si);
    }
    let _ = m.wait();
}

fn select_entry(entry: &ClipEntry, notify: bool) {
    let mut dec = Command::new("cliphist").arg("decode")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn().expect("cliphist decode failed");

    if let Some(mut si) = dec.stdin.take() {
        let _ = si.write_all(entry.raw_line.as_bytes());
        drop(si);
    }
    
    if let Ok(out) = dec.wait_with_output() {
        if out.status.success() {
            let mime = if entry.is_image { "image/png" } else { "text/plain" };
            let mut wl = Command::new("wl-copy").args(["--type", mime])
                .stdin(std::process::Stdio::piped()).spawn().expect("wl-copy failed");
            if let Some(mut si) = wl.stdin.take() {
                let _ = si.write_all(&out.stdout);
                drop(si);
            }
            let _ = wl.wait();

            if notify {
                let msg = if entry.is_image { "Image copied".to_string() }
                else { format!("Copied: {}", char_truncate(&entry.preview, 50)) };
                let _ = Command::new("notify-send").args(["-t", "2000", APP_NAME, &msg]).spawn();
            }
        }
    }
}

fn delete_entry(entry: &ClipEntry) {
    if let Ok(mut c) = Command::new("cliphist").arg("delete")
        .stdin(std::process::Stdio::piped()).spawn()
    {
        if let Some(mut si) = c.stdin.take() {
            let _ = si.write_all(entry.raw_line.as_bytes());
            drop(si);
        }
        let _ = c.wait();
    }
    if let Some(ref p) = entry.thumb_path { let _ = std::fs::remove_file(p); }
}

fn content_type(e: &ClipEntry) -> &'static str {
    if e.is_image { return "IMAGE"; }
    let p = e.preview.trim();
    if p.starts_with("http://") || p.starts_with("https://") { "URL" } else { "TEXT" }
}

fn parse_image_meta(preview: &str) -> Option<String> {
    let inner = preview.trim_start_matches("[[ binary data").trim_end_matches("]]").trim();
    let parts: Vec<&str> = inner.split_whitespace().collect();
    let mut dims = None;
    let mut fmt = None;

    for p in &parts {
        if p.contains('x') && p.chars().all(|c| c.is_ascii_digit() || c == 'x') {
            dims = Some(p.to_string());
        }
        if ["png", "jpg", "jpeg", "gif", "bmp", "webp"].contains(&p.to_lowercase().as_str()) {
            fmt = Some(p.to_uppercase());
        }
    }

    match (dims, fmt) {
        (Some(d), Some(f)) => Some(format!("{} -- {}", d, f)),
        (Some(d), None) => Some(d),
        (None, Some(f)) => Some(f),
        _ => None,
    }
}

fn get_filtered_entry(entries: &[ClipEntry], query: &str, idx: usize) -> Option<ClipEntry> {
    let q = query.to_lowercase();
    let filtered: Vec<&ClipEntry> = if q.is_empty() {
        entries.iter().collect()
    } else {
        entries.iter().filter(|e| e.preview.to_lowercase().contains(&q)).collect()
    };
    filtered.get(idx).map(|e| (*e).clone())
}

fn build_row(entry: &ClipEntry) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_focusable(false);
    let hbox = GtkBox::new(Orientation::Horizontal, 14);
    hbox.set_valign(Align::Center);

    if let Some(ref path) = entry.thumb_path {
        let pic = Picture::for_filename(path.to_str().unwrap_or(""));
        pic.set_size_request(48, 48);
        pic.add_css_class("clip-thumb");
        let frame = gtk4::Frame::new(None);
        frame.set_child(Some(&pic));
        frame.add_css_class("clip-thumb-frame");
        frame.set_size_request(48, 48);
        hbox.append(&frame);
    } else {
        let ib = GtkBox::new(Orientation::Vertical, 0);
        ib.set_size_request(48, 48);
        ib.set_valign(Align::Center);
        ib.set_halign(Align::Center);
        ib.add_css_class("clip-text-icon");
        let lbl = Label::new(Some("T"));
        lbl.add_css_class("clip-text-icon-label");
        lbl.set_valign(Align::Center);
        lbl.set_halign(Align::Center);
        lbl.set_vexpand(true);
        ib.append(&lbl);
        hbox.append(&ib);
    }

    let content = GtkBox::new(Orientation::Vertical, 0);
    content.set_hexpand(true);
    content.set_valign(Align::Center);

    let ctype = content_type(entry);
    let title_text = if entry.is_image { "Image".to_string() }
    else { char_truncate(&entry.preview, MAX_TEXT_PREVIEW) };

    let title = Label::new(Some(&title_text));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_max_width_chars(45);
    title.add_css_class("clip-title");
    content.append(&title);

    let sub_text = if entry.is_image {
        parse_image_meta(&entry.preview).unwrap_or_default()
    } else { char_truncate(&entry.preview, MAX_SUB_PREVIEW) };

    if !sub_text.is_empty() {
        let sub = Label::new(Some(&sub_text));
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        sub.set_max_width_chars(45);
        sub.add_css_class("clip-subtitle");
        content.append(&sub);
    }

    hbox.append(&content);

    let right = GtkBox::new(Orientation::Vertical, 2);
    right.set_valign(Align::Start);
    right.set_halign(Align::End);
    right.set_margin_top(2);
    let badge = Label::new(Some(ctype));
    badge.set_halign(Align::End);
    badge.add_css_class("clip-badge");
    right.append(&badge);
    hbox.append(&right);

    row.set_child(Some(&hbox));
    row
}

fn populate_list(listbox: &ListBox, entries: &[ClipEntry], query: &str) -> usize {
    while let Some(row) = listbox.row_at_index(0) { listbox.remove(&row); }
    let q = query.to_lowercase();
    let mut count = 0;
    for e in entries {
        if q.is_empty() || e.preview.to_lowercase().contains(&q) {
            listbox.append(&build_row(e));
            count += 1;
        }
    }
    if let Some(first) = listbox.row_at_index(0) {
        listbox.select_row(Some(&first));
    }
    count
}

fn activate(app: &Application) {
    let cfg = Config::load();
    CONFIG.with(|c| *c.borrow_mut() = cfg.clone());

    if let Some(win) = app.active_window() {
        if win.is_visible() {
            win.set_visible(false);
        } else {
            if cfg.base.anchor == Anchor::Cursor { update_cursor_position(&win); }
            WIDGETS.with(|w| {
                if let Some(ref wg) = *w.borrow() {
                    let mut ents = wg.entries.borrow_mut();
                    *ents = fetch_entries(cfg.max_items);
                    let n = populate_list(&wg.listbox, &ents, "");
                    wg.status.set_text(&format!("{} items", n));
                    wg.search.set_text("");
                    wg.search.grab_focus();
                }
            });
            win.set_visible(true);
            win.present();
        }
        return;
    }

    let css_content = if let Ok(theme) = std::env::var("GUI_THEME_OVERRIDE") {
    common::paths::get_theme_css(&theme).unwrap_or_else(|| load_css(APP_NAME, &cfg.base.theme, default_css()))
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
    let status_bar = GtkBox::new(Orientation::Horizontal, 0);
    status_bar.add_css_class("clip-status-bar");
    let status = Label::new(Some("0 items"));
    status.add_css_class("clip-status-left");
    status.set_halign(Align::Start);
    status.set_hexpand(true);
    status_bar.append(&status);

    let hints = GtkBox::new(Orientation::Horizontal, 12);
    hints.set_halign(Align::End);
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
    status_bar.append(&hints);
    container.append(&status_bar);
    window.set_child(Some(&container));

    let entries_f = entries.clone();
    let listbox_f = listbox.clone();
    let status_f = status.clone();
    search.connect_changed(move |s| {
        let q = s.text().to_string();
        let ents = entries_f.borrow();
        let n = populate_list(&listbox_f, &ents, &q);
        status_f.set_text(&format!("{} items", n));
    });

    let key_ctrl = EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let ek = entries.clone();
    let lk = listbox.clone();
    let wk = window.clone();
    let sk = search.clone();
    let stk = status.clone();

    key_ctrl.connect_key_pressed(move |_, key, _, mods| {
    let action = CONFIG.with(|c| match_action(&c.borrow().base.keybinds, key, mods));
    let close = CONFIG.with(|c| c.borrow().close_on_select);
    let notify = CONFIG.with(|c| c.borrow().notify_on_copy);
    let max = CONFIG.with(|c| c.borrow().max_items);

    if let Some(action) = action {
        match action {
            Action::Close => { wk.set_visible(false); }
            Action::Select => {
                if let Some(row) = lk.selected_row() {
                    let ents = ek.borrow();
                    if let Some(e) = get_filtered_entry(&ents, &sk.text(), row.index() as usize) {
                        select_entry(&e, notify);
                        if close { wk.set_visible(false); }
                    }
                }
            }
            Action::Delete => {
                if let Some(row) = lk.selected_row() {
                    let ents = ek.borrow();
                    if let Some(e) = get_filtered_entry(&ents, &sk.text(), row.index() as usize) {
                        delete_entry(&e);
                    }
                    drop(ents);
                    let mut ents = ek.borrow_mut();
                    *ents = fetch_entries(max);
                    let n = populate_list(&lk, &ents, &sk.text());
                    stk.set_text(&format!("{} items", n));
                }
            }
            Action::ClearSearch => { sk.set_text(""); }
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
                    let t = (r.index() + 10).min(lk.observe_children().n_items() as i32 - 1);
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
    glib::Propagation::Proceed
});
    window.add_controller(key_ctrl);

    let ec = entries.clone();
    let wc = window.clone();
    let sc = search.clone();
    let cfg_c = cfg.clone();
    listbox.connect_row_activated(move |_, row| {
        let ents = ec.borrow();
        if let Some(e) = get_filtered_entry(&ents, &sc.text(), row.index() as usize) {
            select_entry(&e, cfg_c.notify_on_copy);
            if cfg_c.close_on_select { wc.set_visible(false); }
        }
    });

    WIDGETS.with(|w| {
        *w.borrow_mut() = Some(AppWidgets {
            search: search.clone(), listbox: listbox.clone(),
            status: status.clone(), entries: entries.clone(),
        });
    });

    {
        let mut ents = entries.borrow_mut();
        *ents = fetch_entries(cfg.max_items);
        let n = populate_list(&listbox, &ents, "");
        status.set_text(&format!("{} items", n));
    }

    window.present();
    search.grab_focus();
    log(APP_NAME, &format!("daemon started ({}x{}, anchor={:?})", cfg.base.width, cfg.base.height, cfg.base.anchor));
}

fn get_pid(pidfile: &str) -> Option<i32> {
    std::fs::read_to_string(pidfile).ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .filter(|&pid| unsafe { libc::kill(pid, 0) } == 0)
}

fn print_usage() {
    eprintln!("{} - {}\n", APP_NAME, "Clipboard History Manager"); // or "clipboard manager"
    eprintln!("Usage:");
    eprintln!("  {}                      Start daemon", APP_NAME);
    eprintln!("  {} toggle               Toggle window", APP_NAME);
    eprintln!("  {} --theme <name>       Preview theme", APP_NAME);
    eprintln!("  {} show-themes          List themes", APP_NAME);
    eprintln!("  {} --config             Show config dir", APP_NAME);
    eprintln!("  {} --generate-config    Create defaults", APP_NAME);
    eprintln!("  {} --reload             Restart daemon", APP_NAME);
    eprintln!("  {} --help               Show help", APP_NAME);
}

fn cmd_config() {
    let dir = config_dir(APP_NAME);
    if dir.exists() {
        println!("{}", dir.display());
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                println!("  {}", e.file_name().to_string_lossy());
            }
        }
    } else {
        println!("Config directory does not exist: {}", dir.display());
        println!("Run 'cliphist-gui --generate-config' to create it.");
    }
}

fn cmd_generate_config() {
    let dir = config_dir(APP_NAME);
    std::fs::create_dir_all(&dir).expect("failed to create config dir");
    for (name, content) in [("style.css", default_css()), ("config", default_config())] {
        let p = dir.join(name);
        if p.exists() {
            println!("{} already exists at {}", name, p.display());
        } else {
            let _ = std::fs::write(&p, content);
            println!("Created {}", p.display());
        }
    }
    println!("Config directory: {}", dir.display());
}

fn cmd_reload(pidfile: &str) {
    let exe = std::env::current_exe().expect("cannot find self");
    if let Some(pid) = get_pid(pidfile) {
        unsafe { libc::kill(pid, libc::SIGTERM) };
        for _ in 0..20 {
            if unsafe { libc::kill(pid, 0) } != 0 { break; }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = std::fs::remove_file(pidfile);
    }
    let _ = Command::new(&exe)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    println!("cliphist-gui reloaded");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pidfile = format!("/tmp/{}-{}.pid", APP_NAME, unsafe { libc::getuid() });

    if args.len() > 1 {
    match args[1].as_str() {
        "--help" | "-h" => { print_usage(); return; }
        "--config" => { cmd_config(); return; }
        "--generate-config" => { cmd_generate_config(); return; }
        "--reload" => { cmd_reload(&pidfile); return; }
        "toggle" => {
            if let Some(pid) = get_pid(&pidfile) {
                unsafe { libc::kill(pid, libc::SIGUSR1) };
            } else {
                eprintln!("Daemon not running");
            }
            return;
        }
        "open" => {
            if let Some(pid) = get_pid(&pidfile) {
                unsafe { libc::kill(pid, libc::SIGUSR1) };
            } else {
                eprintln!("Daemon not running");
            }
            return;
        }
        "close" => {
            if let Some(pid) = get_pid(&pidfile) {
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
            return;
        }
        "show-themes" | "--themes" => {
    println!("Available themes:");
    for (name, _) in common::paths::builtin_themes() {
        println!("  {}", name);
    }
    return;
}
"-T" | "--theme" => {
    if args.len() < 3 {
        eprintln!("Usage: {} --theme <name>", APP_NAME);
        return;
    }
    let theme = &args[2];
    if common::paths::get_theme_css(theme).is_none() {
        eprintln!("Unknown theme: {}", theme);
        return;
    }
    // Kill existing
    if let Some(pid) = get_pid(&pidfile) {
        unsafe { libc::kill(pid, libc::SIGTERM) };
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = std::fs::remove_file(&pidfile);
    }
    // Start new daemon with theme
    let exe = std::env::current_exe().expect("cannot find self");
    let _ = Command::new(&exe)
        .env("GUI_THEME_OVERRIDE", theme)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    println!("Started with theme: {}", theme);
    return;
}        other => {
            eprintln!("Unknown option: {}", other);
            print_usage();
            std::process::exit(1);
        }
    }
} 
    if let Some(pid) = get_pid(&pidfile) {
        unsafe { libc::kill(pid, libc::SIGUSR1) };
        return;
    }

    let _ = std::fs::write(&pidfile, std::process::id().to_string());

    let app = Application::builder()
        .application_id("com.vib1240n.cliphist-gui")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(|app| {
        activate(app);

        glib::unix_signal_add_local(libc::SIGUSR1, {
            let app = app.clone();
            move || {
                let cfg = Config::load();
                CONFIG.with(|c| *c.borrow_mut() = cfg.clone());

                if let Some(win) = app.active_window() {
                    if win.is_visible() {
                        win.set_visible(false);
                    } else {
                        if cfg.base.anchor == Anchor::Cursor { update_cursor_position(&win); }
                        WIDGETS.with(|w| {
                            if let Some(ref wg) = *w.borrow() {
                                let mut ents = wg.entries.borrow_mut();
                                *ents = fetch_entries(cfg.max_items);
                                let n = populate_list(&wg.listbox, &ents, "");
                                wg.status.set_text(&format!("{} items", n));
                                wg.search.set_text("");
                                wg.search.grab_focus();
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
    });

    app.run_with_args::<String>(&[]);
    let _ = std::fs::remove_file(&pidfile);
}
