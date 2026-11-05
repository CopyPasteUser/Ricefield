//! Rice-independent file-list presentation primitives.
//!
//! Painting is intentionally separate from selection, drag/drop, navigation
//! and filesystem actions. A rice may replace this renderer without acquiring
//! authority to run or mutate file operations.

use crate::{
    filesystem::{self, Entry},
    theme::{Layout, Theme},
};
use eframe::egui::{
    self, Align2, Color32, FontId, Painter, Pos2, Rect, Response, Sense, Stroke, Vec2,
};

#[derive(Clone, Copy)]
pub struct FileCellLayout {
    pub width: f32,
    pub height: f32,
    pub grid: bool,
}

#[derive(Clone, Copy)]
struct FileCellVisual {
    selected: bool,
    hovered: bool,
    grid: bool,
}

/// Draw a file cell and return its response so the controller can decide
/// selection, navigation, context menus and drag/drop without renderer access
/// to any of those actions.
pub fn file_cell(
    ui: &mut egui::Ui,
    file: &Entry,
    theme: &Theme,
    selected: bool,
    layout: FileCellLayout,
    thumbnail: Option<(egui::TextureId, Vec2)>,
) -> (Rect, Response) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(layout.width.max(80.0), layout.height),
        Sense::click_and_drag(),
    );
    paint_file_cell(
        ui.painter(),
        rect,
        file,
        theme,
        FileCellVisual {
            selected,
            hovered: response.hovered(),
            grid: layout.grid,
        },
        thumbnail,
    );
    let response = response.on_hover_text(file.path.display().to_string());
    (rect, response)
}

pub fn sort_header(ui: &mut egui::Ui, sort_by: &str, descending: bool) -> Option<&'static str> {
    let mut change_sort = None;
    ui.horizontal(|ui| {
        for (key, label) in [
            ("name", "Name"),
            ("type", "Type"),
            ("size", "Size"),
            ("modified", "Modified"),
        ] {
            let caption = if sort_by == key {
                format!("{label} ({})", if descending { "desc" } else { "asc" })
            } else {
                label.to_owned()
            };
            if ui
                .small_button(caption)
                .on_hover_text("Sort by this column")
                .clicked()
            {
                change_sort = Some(key);
            }
        }
    });
    change_sort
}

pub fn shorten(name: &str, max: usize) -> String {
    let count = name.chars().count();
    if count <= max {
        return name.into();
    }
    let keep = max.saturating_sub(1);
    format!("{}…", name.chars().take(keep).collect::<String>())
}

fn paint_file_cell(
    painter: &Painter,
    rect: Rect,
    file: &Entry,
    theme: &Theme,
    visual: FileCellVisual,
    thumbnail: Option<(egui::TextureId, Vec2)>,
) {
    let painter = painter.with_clip_rect(rect);
    if visual.selected || visual.hovered {
        painter.rect_filled(
            rect.shrink(2.0),
            theme.metrics.rounding,
            if visual.selected {
                theme.palette.selection()
            } else {
                theme.palette.background()
            },
        );
    }
    let icon_size = theme.metrics.icon_size;
    let icon_area = if visual.grid {
        Rect::from_center_size(
            Pos2::new(rect.center().x, rect.top() + 34.0),
            Vec2::splat(icon_size * 1.6),
        )
    } else {
        Rect::from_center_size(
            Pos2::new(rect.left() + 22.0, rect.center().y),
            Vec2::splat(icon_size),
        )
    };
    if let Some((texture, size)) = thumbnail {
        let maximum = if visual.grid {
            Vec2::new(74.0, 55.0)
        } else {
            Vec2::splat(icon_size)
        };
        let scale = (maximum.x / size.x).min(maximum.y / size.y).min(1.0);
        let image_area = Rect::from_center_size(icon_area.center(), size * scale);
        painter.image(
            texture,
            image_area,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        paint_file_icon(&painter, icon_area, file, theme);
    }
    let name = if visual.grid {
        shorten(&file.name, 22)
    } else {
        shorten(&file.name, ((rect.width() * 0.53) / 7.3) as usize)
    };
    let font = FontId::proportional(if visual.grid { 12.5 } else { 13.0 });
    let foreground = theme.palette.text();
    if visual.grid {
        painter.text(
            Pos2::new(rect.center().x, rect.top() + 67.0),
            Align2::CENTER_CENTER,
            name,
            font,
            foreground,
        );
    } else {
        painter.text(
            Pos2::new(rect.left() + 43.0, rect.center().y),
            Align2::LEFT_CENTER,
            name,
            font,
            foreground,
        );
        let label = format!(
            "{}    {}    {}",
            file.kind(),
            if file.is_dir {
                "—".into()
            } else {
                filesystem::human_size(file.size)
            },
            filesystem::age(file.modified)
        );
        painter.text(
            Pos2::new(rect.left() + rect.width() * 0.61, rect.center().y),
            Align2::LEFT_CENTER,
            shorten(&label, ((rect.width() * 0.38) / 7.0) as usize),
            FontId::proportional(11.5),
            theme.palette.muted(),
        );
    }
}

fn paint_file_icon(painter: &Painter, rect: Rect, file: &Entry, theme: &Theme) {
    let round: u8 = if theme.layout == Layout::Classic {
        0
    } else {
        3
    };
    if file.is_dir {
        let top = Rect::from_min_max(
            Pos2::new(rect.left() + 2.0, rect.top() + 6.0),
            Pos2::new(rect.left() + rect.width() * 0.55, rect.top() + 15.0),
        );
        painter.rect_filled(top, round, theme.palette.folder());
        let body = Rect::from_min_max(
            Pos2::new(rect.left() + 1.0, rect.top() + 12.0),
            Pos2::new(rect.right() - 1.0, rect.bottom() - 3.0),
        );
        painter.rect_filled(body, round, theme.palette.folder());
        painter.line_segment(
            [
                Pos2::new(body.left() + 2.0, body.top() + 3.0),
                Pos2::new(body.right() - 2.0, body.top() + 3.0),
            ],
            Stroke::new(1.0, Color32::WHITE.gamma_multiply(0.6)),
        );
    } else {
        let body = Rect::from_min_max(
            Pos2::new(rect.left() + rect.width() * 0.15, rect.top() + 1.0),
            Pos2::new(rect.right() - rect.width() * 0.15, rect.bottom() - 1.0),
        );
        painter.rect_filled(body, round, theme.palette.file());
        let short = match file.kind() {
            "Image" => "IMG",
            "Audio" => "AUD",
            "Video" => "VID",
            "Archive" => "ZIP",
            "PDF" => "PDF",
            "Code" => "</>",
            "Link" => "LNK",
            _ => "FILE",
        };
        painter.text(
            body.center(),
            Align2::CENTER_CENTER,
            short,
            FontId::proportional((rect.width() * 0.29).clamp(8.0, 13.0)),
            Color32::WHITE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::shorten;

    #[test]
    fn file_label_truncation_preserves_unicode_scalars() {
        assert_eq!(shorten("file.txt", 10), "file.txt");
        assert_eq!(shorten("☃☃☃☃", 3), "☃☃…");
        assert_eq!(shorten("folder", 0), "…");
    }
}
