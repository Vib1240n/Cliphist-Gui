use crate::config::Config;
use crate::entries::{content_type, parse_image_meta, ClipEntry, PinnedClipEntry};
use common::css::char_truncate;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, DragSource, DropTarget, Label, ListBox, ListBoxRow, Orientation, Overlay,
    Picture,
};

const MAX_TEXT_PREVIEW: usize = 120;
const MAX_SUB_PREVIEW: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Recent,
    Pinned,
}

/// Create pin badge widget
fn create_pin_badge() -> Label {
    let badge = Label::new(Some("\u{1F4CC}")); // pushpin emoji
    badge.add_css_class("clip-pin-badge");
    badge.set_halign(Align::End);
    badge.set_valign(Align::Start);
    badge
}

/// Create icon box with optional pin badge overlay
fn create_icon_with_badge(
    is_image: bool,
    thumb_path: Option<&std::path::Path>,
    show_pin: bool,
) -> gtk4::Widget {
    let overlay = Overlay::new();

    if is_image {
        if let Some(path) = thumb_path {
            let pic = Picture::for_filename(path.to_str().unwrap_or(""));
            pic.set_size_request(48, 48);
            pic.add_css_class("clip-thumb");
            let frame = gtk4::Frame::new(None);
            frame.set_child(Some(&pic));
            frame.add_css_class("clip-thumb-frame");
            frame.set_size_request(48, 48);
            overlay.set_child(Some(&frame));
        } else {
            let ib = GtkBox::new(Orientation::Vertical, 0);
            ib.set_size_request(48, 48);
            ib.set_valign(Align::Center);
            ib.set_halign(Align::Center);
            ib.add_css_class("clip-text-icon");
            let lbl = Label::new(Some("I"));
            lbl.add_css_class("clip-text-icon-label");
            lbl.set_valign(Align::Center);
            lbl.set_halign(Align::Center);
            lbl.set_vexpand(true);
            ib.append(&lbl);
            overlay.set_child(Some(&ib));
        }
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
        overlay.set_child(Some(&ib));
    }

    if show_pin {
        let badge = create_pin_badge();
        overlay.add_overlay(&badge);
    }

    overlay.upcast()
}

/// Build row for regular clipboard entry, optionally showing pin icon
pub fn build_row(entry: &ClipEntry, is_pinned: bool, _cfg: &Config) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_focusable(false);
    row.set_widget_name(&entry.id);

    let hbox = GtkBox::new(Orientation::Horizontal, 14);
    hbox.set_valign(Align::Center);

    // Thumbnail or text icon with pin badge overlay
    let icon = create_icon_with_badge(entry.is_image, entry.thumb_path.as_deref(), is_pinned);
    hbox.append(&icon);

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

/// Build row for pinned entry (from storage, not cliphist)
pub fn build_pinned_row(entry: &PinnedClipEntry, _cfg: &Config) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_focusable(false);
    row.set_widget_name(&format!("pinned:{}", entry.meta.hash));

    let hbox = GtkBox::new(Orientation::Horizontal, 14);
    hbox.set_valign(Align::Center);

    // Thumbnail or text icon with pin badge (always shown for pinned)
    let is_image = entry.meta.content_type == "image";
    let icon = create_icon_with_badge(
        is_image,
        entry.thumb_path.as_deref(),
        true, // always show pin badge
    );
    hbox.append(&icon);

    let content = GtkBox::new(Orientation::Vertical, 0);
    content.set_hexpand(true);
    content.set_valign(Align::Center);

    let title_text = if is_image {
        "Image".to_string()
    } else {
        char_truncate(&entry.meta.preview, MAX_TEXT_PREVIEW)
    };

    let title = Label::new(Some(&title_text));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_max_width_chars(45);
    title.add_css_class("clip-title");
    content.append(&title);

    if !is_image {
        let sub = Label::new(Some(&char_truncate(&entry.meta.preview, MAX_SUB_PREVIEW)));
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
    let badge_text = if is_image { "IMAGE" } else { "TEXT" };
    let badge = Label::new(Some(badge_text));
    badge.set_halign(Align::End);
    badge.add_css_class("clip-badge");
    badge.add_css_class("clip-badge-pinned");
    right.append(&badge);
    hbox.append(&right);

    row.set_child(Some(&hbox));
    row
}

/// Setup drag-and-drop for a pinned row
pub fn setup_row_drag(row: &ListBoxRow, hash: String) {
    // Drag source
    let drag_source = DragSource::new();
    drag_source.set_actions(gdk4::DragAction::MOVE);

    let hash_for_drag = hash.clone();
    drag_source.connect_prepare(move |_, _, _| {
        let content = gdk4::ContentProvider::for_value(&hash_for_drag.to_value());
        Some(content)
    });

    row.add_controller(drag_source);

    // Drop target
    let drop_target = DropTarget::new(glib::Type::STRING, gdk4::DragAction::MOVE);

    drop_target.connect_drop(move |target, value, _, _| {
        if let Ok(source_hash) = value.get::<String>() {
            if source_hash != hash {
                // Get the listbox and reorder
                if let Some(row) = target.widget().and_downcast::<ListBoxRow>() {
                    if let Some(listbox) = row.parent().and_downcast::<ListBox>() {
                        // Signal that reorder happened - will be handled by app.rs
                        listbox.emit_by_name::<()>("row-activated", &[&row]);
                    }
                }
            }
        }
        true
    });

    row.add_controller(drop_target);
}

/// Populate listbox with recent entries (pinned at top if <= 5 pinned)
pub fn populate_list(
    listbox: &ListBox,
    entries: &[ClipEntry],
    pinned: &[PinnedClipEntry],
    query: &str,
    cfg: &Config,
) -> usize {
    while let Some(row) = listbox.row_at_index(0) {
        listbox.remove(&row);
    }

    let q = query.to_lowercase();
    let mut count = 0;

    // If <= 5 pinned items, show them at top of recent view
    if pinned.len() <= 5 {
        for p in pinned {
            if q.is_empty() || p.meta.preview.to_lowercase().contains(&q) {
                let row = build_pinned_row(p, cfg);
                listbox.append(&row);
                count += 1;
            }
        }
    }

    // Add regular entries
    for e in entries {
        if q.is_empty() || e.preview.to_lowercase().contains(&q) {
            let row = build_row(e, false, cfg);
            listbox.append(&row);
            count += 1;
        }
    }

    if let Some(first) = listbox.row_at_index(0) {
        listbox.select_row(Some(&first));
    }
    count
}

/// Populate listbox with only pinned entries (for Pinned tab)
pub fn populate_pinned_list(
    listbox: &ListBox,
    pinned: &[PinnedClipEntry],
    query: &str,
    cfg: &Config,
    enable_drag: bool,
) -> usize {
    while let Some(row) = listbox.row_at_index(0) {
        listbox.remove(&row);
    }

    let q = query.to_lowercase();
    let mut count = 0;

    for p in pinned {
        if q.is_empty() || p.meta.preview.to_lowercase().contains(&q) {
            let row = build_pinned_row(p, cfg);
            if enable_drag {
                setup_row_drag(&row, p.meta.hash.clone());
            }
            listbox.append(&row);
            count += 1;
        }
    }

    if let Some(first) = listbox.row_at_index(0) {
        listbox.select_row(Some(&first));
    }
    count
}

/// Create tab header bar
pub fn create_tab_bar(pinned_count: usize) -> GtkBox {
    let tab_bar = GtkBox::new(Orientation::Horizontal, 0);
    tab_bar.add_css_class("clip-tab-bar");

    let recent_tab = Label::new(Some("Clipboard"));
    recent_tab.add_css_class("clip-tab");
    recent_tab.add_css_class("clip-tab-active");
    recent_tab.set_widget_name("tab-recent");
    tab_bar.append(&recent_tab);

    let pinned_tab = Label::new(Some(&format!("Pinned ({})", pinned_count)));
    pinned_tab.add_css_class("clip-tab");
    pinned_tab.set_widget_name("tab-pinned");
    tab_bar.append(&pinned_tab);

    tab_bar
}

/// Update tab active state
pub fn update_tab_active(tab_bar: &GtkBox, active_tab: Tab) {
    let mut child = tab_bar.first_child();
    while let Some(widget) = child {
        if let Some(label) = widget.downcast_ref::<Label>() {
            let name = label.widget_name();
            label.remove_css_class("clip-tab-active");
            match active_tab {
                Tab::Recent if name == "tab-recent" => {
                    label.add_css_class("clip-tab-active");
                }
                Tab::Pinned if name == "tab-pinned" => {
                    label.add_css_class("clip-tab-active");
                }
                _ => {}
            }
        }
        child = widget.next_sibling();
    }
}

/// Update pinned count in tab bar
pub fn update_pinned_count(tab_bar: &GtkBox, count: usize) {
    let mut child = tab_bar.first_child();
    while let Some(widget) = child {
        if let Some(label) = widget.downcast_ref::<Label>() {
            if label.widget_name() == "tab-pinned" {
                label.set_text(&format!("Pinned ({})", count));
                break;
            }
        }
        child = widget.next_sibling();
    }
}
