use crate::desktop::DesktopEntry;
use crate::files::{display, FileHit};
use common::css::char_truncate;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Image, Label, ListBox, ListBoxRow, Orientation};
use std::path::PathBuf;

/// Rows rendered per refresh
pub const MAX_ROWS: usize = 50;

/// One result row. The list is built 1:1 from a Vec<Item>, so a row's index
/// is always its item's index.
#[derive(Clone, Debug)]
pub enum Item {
    App(DesktopEntry),
    Calc { expr: String, result: String },
    File(FileHit),
    Snippet(snippets::Snippet),
    /// "\p name | trigger = content" typed in the search bar
    NewSnippet(snippets::Snippet),
    /// Non-actionable message (empty state, errors, indexing...)
    Info(String),
}

pub fn load_icon(icon_name: &str, size: i32) -> Option<Image> {
    if icon_name.is_empty() {
        return None;
    }
    if icon_name.starts_with('/') {
        let p = PathBuf::from(icon_name);
        if p.exists() {
            let img = Image::from_file(&p);
            img.set_pixel_size(size);
            return Some(img);
        }
    }
    let display = gdk4::Display::default()?;
    let theme = gtk4::IconTheme::for_display(&display);
    if theme.has_icon(icon_name) {
        let img = Image::from_icon_name(icon_name);
        img.set_pixel_size(size);
        return Some(img);
    }
    None
}

/// Icon for a file by name only (no file read): mime guess -> themed icon
fn file_icon(hit: &FileHit, size: i32) -> Image {
    let img = if hit.is_dir {
        Image::from_icon_name("folder")
    } else {
        let (ctype, _) = gio::content_type_guess(Some(&hit.path), &[]);
        Image::from_gicon(&gio::content_type_get_icon(&ctype))
    };
    img.set_pixel_size(size);
    img
}

fn icon_box(child: &impl IsA<gtk4::Widget>) -> GtkBox {
    let b = GtkBox::new(Orientation::Vertical, 0);
    b.set_size_request(48, 48);
    b.set_valign(Align::Center);
    b.set_halign(Align::Center);
    b.add_css_class("launch-icon-box");
    child.set_valign(Align::Center);
    child.set_halign(Align::Center);
    b.append(child);
    b
}

fn letter(text: &str) -> Label {
    let l = Label::new(Some(text));
    l.add_css_class("launch-icon-fallback");
    l
}

fn row(icon: GtkBox, title: &str, subtitle: &str, extra_title_class: Option<&str>) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_focusable(false);
    let hbox = GtkBox::new(Orientation::Horizontal, 14);
    hbox.set_valign(Align::Center);
    hbox.append(&icon);

    let content = GtkBox::new(Orientation::Vertical, 0);
    content.set_hexpand(true);
    content.set_valign(Align::Center);
    let t = Label::new(Some(title));
    t.set_xalign(0.0);
    t.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    t.set_max_width_chars(50);
    t.add_css_class("launch-title");
    if let Some(c) = extra_title_class {
        t.add_css_class(c);
    }
    content.append(&t);
    if !subtitle.is_empty() {
        let s = Label::new(Some(subtitle));
        s.set_xalign(0.0);
        s.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        s.set_max_width_chars(60);
        s.add_css_class("launch-subtitle");
        content.append(&s);
    }
    hbox.append(&content);
    row.set_child(Some(&hbox));
    row
}

pub fn build_row(item: &Item) -> ListBoxRow {
    match item {
        Item::App(e) => {
            let icon = match load_icon(&e.icon, 48) {
                Some(img) => icon_box(&img),
                None => icon_box(&letter(&e.name.chars().next().unwrap_or('?').to_string())),
            };
            row(icon, &e.name, &char_truncate(&e.description, 60), None)
        }
        Item::Calc { expr, result } => {
            row(icon_box(&letter("=")), result, &format!("= {expr}"), Some("launch-calc-result"))
        }
        Item::File(hit) => {
            let name = hit
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| hit.path.display().to_string());
            let parent = hit
                .path
                .parent()
                .map(|p| display(&p.to_string_lossy()))
                .unwrap_or_default();
            let title = if hit.is_dir { format!("{name}/") } else { name };
            row(icon_box(&file_icon(hit, 40)), &title, &parent, None)
        }
        Item::Snippet(s) => {
            let sub = match (&s.trigger, s.encrypted) {
                (Some(t), true) => format!("{t}  ·  encrypted"),
                (Some(t), false) => format!("{t}  ·  {}", char_truncate(&s.body, 50)),
                (None, true) => "encrypted".to_string(),
                (None, false) => char_truncate(&s.body, 60),
            };
            let glyph = if s.encrypted { "🔒" } else { "✂" };
            row(icon_box(&letter(glyph)), &s.name, &sub, None)
        }
        Item::NewSnippet(s) => {
            let kind = if s.encrypted { "encrypted" } else { "plain" };
            let trig = s.trigger.as_deref().map(|t| format!(" · trigger {t}")).unwrap_or_default();
            row(
                icon_box(&letter("+")),
                &format!("Save {kind} snippet \"{}\"", s.name),
                &format!("Enter to save{trig}"),
                None,
            )
        }
        Item::Info(msg) => row(icon_box(&letter("i")), msg, "", None),
    }
}

pub fn populate(listbox: &ListBox, items: &[Item]) {
    while let Some(r) = listbox.row_at_index(0) {
        listbox.remove(&r);
    }
    for it in items.iter().take(MAX_ROWS) {
        listbox.append(&build_row(it));
    }
    if let Some(first) = listbox.row_at_index(0) {
        listbox.select_row(Some(&first));
    }
}
