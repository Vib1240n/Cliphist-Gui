use crate::entries::{content_type, parse_image_meta, ClipEntry, PinnedClipEntry};
use common::css::char_truncate;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, DragSource, DropTarget, GestureClick, Label, ListBox, ListBoxRow,
    Orientation, Overlay, Picture,
};
use std::path::Path;

const MAX_TEXT_PREVIEW: usize = 120;
const MAX_SUB_PREVIEW: usize = 60;
/// Rows actually built per refresh. Matching runs over everything; only the
/// first N are rendered so search stays instant with a long history.
const MAX_ROWS: usize = 200;

/// Row name prefix for pinned rows; regular rows are named by cliphist id.
pub const PINNED_PREFIX: &str = "pinned:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Recent,
    Pinned,
}

/// Called after a drag-and-drop reorder: (source hash, target hash)
pub type MoveFn = fn(String, String);

fn create_pin_badge() -> Label {
    let badge = Label::new(Some("\u{1F4CC}")); // pushpin
    badge.add_css_class("clip-pin-badge");
    badge.set_halign(Align::End);
    badge.set_valign(Align::Start);
    badge
}

fn letter_icon(letter: &str, pending: bool) -> GtkBox {
    let ib = GtkBox::new(Orientation::Vertical, 0);
    ib.set_size_request(48, 48);
    ib.set_valign(Align::Center);
    ib.set_halign(Align::Center);
    ib.add_css_class("clip-text-icon");
    if pending {
        ib.add_css_class("clip-thumb-pending");
    }
    let lbl = Label::new(Some(letter));
    lbl.add_css_class("clip-text-icon-label");
    lbl.set_valign(Align::Center);
    lbl.set_halign(Align::Center);
    lbl.set_vexpand(true);
    ib.append(&lbl);
    ib
}

fn thumb_frame(path: &Path) -> gtk4::Frame {
    let pic = Picture::for_filename(path);
    pic.set_size_request(48, 48);
    pic.add_css_class("clip-thumb");
    let frame = gtk4::Frame::new(None);
    frame.set_child(Some(&pic));
    frame.add_css_class("clip-thumb-frame");
    frame.set_size_request(48, 48);
    frame
}

/// Icon cell. Images whose thumbnail isn't generated yet get a placeholder and
/// remember the thumbnail path (overlay widget name), so `refresh_thumbnails`
/// can swap the picture in when the background worker finishes.
fn create_icon(is_image: bool, thumb_path: Option<&Path>, show_pin: bool) -> gtk4::Widget {
    let overlay = Overlay::new();
    match (is_image, thumb_path) {
        (true, Some(path)) if path.exists() => overlay.set_child(Some(&thumb_frame(path))),
        (true, Some(path)) => {
            overlay.set_widget_name(&path.to_string_lossy());
            overlay.set_child(Some(&letter_icon("I", true)));
        }
        (true, None) => overlay.set_child(Some(&letter_icon("I", false))),
        (false, _) => overlay.set_child(Some(&letter_icon("T", false))),
    }
    if show_pin {
        overlay.add_overlay(&create_pin_badge());
    }
    overlay.upcast()
}

fn text_column(title_text: &str, sub_text: &str) -> GtkBox {
    let content = GtkBox::new(Orientation::Vertical, 0);
    content.set_hexpand(true);
    content.set_valign(Align::Center);

    let title = Label::new(Some(title_text));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_max_width_chars(45);
    title.add_css_class("clip-title");
    content.append(&title);

    if !sub_text.is_empty() {
        let sub = Label::new(Some(sub_text));
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        sub.set_max_width_chars(45);
        sub.add_css_class("clip-subtitle");
        content.append(&sub);
    }
    content
}

fn type_badge(text: &str, pinned: bool) -> GtkBox {
    let right = GtkBox::new(Orientation::Vertical, 2);
    right.set_valign(Align::Start);
    right.set_halign(Align::End);
    right.set_margin_top(2);
    let badge = Label::new(Some(text));
    badge.set_halign(Align::End);
    badge.add_css_class("clip-badge");
    if pinned {
        badge.add_css_class("clip-badge-pinned");
    }
    right.append(&badge);
    right
}

/// Regular cliphist entry. Row name = cliphist id (used to find the entry back).
pub fn build_row(entry: &ClipEntry) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_focusable(false);
    row.set_widget_name(&entry.id);

    let hbox = GtkBox::new(Orientation::Horizontal, 14);
    hbox.set_valign(Align::Center);
    hbox.append(&create_icon(entry.is_image, entry.thumb_path.as_deref(), false));

    let (title, sub) = if entry.is_image {
        ("Image".to_string(), parse_image_meta(&entry.preview).unwrap_or_default())
    } else {
        (
            char_truncate(&entry.preview, MAX_TEXT_PREVIEW),
            char_truncate(&entry.preview, MAX_SUB_PREVIEW),
        )
    };
    hbox.append(&text_column(&title, &sub));
    hbox.append(&type_badge(content_type(entry), false));

    row.set_child(Some(&hbox));
    row
}

/// Pinned entry (from pinned storage). Row name = "pinned:<hash>".
pub fn build_pinned_row(entry: &PinnedClipEntry) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_focusable(false);
    row.set_widget_name(&format!("{PINNED_PREFIX}{}", entry.meta.hash));

    let is_image = entry.meta.is_image();
    let hbox = GtkBox::new(Orientation::Horizontal, 14);
    hbox.set_valign(Align::Center);
    hbox.append(&create_icon(is_image, entry.thumb_path.as_deref(), true));

    let (title, sub) = if is_image {
        ("Image".to_string(), parse_image_meta(&entry.meta.preview).unwrap_or_default())
    } else {
        (
            char_truncate(&entry.meta.preview, MAX_TEXT_PREVIEW),
            char_truncate(&entry.meta.preview, MAX_SUB_PREVIEW),
        )
    };
    hbox.append(&text_column(&title, &sub));
    hbox.append(&type_badge(if is_image { "IMAGE" } else { "TEXT" }, true));

    row.set_child(Some(&hbox));
    row
}

/// Drag a pinned row onto another to reorder.
fn setup_row_drag(row: &ListBoxRow, hash: String, on_move: MoveFn) {
    let drag_source = DragSource::new();
    drag_source.set_actions(gdk4::DragAction::MOVE);
    let hash_for_drag = hash.clone();
    drag_source.connect_prepare(move |_, _, _| {
        Some(gdk4::ContentProvider::for_value(&hash_for_drag.to_value()))
    });
    row.add_controller(drag_source);

    let drop_target = DropTarget::new(glib::Type::STRING, gdk4::DragAction::MOVE);
    drop_target.connect_drop(move |_, value, _, _| {
        let Ok(source) = value.get::<String>() else { return false };
        if source == hash {
            return false;
        }
        let target = hash.clone();
        // Rebuild the list after the drop handler returns, not inside it
        glib::idle_add_local_once(move || on_move(source, target));
        true
    });
    row.add_controller(drop_target);
}

fn clear(listbox: &ListBox) {
    while let Some(row) = listbox.row_at_index(0) {
        listbox.remove(&row);
    }
}

fn select_first(listbox: &ListBox) {
    if let Some(first) = listbox.row_at_index(0) {
        listbox.select_row(Some(&first));
    }
}

/// Recent tab. `inline_pinned`: show pins on top (when there are few of them).
/// Returns the number of matches (may exceed the rows rendered).
pub fn populate_list(
    listbox: &ListBox,
    entries: &[ClipEntry],
    pinned: &[PinnedClipEntry],
    query: &str,
    inline_pinned: bool,
) -> usize {
    clear(listbox);
    let q = query.to_lowercase();
    let mut matches = 0;
    let mut rendered = 0;

    if inline_pinned {
        for p in pinned {
            if q.is_empty() || p.meta.preview.to_lowercase().contains(&q) {
                matches += 1;
                if rendered < MAX_ROWS {
                    listbox.append(&build_pinned_row(p));
                    rendered += 1;
                }
            }
        }
    }
    for e in entries {
        if q.is_empty() || e.preview.to_lowercase().contains(&q) {
            matches += 1;
            if rendered < MAX_ROWS {
                listbox.append(&build_row(e));
                rendered += 1;
            }
        }
    }

    select_first(listbox);
    matches
}

/// Pinned tab, with drag-and-drop reordering.
pub fn populate_pinned_list(
    listbox: &ListBox,
    pinned: &[PinnedClipEntry],
    query: &str,
    on_move: MoveFn,
) -> usize {
    clear(listbox);
    let q = query.to_lowercase();
    let mut count = 0;
    for p in pinned {
        if q.is_empty() || p.meta.preview.to_lowercase().contains(&q) {
            let row = build_pinned_row(p);
            if q.is_empty() {
                // reordering a filtered list would be confusing
                setup_row_drag(&row, p.meta.hash.clone(), on_move);
            }
            listbox.append(&row);
            count += 1;
        }
    }
    select_first(listbox);
    count
}

/// Swap generated thumbnails into placeholder rows without rebuilding the
/// list (keeps selection and scroll position).
pub fn refresh_thumbnails(listbox: &ListBox) {
    let mut i = 0;
    while let Some(row) = listbox.row_at_index(i) {
        i += 1;
        let Some(hbox) = row.child() else { continue };
        let Some(overlay) = hbox.first_child().and_downcast::<Overlay>() else { continue };
        let name = overlay.widget_name();
        if name.is_empty() {
            continue;
        }
        let path = Path::new(name.as_str());
        let pending = overlay.child().is_some_and(|c| c.has_css_class("clip-thumb-pending"));
        if pending && path.exists() {
            overlay.set_child(Some(&thumb_frame(path)));
        }
    }
}

/// Tab bar: "Clipboard" | "Pinned (n)". Each tab is clickable.
pub fn create_tab_bar(on_click: fn(Tab)) -> GtkBox {
    let tab_bar = GtkBox::new(Orientation::Horizontal, 0);
    tab_bar.add_css_class("clip-tab-bar");

    for (text, name, tab) in [
        ("Clipboard", "tab-recent", Tab::Recent),
        ("Pinned", "tab-pinned", Tab::Pinned),
    ] {
        let label = Label::new(Some(text));
        label.add_css_class("clip-tab");
        label.set_widget_name(name);
        let click = GestureClick::new();
        click.connect_released(move |_, _, _, _| on_click(tab));
        label.add_controller(click);
        tab_bar.append(&label);
    }
    update_tab_active(&tab_bar, Tab::Recent);
    tab_bar
}

pub fn update_tab_active(tab_bar: &GtkBox, active_tab: Tab) {
    let mut child = tab_bar.first_child();
    while let Some(widget) = child {
        let active = matches!(
            (active_tab, widget.widget_name().as_str()),
            (Tab::Recent, "tab-recent") | (Tab::Pinned, "tab-pinned")
        );
        if active {
            widget.add_css_class("clip-tab-active");
        } else {
            widget.remove_css_class("clip-tab-active");
        }
        child = widget.next_sibling();
    }
}

pub fn update_pinned_count(tab_bar: &GtkBox, count: usize) {
    let mut child = tab_bar.first_child();
    while let Some(widget) = child {
        if widget.widget_name() == "tab-pinned" {
            if let Some(label) = widget.downcast_ref::<Label>() {
                label.set_text(&format!("Pinned ({count})"));
            }
            break;
        }
        child = widget.next_sibling();
    }
}
