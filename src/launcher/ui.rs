use std::path::PathBuf;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Image, Label, ListBox, ListBoxRow, Orientation,
};
use common::css::char_truncate;
use crate::desktop::DesktopEntry;
use crate::search::filter_entries;
use crate::calc::calc_eval;

pub fn load_icon(icon_name: &str, size: i32) -> Option<Image> {
    if icon_name.is_empty() { return None; }

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

pub fn build_row(entry: &DesktopEntry) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_focusable(false);
    
    let hbox = GtkBox::new(Orientation::Horizontal, 14);
    hbox.set_valign(Align::Center);

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

pub fn build_calc_row(expr: &str, result: &str) -> ListBoxRow {
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

pub fn populate_list(listbox: &ListBox, entries: &[DesktopEntry], query: &str, calc_enabled: bool) -> usize {
    while let Some(row) = listbox.row_at_index(0) { listbox.remove(&row); }

    if calc_enabled && query.starts_with('=') && query.len() > 1 {
        let expr = &query[1..];
        if let Some(result) = calc_eval(expr) {
            listbox.append(&build_calc_row(expr, &result));
            if let Some(first) = listbox.row_at_index(0) {
                listbox.select_row(Some(&first));
            }
            return 1;
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

