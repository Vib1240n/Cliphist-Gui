use gdk4::prelude::*;
use gio;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, CssProvider, Entry,
    EventControllerKey, Image, Label, ListBox, ListBoxRow, Orientation, ScrolledWindow,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::io::Write;

use common::{
    Action, Anchor, ConfigBase,
    config::{parse_bool, parse_config_file},
    keys::match_action,
    layer::{apply_layer_shell, update_cursor_position},
    logging::log,
    paths::config_dir,
    css::{load_css, char_truncate},
};

const APP_NAME: &str = "launch-gui";

fn default_config() -> &'static str { include_str!("config.default") }
fn default_css() -> &'static str { include_str!("style.css") }

#[derive(Clone, Debug)]
struct Config {
    base: ConfigBase,
    terminal: String,
    calculator: bool,
}

impl Config {
    fn default() -> Self {
        Self {
            base: ConfigBase::new(APP_NAME, 580, 400),
            terminal: "kitty".to_string(),
            calculator: true,
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
                    "terminal" => cfg.terminal = val,
                    "calculator" => cfg.calculator = parse_bool(&val, true),
                    _ => {}
                }
            }
        }
        cfg
    }
}

#[derive(Clone, Debug)]
struct DesktopEntry {
    name: String,
    exec: String,
    icon: String,
    description: String,
    terminal: bool,
    path: PathBuf,
    score: i32,
}

struct AppWidgets {
    search: Entry,
    listbox: ListBox,
    status: Label,
    entries: Rc<RefCell<Vec<DesktopEntry>>>,
}

thread_local! {
    static WIDGETS: RefCell<Option<AppWidgets>> = RefCell::new(None);
    static CONFIG: RefCell<Config> = RefCell::new(Config::default());
    static FREQUENCY: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
}

fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("applications"));
    }
    
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or("/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        dirs.push(PathBuf::from(dir).join("applications"));
    }
    
    dirs
}

fn parse_desktop_file(path: &PathBuf) -> Option<DesktopEntry> {
    let content = std::fs::read_to_string(path).ok()?;
    
    let mut name = String::new();
    let mut exec = String::new();
    let mut icon = String::new();
    let mut description = String::new();
    let mut terminal = false;
    let mut no_display = false;
    let mut hidden = false;
    let mut in_desktop_entry = false;

    for line in content.lines() {
        let t = line.trim();
        
        if t.starts_with('[') {
            in_desktop_entry = t == "[Desktop Entry]";
            continue;
        }
        
        if !in_desktop_entry { continue; }
        
        if let Some((k, v)) = t.split_once('=') {
            let key = k.trim();
            let val = v.trim();
            match key {
                "Name" if name.is_empty() => name = val.to_string(),
                "Exec" => exec = val.to_string(),
                "Icon" => icon = val.to_string(),
                "Comment" if description.is_empty() => description = val.to_string(),
                "GenericName" if description.is_empty() => description = val.to_string(),
                "Terminal" => terminal = val.to_lowercase() == "true",
                "NoDisplay" => no_display = val.to_lowercase() == "true",
                "Hidden" => hidden = val.to_lowercase() == "true",
                _ => {}
            }
        }
    }

    if name.is_empty() || exec.is_empty() || no_display || hidden {
        return None;
    }

    // clean exec - remove field codes
    let exec_clean = exec
        .replace("%f", "").replace("%F", "")
        .replace("%u", "").replace("%U", "")
        .replace("%c", "").replace("%k", "")
        .replace("%i", "").replace("%d", "").replace("%D", "")
        .trim().to_string();

    Some(DesktopEntry {
        name, exec: exec_clean, icon, description, terminal,
        path: path.clone(), score: 0,
    })
}

fn load_entries() -> Vec<DesktopEntry> {
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for dir in xdg_data_dirs() {
        if !dir.exists() { continue; }
        
        let walker = walkdir(dir.clone());
        for path in walker {
            if path.extension().map(|e| e == "desktop").unwrap_or(false) {
                if let Some(entry) = parse_desktop_file(&path) {
                    if seen.insert(entry.name.clone()) {
                        entries.push(entry);
                    }
                }
            }
        }
    }

    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    log(APP_NAME, &format!("loaded {} desktop entries", entries.len()));
    entries
}

fn walkdir(dir: PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                files.extend(walkdir(p));
            } else {
                files.push(p);
            }
        }
    }
    files
}

fn fuzzy_match(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() { return Some(0); }
    
    let q = query.to_lowercase();
    let t = text.to_lowercase();
    
    // exact match
    if t == q { return Some(1000); }
    // starts with
    if t.starts_with(&q) { return Some(500 + (100 - q.len() as i32)); }
    // contains
    if t.contains(&q) { return Some(200); }
    
    // fuzzy char match
    let mut qi = q.chars().peekable();
    let mut score = 0;
    let mut consecutive = 0;
    
    for c in t.chars() {
        if qi.peek() == Some(&c) {
            qi.next();
            consecutive += 1;
            score += consecutive * 10;
        } else {
            consecutive = 0;
        }
    }
    
    if qi.peek().is_none() { Some(score) } else { None }
}

fn filter_entries(entries: &[DesktopEntry], query: &str) -> Vec<DesktopEntry> {
    if query.is_empty() {
        return entries.to_vec();
    }

    let mut matched: Vec<(DesktopEntry, i32)> = entries.iter()
        .filter_map(|e| {
            let name_score = fuzzy_match(query, &e.name);
            let desc_score = fuzzy_match(query, &e.description).map(|s| s / 2);
            let best = name_score.max(desc_score);
            best.map(|s| (e.clone(), s))
        })
        .collect();

    // add frequency bonus
    FREQUENCY.with(|f| {
        let freq = f.borrow();
        for (entry, score) in &mut matched {
            if let Some(&count) = freq.get(&entry.name) {
                *score += (count * 50) as i32;
            }
        }
    });

    matched.sort_by(|a, b| b.1.cmp(&a.1));
    matched.into_iter().map(|(e, _)| e).collect()
}

fn calc_eval(expr: &str) -> Option<String> {
    // let e = expr.trim();
    let e = expr.trim().trim_matches('=').to_lowercase();
    if e.is_empty() { return None; }
    
    // let allowed = |c: char| c.is_ascii_digit() || "+-*/.^() ".contains(c);
    let allowed = |c: char| c.is_ascii_digit() || "+-*/.^() ".contains(c);
    if !e.chars().all(allowed) { return None; }
    
    // Using bc -l for floating point math
    let mut child = Command::new("bc")
        .arg("-l")
        .env("BC_LINE_LENGTH", "0")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn().ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        // scale=4 ensures we don't get 20 trailing zeros from bc
        let query = format!("scale=4; {}\n", e);
        let _ = stdin.write_all(query.as_bytes());
    }

    let output = child.wait_with_output().ok()?;
    if output.status.success() {
        let res = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // Strip trailing zeros and potential trailing dot
        if res.contains('.'){
        let cleaned = res.trim_end_matches('0').trim_end_matches('.').to_string();
        if cleaned.is_empty() || cleaned == "-" { return Some("0".to_string()); }
            return Some(cleaned)
        }
        Some(res)
    } else { None }
}

fn launch_app(entry: &DesktopEntry, terminal: &str) {
    let exec = &entry.exec;
    
    FREQUENCY.with(|f| {
        let mut freq = f.borrow_mut();
        *freq.entry(entry.name.clone()).or_insert(0) += 1;
    });

    log(APP_NAME, &format!("launching: {} ({})", entry.name, exec));

    if entry.terminal {
        let _ = Command::new(terminal)
            .arg("-e")
            .arg("sh")
            .arg("-c")
            .arg(exec)
            .spawn();
    } else {
        let _ = Command::new("sh")
            .arg("-c")
            .arg(exec)
            .spawn();
    }
}

fn load_icon(icon_name: &str, size: i32) -> Option<Image> {
    if icon_name.is_empty() { return None; }

    // absolute path
    if icon_name.starts_with('/') {
        let p = PathBuf::from(icon_name);
        if p.exists() {
            let img = Image::from_file(&p);
            img.set_pixel_size(size);
            return Some(img);
        }
    }

    // theme icon
    let display = gdk4::Display::default()?;
    let theme = gtk4::IconTheme::for_display(&display);
    
    if theme.has_icon(icon_name) {
        let img = Image::from_icon_name(icon_name);
        img.set_pixel_size(size);
        return Some(img);
    }

    None
}

fn build_row(entry: &DesktopEntry) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_focusable(false);
    
    let hbox = GtkBox::new(Orientation::Horizontal, 14);
    hbox.set_valign(Align::Center);

    // icon
    let icon_box = GtkBox::new(Orientation::Vertical, 0);
    icon_box.set_size_request(48, 48);
    icon_box.set_valign(Align::Center);
    icon_box.set_halign(Align::Center);
    icon_box.add_css_class("launch-icon-box");

    if let Some(img) = load_icon(&entry.icon, 32) {
        img.set_valign(Align::Center);
        img.set_halign(Align::Center);
        icon_box.append(&img);
    } else {
        let lbl = Label::new(Some(&entry.name.chars().next().unwrap_or('?').to_string()));
        lbl.add_css_class("launch-icon-fallback");
        lbl.set_valign(Align::Center);
        lbl.set_halign(Align::Center);
        icon_box.append(&lbl);
    }
    hbox.append(&icon_box);

    // content
    let content = GtkBox::new(Orientation::Vertical, 0);
    content.set_hexpand(true);
    content.set_valign(Align::Center);

    let title = Label::new(Some(&entry.name));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_max_width_chars(50);
    title.add_css_class("launch-title");
    content.append(&title);

    if !entry.description.is_empty() {
        let desc = Label::new(Some(&char_truncate(&entry.description, 60)));
        desc.set_xalign(0.0);
        desc.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        desc.set_max_width_chars(50);
        desc.add_css_class("launch-subtitle");
        content.append(&desc);
    }

    hbox.append(&content);
    row.set_child(Some(&hbox));
    row
}

fn build_calc_row(expr: &str, result: &str) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_focusable(false);
    
    let hbox = GtkBox::new(Orientation::Horizontal, 14);
    hbox.set_valign(Align::Center);

    let icon_box = GtkBox::new(Orientation::Vertical, 0);
    icon_box.set_size_request(48, 48);
    icon_box.set_valign(Align::Center);
    icon_box.add_css_class("launch-icon-box");
    let lbl = Label::new(Some("="));
    lbl.add_css_class("launch-icon-fallback");
    lbl.set_valign(Align::Center);
    icon_box.append(&lbl);
    hbox.append(&icon_box);

    let content = GtkBox::new(Orientation::Vertical, 0);
    content.set_hexpand(true);
    content.set_valign(Align::Center);

    let title = Label::new(Some(result));
    title.set_xalign(0.0);
    title.add_css_class("launch-title");
    title.add_css_class("launch-calc-result");
    content.append(&title);

    let sub = Label::new(Some(&format!("= {}", expr)));
    sub.set_xalign(0.0);
    sub.add_css_class("launch-subtitle");
    content.append(&sub);

    hbox.append(&content);
    row.set_child(Some(&hbox));
    row
}

fn populate_list(listbox: &ListBox, entries: &[DesktopEntry], query: &str, calc_enabled: bool) -> usize {
    while let Some(row) = listbox.row_at_index(0) { listbox.remove(&row); }

    // calculator mode
    if calc_enabled && query.starts_with('=') && query.len() > 1 {
        let expr = &query[1..];
        if let Some(result) = calc_eval(expr) {
            listbox.append(&build_calc_row(expr, &result));
            if let Some(first) = listbox.row_at_index(0) {
                listbox.select_row(Some(&first));
            }
            return 1; // Only show the calculator result
        }
    }

    let filtered = filter_entries(entries, query);
    let count = filtered.len();
    
    for e in filtered.iter().take(50) {
        listbox.append(&build_row(e));
    }

    if let Some(first) = listbox.row_at_index(0) {
        listbox.select_row(Some(&first));
    }
    count
}

fn get_filtered_entry(entries: &[DesktopEntry], query: &str, idx: usize) -> Option<DesktopEntry> {
    let filtered = filter_entries(entries, query);
    filtered.get(idx).cloned()
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
                    let ents = wg.entries.borrow();
                    let n = populate_list(&wg.listbox, &ents, "", cfg.calculator);
                    wg.status.set_text(&format!("{} apps", n));
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

    let entries: Rc<RefCell<Vec<DesktopEntry>>> = Rc::new(RefCell::new(Vec::new()));

    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(cfg.base.width)
        .default_height(cfg.base.height)
        .resizable(false)
        .build();

    apply_layer_shell(&window, &cfg.base, APP_NAME);
    window.set_default_size(cfg.base.width, cfg.base.height);

    let container = GtkBox::new(Orientation::Vertical, 0);
    container.add_css_class("launch-container");
    container.set_size_request(cfg.base.width, cfg.base.height);

    // header
    let header = GtkBox::new(Orientation::Vertical, 0);
    header.add_css_class("launch-header");

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
    header.append(&search_row);

    let section_label = Label::new(Some("Applications"));
    section_label.set_xalign(0.0);
    section_label.add_css_class("launch-section-label");
    header.append(&section_label);
    container.append(&header);

    // list
    let scroll = ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);
    scroll.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
    let listbox = ListBox::new();
    listbox.add_css_class("launch-list");
    listbox.set_selection_mode(gtk4::SelectionMode::Single);
    scroll.set_child(Some(&listbox));
    container.append(&scroll);
    let scroll_k = scroll.clone();
    // status bar
    let status_bar = GtkBox::new(Orientation::Horizontal, 0);
    status_bar.add_css_class("launch-status-bar");
    let status = Label::new(Some("0 apps"));
    status.add_css_class("launch-status-left");
    status.set_halign(Align::Start);
    status.set_hexpand(true);
    status_bar.append(&status);

    let hints = GtkBox::new(Orientation::Horizontal, 12);
    hints.set_halign(Align::End);
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
    status_bar.append(&hints);
    container.append(&status_bar);
    window.set_child(Some(&container));

    // search handler
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
    });

    // keybinds
    let key_ctrl = EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let ek = entries.clone();
    let lk = listbox.clone();
    let wk = window.clone();
    let sk = search.clone();

    key_ctrl.connect_key_pressed(move |_, key, _, mods| {
        let action = CONFIG.with(|c| match_action(&c.borrow().base.keybinds, key, mods));
        let terminal = CONFIG.with(|c| c.borrow().terminal.clone());
        let calc = CONFIG.with(|c| c.borrow().calculator);

        if let Some(action) = action {
            match action {
                Action::Close => { wk.set_visible(false); }
                Action::Select => {
                    let q = sk.text().to_string();
                    
                    // calc mode - copy result
                    if calc && q.starts_with('=') {
        if let Some(result) = calc_eval(&q[1..]) {
            // Use wl-copy for Wayland/Hyprland
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
                Action::ClearSearch => { sk.set_text(""); }
                Action::Next => {
                    if let Some(r) = lk.selected_row() {
                        if let Some(n) = lk.row_at_index(r.index() + 1) { lk.select_row(Some(&n)); common::css::scroll_to_selected(&lk, &scroll_k);}
                    }
                }
                Action::Prev => {
                    if let Some(r) = lk.selected_row() {
                        if r.index() > 0 {
                            if let Some(p) = lk.row_at_index(r.index() - 1) { lk.select_row(Some(&p)); common::css::scroll_to_selected(&lk, &scroll_k);}
                        }
                    }
                }
                Action::PageDown => {
                    if let Some(r) = lk.selected_row() {
                        let t = (r.index() + 10).min(lk.observe_children().n_items() as i32 - 1);
                        if let Some(nr) = lk.row_at_index(t) { lk.select_row(Some(&nr)); common::css::scroll_to_selected(&lk, &scroll_k);}
                    }
                }
                Action::PageUp => {
                    if let Some(r) = lk.selected_row() {
                        let t = (r.index() - 10).max(0);
                        if let Some(nr) = lk.row_at_index(t) { lk.select_row(Some(&nr)); common::css::scroll_to_selected(&lk, &scroll_k);}
                    }
                }
                Action::First => {
                    if let Some(r) = lk.row_at_index(0) { lk.select_row(Some(&r)); common::css::scroll_to_selected(&lk, &scroll_k);}
                }
                Action::Last => {
                    let n = lk.observe_children().n_items();
                    if n > 0 {
                        if let Some(r) = lk.row_at_index(n as i32 - 1) { lk.select_row(Some(&r)); common::css::scroll_to_selected(&lk, &scroll_k);}
                    }
                }
                _ => {}
            }
            return glib::Propagation::Stop;
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
            search: search.clone(), listbox: listbox.clone(),
            status: status.clone(), entries: entries.clone(),
        });
    });

    // load entries
    {
        let mut ents = entries.borrow_mut();
        *ents = load_entries();
        let n = populate_list(&listbox, &ents, "", cfg.calculator);
        status.set_text(&format!("{} apps", n));
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
    eprintln!("{} - {}\n", APP_NAME, "app launcher"); // or "clipboard manager"
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
        println!("Run 'launch-gui --generate-config' to create it.");
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
    println!("launch-gui reloaded");
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
        .application_id("com.vib1240n.launch-gui")
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
                                let ents = wg.entries.borrow();
                                let n = populate_list(&wg.listbox, &ents, "", cfg.calculator);
                                wg.status.set_text(&format!("{} apps", n));
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
