use crate::entries::{content_type, parse_image_meta, ClipEntry};
use common::css::char_truncate;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, ListBox, ListBoxRow, Orientation, Picture};

const MAX_TEXT_PREVIEW: usize = 120;
const MAX_SUB_PREVIEW: usize = 60;

pub fn build_row(entry: &ClipEntry) -> ListBoxRow {
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
    let title_text = if entry.is_image {
        "Image".to_string()
    } else {
        char_truncate(&entry.preview, MAX_TEXT_PREVIEW)
    };

    let title = Label::new(Some(&title_text));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_max_width_chars(45);
    title.add_css_class("clip-title");
    content.append(&title);

    let sub_text = if entry.is_image {
        parse_image_meta(&entry.preview).unwrap_or_default()
    } else {
        char_truncate(&entry.preview, MAX_SUB_PREVIEW)
    };

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

pub fn populate_list(listbox: &ListBox, entries: &[ClipEntry], query: &str) -> usize {
    while let Some(row) = listbox.row_at_index(0) {
        listbox.remove(&row);
    }
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
