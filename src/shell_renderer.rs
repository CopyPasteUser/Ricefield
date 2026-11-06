//! Rice-independent file-list presentation primitives.
//!
//! Painting is intentionally separate from selection, drag/drop, navigation
//! and filesystem actions. A rice may replace this renderer without acquiring
//! authority to run or mutate file operations.

use crate::{
    filesystem::{self, Entry},
    theme::{Layout, ShellKind, Theme},
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

pub fn sort_header(
    ui: &mut egui::Ui,
    theme: &Theme,
    sort_by: &str,
    descending: bool,
) -> Option<&'static str> {
    let mut change_sort = None;
    if matches!(
        theme.shell_kind(),
        ShellKind::Aero | ShellKind::Ribbon | ShellKind::Aqua
    ) {
        // Both Explorer-era shells align their sortable details columns with
        // the same virtualized rows; the skins differ only in paint tokens.
        // Below 470 points, show just Name instead of clipping tiny headers.
        let available = ui.available_width();
        let compact = available < 470.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (key, label, portion) in [
                ("name", "Name", 0.50),
                ("modified", "Date modified", 0.20),
                ("type", "Type", 0.17),
                ("size", "Size", 0.13),
            ] {
                if compact && key != "name" {
                    continue;
                }
                let width = if compact {
                    available
                } else {
                    available * portion
                };
                let arrow = if sort_by == key {
                    if descending { " v" } else { " ^" }
                } else {
                    ""
                };
                let response = ui.add_sized(
                    [width.max(1.0), 25.0],
                    egui::Button::new(format!("{label}{arrow}"))
                        .fill(
                            if matches!(theme.shell_kind(), ShellKind::Ribbon | ShellKind::Aqua) {
                                theme.palette.panel()
                            } else {
                                Color32::from_rgb(247, 250, 253)
                            },
                        )
                        .stroke(Stroke::new(0.6, theme.palette.border()))
                        .corner_radius(egui::CornerRadius::ZERO),
                );
                if response.on_hover_text(format!("Sort by {label}")).clicked() {
                    change_sort = Some(key);
                }
            }
        });
        return change_sort;
    }
    let luna = theme.shell_kind() == ShellKind::Luna;
    let widths = [0.49, 0.13, 0.16, 0.22];
    let available = ui.available_width();
    let compact_luna = luna && available < 420.0;
    ui.horizontal(|ui| {
        if luna {
            ui.spacing_mut().item_spacing.x = 0.0;
        }
        for (index, (key, label)) in [
            ("name", "Name"),
            ("size", "Size"),
            ("type", "Type"),
            ("modified", "Modified"),
        ]
        .into_iter()
        .enumerate()
        {
            if compact_luna && index > 0 {
                continue;
            }
            let caption = if !luna && sort_by == key {
                format!("{label} ({})", if descending { "desc" } else { "asc" })
            } else {
                label.to_owned()
            };
            let button = egui::Button::new(caption);
            let response = if luna {
                ui.add_sized(
                    [
                        if compact_luna {
                            available
                        } else {
                            (available * widths[index]).max(40.0)
                        },
                        23.0,
                    ],
                    button
                        .fill(Color32::from_rgb(240, 239, 233))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(184, 186, 194)))
                        .corner_radius(egui::CornerRadius::ZERO),
                )
            } else {
                ui.add(button.small())
            };
            if response.on_hover_text("Sort by this column").clicked() {
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
            rect.shrink(
                if matches!(theme.shell_kind(), ShellKind::Aero | ShellKind::Aqua) {
                    0.5
                } else {
                    2.0
                },
            ),
            theme.metrics.rounding,
            if visual.selected {
                theme.palette.selection()
            } else if theme.shell_kind() == ShellKind::Aero {
                Color32::from_rgb(232, 246, 254)
            } else if theme.shell_kind() == ShellKind::Aqua {
                Color32::from_rgb(237, 252, 249)
            } else {
                theme.palette.background()
            },
        );
        if matches!(theme.shell_kind(), ShellKind::Aero | ShellKind::Aqua) {
            painter.rect_stroke(
                rect.shrink(1.0),
                2,
                Stroke::new(1.0, theme.palette.border()),
                egui::StrokeKind::Inside,
            );
        }
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
    let explorer_list = matches!(
        theme.shell_kind(),
        ShellKind::Aero | ShellKind::Ribbon | ShellKind::Aqua
    ) && !visual.grid;
    let compact_explorer = explorer_list && rect.width() < 470.0;
    let compact_luna = theme.shell_kind() == ShellKind::Luna && rect.width() < 420.0;
    let name = if visual.grid {
        shorten(&file.name, 22)
    } else if compact_luna || compact_explorer {
        shorten(&file.name, ((rect.width() - 52.0) / 7.0).max(1.0) as usize)
    } else if explorer_list {
        shorten(
            &file.name,
            ((rect.width() * 0.50 - 51.0) / 7.0).max(1.0) as usize,
        )
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
        if explorer_list && !compact_explorer {
            for (offset, portion, value) in [
                (0.50, 0.20, filesystem::age(file.modified)),
                (0.70, 0.17, file.kind().to_owned()),
                (
                    0.87,
                    0.13,
                    if file.is_dir {
                        String::new()
                    } else {
                        filesystem::human_size(file.size)
                    },
                ),
            ] {
                painter.text(
                    Pos2::new(rect.left() + rect.width() * offset + 7.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    shorten(
                        &value,
                        ((rect.width() * portion - 14.0) / 6.7).max(1.0) as usize,
                    ),
                    FontId::proportional(11.8),
                    theme.palette.muted(),
                );
            }
        } else if theme.shell_kind() == ShellKind::Luna && !compact_luna {
            let size = if file.is_dir {
                String::new()
            } else {
                filesystem::human_size(file.size)
            };
            for (offset, column_width, value) in [
                (0.49, 0.13, size),
                (0.62, 0.16, file.kind().to_owned()),
                (0.78, 0.22, filesystem::age(file.modified)),
            ] {
                painter.text(
                    Pos2::new(rect.left() + rect.width() * offset + 5.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    shorten(
                        &value,
                        ((rect.width() * column_width - 10.0) / 6.5).max(1.0) as usize,
                    ),
                    FontId::proportional(11.7),
                    theme.palette.text(),
                );
            }
        } else if !compact_luna {
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
}

fn paint_file_icon(painter: &Painter, rect: Rect, file: &Entry, theme: &Theme) {
    if theme.shell_kind() == ShellKind::Luna {
        paint_luna_file_icon(painter, rect, file);
        return;
    }
    if theme.shell_kind() == ShellKind::Aero {
        paint_aero_file_icon(painter, rect, file);
        return;
    }
    if theme.shell_kind() == ShellKind::Ribbon {
        paint_win10_file_icon(painter, rect, file, theme);
        return;
    }
    if theme.shell_kind() == ShellKind::Aqua {
        paint_aqua_file_icon(painter, rect, file, theme);
        return;
    }
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

/// Lightweight original glass-and-sunlight file glyphs. These are vectors,
/// not Windows, stock icon packs, web downloads or expensive blur textures.
fn paint_aqua_file_icon(painter: &Painter, rect: Rect, file: &Entry, theme: &Theme) {
    let size = rect.width().min(rect.height()).max(12.0);
    let at = |x: f32, y: f32| Pos2::new(rect.left() + x * size, rect.top() + y * size);
    let box_at = |x1: f32, y1: f32, x2: f32, y2: f32| Rect::from_min_max(at(x1, y1), at(x2, y2));
    if file.is_dir {
        painter.rect_filled(
            box_at(0.08, 0.20, 0.56, 0.43),
            2,
            Color32::from_rgb(206, 149, 55),
        );
        painter.rect_filled(
            box_at(0.06, 0.31, 0.94, 0.86),
            3,
            Color32::from_rgb(225, 172, 67),
        );
        painter.rect_filled(box_at(0.08, 0.35, 0.92, 0.84), 3, theme.palette.folder());
        painter.rect_filled(
            box_at(0.12, 0.38, 0.88, 0.48),
            2,
            Color32::WHITE.gamma_multiply(0.69),
        );
        painter.line_segment(
            [at(0.14, 0.74), at(0.86, 0.74)],
            Stroke::new(1.2, Color32::from_rgb(227, 162, 46)),
        );
    } else {
        painter.rect_filled(
            box_at(0.20, 0.07, 0.81, 0.92),
            3,
            Color32::from_rgb(214, 249, 251),
        );
        painter.rect_stroke(
            box_at(0.20, 0.07, 0.81, 0.92),
            3,
            Stroke::new(1.0, Color32::from_rgb(52, 151, 174)),
            egui::StrokeKind::Inside,
        );
        painter.rect_filled(
            box_at(0.26, 0.15, 0.74, 0.29),
            2,
            Color32::WHITE.gamma_multiply(0.88),
        );
        let emblem = match file.kind() {
            "Image" => "IMG",
            "Audio" => "MP3",
            "Video" => "VID",
            "Archive" => "ZIP",
            "PDF" => "PDF",
            "Code" => "DEV",
            "Link" => "URL",
            _ => "FILE",
        };
        painter.rect_filled(box_at(0.23, 0.39, 0.78, 0.70), 3, theme.palette.file());
        painter.text(
            at(0.5, 0.55),
            Align2::CENTER_CENTER,
            emblem,
            FontId::proportional((size * 0.19).clamp(7.0, 10.0)),
            Color32::WHITE,
        );
    }
}

/// Flat Explorer-era glyphs, drawn locally and independent of OS icon assets.
fn paint_win10_file_icon(painter: &Painter, rect: Rect, file: &Entry, theme: &Theme) {
    let s = rect.width().min(rect.height());
    let p = |x: f32, y: f32| Pos2::new(rect.left() + x * s, rect.top() + y * s);
    if file.is_dir {
        painter.rect_filled(
            Rect::from_min_max(p(0.09, 0.23), p(0.51, 0.47)),
            1,
            Color32::from_rgb(208, 156, 57),
        );
        painter.rect_filled(
            Rect::from_min_max(p(0.11, 0.33), p(0.89, 0.76)),
            1,
            theme.palette.folder(),
        );
        painter.rect_filled(
            Rect::from_min_max(p(0.09, 0.45), p(0.93, 0.84)),
            1,
            Color32::from_rgb(246, 205, 101),
        );
        painter.line_segment(
            [p(0.12, 0.47), p(0.90, 0.47)],
            Stroke::new(1.0, Color32::from_rgb(255, 238, 172)),
        );
    } else {
        let paper = Rect::from_min_max(p(0.22, 0.07), p(0.79, 0.92));
        painter.rect_filled(paper, 1, Color32::WHITE);
        painter.rect_stroke(
            paper,
            1,
            Stroke::new(1.0, theme.palette.file()),
            egui::StrokeKind::Inside,
        );
        painter.add(egui::Shape::convex_polygon(
            vec![p(0.60, 0.07), p(0.79, 0.26), p(0.60, 0.26)],
            Color32::from_rgb(204, 230, 248),
            Stroke::NONE,
        ));
        for y in [0.39, 0.49, 0.59] {
            painter.line_segment(
                [p(0.29, y), p(0.70, y)],
                Stroke::new(1.1, theme.palette.file()),
            );
        }
        let tag = match file.kind() {
            "PDF" => "PDF",
            "Archive" => "ZIP",
            "Image" => "IMG",
            "Audio" => "AUD",
            "Video" => "VID",
            _ => "",
        };
        if !tag.is_empty() {
            painter.rect_filled(
                Rect::from_min_max(p(0.20, 0.68), p(0.68, 0.86)),
                0,
                theme.palette.file(),
            );
            painter.text(
                p(0.44, 0.77),
                Align2::CENTER_CENTER,
                tag,
                FontId::proportional((s * 0.22).max(6.0)),
                Color32::WHITE,
            );
        }
    }
}

/// Original, layered vector artwork: recognizable Aero-era depth without
/// distributing any Windows icon assets or relying on external icon packs.
fn paint_aero_file_icon(painter: &Painter, rect: Rect, file: &Entry) {
    let s = rect.width().min(rect.height());
    let p = |x: f32, y: f32| Pos2::new(rect.left() + x * s, rect.top() + y * s);
    if file.is_dir {
        painter.rect_filled(
            Rect::from_min_max(p(0.10, 0.23), p(0.53, 0.45)),
            2,
            Color32::from_rgb(203, 147, 48),
        );
        painter.rect_filled(
            Rect::from_min_max(p(0.13, 0.28), p(0.91, 0.78)),
            2,
            Color32::from_rgb(231, 177, 67),
        );
        painter.rect_filled(
            Rect::from_min_max(p(0.17, 0.35), p(0.86, 0.66)),
            1,
            Color32::from_rgb(255, 238, 164),
        );
        painter.rect_filled(
            Rect::from_min_max(p(0.07, 0.43), p(0.93, 0.84)),
            2,
            Color32::from_rgb(244, 197, 89),
        );
        painter.line_segment(
            [p(0.10, 0.47), p(0.91, 0.47)],
            Stroke::new(1.0, Color32::from_rgb(255, 241, 176)),
        );
        painter.line_segment(
            [p(0.10, 0.81), p(0.91, 0.81)],
            Stroke::new(1.0, Color32::from_rgb(190, 137, 43)),
        );
    } else {
        let paper = Rect::from_min_max(p(0.20, 0.07), p(0.79, 0.91));
        painter.rect_filled(
            paper.translate(Vec2::new(1.0, 1.0)),
            1,
            Color32::from_rgb(147, 177, 193),
        );
        painter.rect_filled(paper, 1, Color32::WHITE);
        painter.rect_stroke(
            paper,
            1,
            Stroke::new(1.0, Color32::from_rgb(98, 142, 179)),
            egui::StrokeKind::Inside,
        );
        painter.add(egui::Shape::convex_polygon(
            vec![p(0.60, 0.07), p(0.79, 0.27), p(0.60, 0.27)],
            Color32::from_rgb(200, 229, 249),
            Stroke::NONE,
        ));
        for y in [0.39, 0.49, 0.59] {
            painter.line_segment(
                [p(0.28, y), p(0.71, y)],
                Stroke::new(1.0, Color32::from_rgb(153, 186, 208)),
            );
        }
        let (band, symbol) = match file.kind() {
            "Image" => (Color32::from_rgb(65, 148, 100), "IMG"),
            "Audio" => (Color32::from_rgb(151, 105, 181), "AUD"),
            "Video" => (Color32::from_rgb(73, 126, 178), "VID"),
            "Archive" => (Color32::from_rgb(196, 129, 49), "ZIP"),
            "PDF" => (Color32::from_rgb(178, 62, 59), "PDF"),
            _ => (Color32::from_rgb(65, 142, 197), ""),
        };
        painter.rect_filled(Rect::from_min_max(p(0.18, 0.70), p(0.63, 0.86)), 1, band);
        painter.text(
            p(0.40, 0.78),
            Align2::CENTER_CENTER,
            symbol,
            FontId::proportional((s * 0.23).max(6.0)),
            Color32::WHITE,
        );
    }
}

/// Original multi-layer vector icons evoke the depth of the XP era without
/// bundling or tracing platform artwork.
fn paint_luna_file_icon(painter: &Painter, rect: Rect, file: &Entry) {
    let s = rect.width().min(rect.height());
    let p = |x: f32, y: f32| Pos2::new(rect.left() + x * s, rect.top() + y * s);
    if file.is_dir {
        let shadow = Rect::from_min_max(p(0.12, 0.29), p(0.92, 0.78));
        painter.rect_filled(shadow, 1, Color32::from_rgb(170, 116, 21));
        let tab = Rect::from_min_max(p(0.12, 0.20), p(0.48, 0.37));
        painter.rect_filled(tab, 1, Color32::from_rgb(228, 167, 39));
        painter.rect_filled(
            Rect::from_min_max(p(0.14, 0.25), p(0.88, 0.72)),
            1,
            Color32::from_rgb(255, 217, 98),
        );
        painter.rect_filled(
            Rect::from_min_max(p(0.10, 0.39), p(0.90, 0.80)),
            2,
            Color32::from_rgb(245, 184, 53),
        );
        painter.line_segment(
            [p(0.13, 0.42), p(0.88, 0.42)],
            Stroke::new((s * 0.035).max(1.0), Color32::from_rgb(255, 236, 148)),
        );
        painter.line_segment(
            [p(0.13, 0.77), p(0.88, 0.77)],
            Stroke::new(1.0, Color32::from_rgb(206, 139, 28)),
        );
    } else {
        let paper = Rect::from_min_max(p(0.21, 0.10), p(0.77, 0.88));
        painter.rect_filled(
            paper.translate(Vec2::new(1.5, 1.5)),
            1,
            Color32::from_rgb(139, 158, 181),
        );
        painter.rect_filled(paper, 1, Color32::WHITE);
        painter.rect_stroke(
            paper,
            1,
            Stroke::new(1.0, Color32::from_rgb(90, 118, 150)),
            egui::StrokeKind::Inside,
        );
        painter.add(egui::Shape::convex_polygon(
            vec![p(0.61, 0.10), p(0.77, 0.25), p(0.61, 0.25)],
            Color32::from_rgb(192, 219, 240),
            Stroke::NONE,
        ));
        for y in [0.39, 0.47, 0.55, 0.63] {
            painter.line_segment(
                [p(0.28, y), p(0.69, y)],
                Stroke::new(1.0, Color32::from_rgb(134, 166, 194)),
            );
        }
        let strip = match file.kind() {
            "Image" => Color32::from_rgb(83, 160, 72),
            "Archive" => Color32::from_rgb(198, 122, 50),
            "PDF" => Color32::from_rgb(188, 67, 62),
            _ => Color32::from_rgb(75, 134, 199),
        };
        painter.rect_filled(Rect::from_min_max(p(0.19, 0.68), p(0.47, 0.83)), 1, strip);
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
