use crate::entries::{content_type, parse_image_meta, ClipEntry};
use common::css::char_truncate;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, ListBox, ListBoxRow, Orientation, Picture};
use std::path::PathBuf;

const MAX_TEXT_PREVIEW: usize = 120;
const MAX_SUB_PREVIEW: usize = 60;

/// Build a row - uses placeholder for missing thumbnails
pub fn build_row(entry: &ClipEntry) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_focusable(false);

    // Store the entry ID as widget name for later thumbnail updates
    row.set_widget_name(&entry.id);

    let hbox = GtkBox::new(Orientation::Horizontal, 14);
    hbox.set_valign(Align::Center);

    // Thumbnail/icon container
    let thumb_container = GtkBox::new(Orientation::Vertical, 0);
    thumb_container.set_size_request(48, 48);
    thumb_container.set_valign(Align::Center);
    thumb_container.set_halign(Align::Center);
    // Mark container for easy lookup
    thumb_container.set_widget_name("thumb_container");

    if let Some(ref path) = entry.thumb_path {
        // Has cached thumbnail - show it
        let pic = Picture::for_filename(path.to_str().unwrap_or(""));
        pic.set_size_request(48, 48);
        pic.add_css_class("clip-thumb");
        let frame = gtk4::Frame::new(None);
        frame.set_child(Some(&pic));
        frame.add_css_class("clip-thumb-frame");
        frame.set_size_request(48, 48);
        thumb_container.append(&frame);
    } else if entry.is_image {
        // Image without thumbnail - show loading placeholder
        let ib = GtkBox::new(Orientation::Vertical, 0);
        ib.set_size_request(48, 48);
        ib.set_valign(Align::Center);
        ib.set_halign(Align::Center);
        ib.add_css_class("clip-text-icon");
        ib.add_css_class("clip-thumb-loading");
        let lbl = Label::new(Some("...")); // Loading indicator
        lbl.add_css_class("clip-text-icon-label");
        lbl.set_valign(Align::Center);
        lbl.set_halign(Align::Center);
        lbl.set_vexpand(true);
        ib.append(&lbl);
        thumb_container.append(&ib);
    } else {
        // Text entry - show T icon
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
        thumb_container.append(&ib);
    }

    hbox.append(&thumb_container);

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

/// Update a row's thumbnail after async generation
pub fn update_row_thumbnail(listbox: &ListBox, id: &str, path: &PathBuf) {
    // Find the row by ID
    let mut idx = 0;
    while let Some(row) = listbox.row_at_index(idx) {
        if row.widget_name() == id {
            // Found the row - update its thumbnail
            if let Some(hbox) = row.child().and_then(|c| c.downcast::<GtkBox>().ok()) {
                if let Some(container) = hbox.first_child() {
                    if let Ok(container) = container.downcast::<GtkBox>() {
                        if container.widget_name() == "thumb_container" {
                            // Remove old content
                            while let Some(child) = container.first_child() {
                                container.remove(&child);
                            }

                            // Add new thumbnail
                            let pic = Picture::for_filename(path.to_str().unwrap_or(""));
                            pic.set_size_request(48, 48);
                            pic.add_css_class("clip-thumb");
                            let frame = gtk4::Frame::new(None);
                            frame.set_child(Some(&pic));
                            frame.add_css_class("clip-thumb-frame");
                            frame.set_size_request(48, 48);
                            container.append(&frame);
                        }
                    }
                }
            }
            break;
        }
        idx += 1;
    }
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
