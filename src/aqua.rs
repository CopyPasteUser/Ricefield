//! Original Frutiger Aero-inspired controls for RiceField's Aqua shell.
//!
//! A small, painter-based visual layer: no downloaded OS artwork, blur
//! effects, animated textures or theme-specific filesystem operations.

use crate::theme::Theme;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AquaAction {
    NewFolder,
    Favorite,
    Copy,
    Cut,
    Paste,
    Rename,
    Trash,
    Preview,
    ToggleView,
    Split,
    Tools,
    Discover,
    Refresh,
    Transfers,
}

impl AquaAction {
    const ALL: [Self; 14] = [
        Self::NewFolder,
        Self::Favorite,
        Self::Copy,
        Self::Cut,
        Self::Paste,
        Self::Rename,
        Self::Trash,
        Self::Preview,
        Self::ToggleView,
        Self::Split,
        Self::Tools,
        Self::Discover,
        Self::Refresh,
        Self::Transfers,
    ];

    const QUICK: [Self; 5] = [
        Self::NewFolder,
        Self::Favorite,
        Self::Copy,
        Self::Paste,
        Self::Preview,
    ];

    const DISPLAY: [Self; 10] = [
        Self::NewFolder,
        Self::Favorite,
        Self::Copy,
        Self::Paste,
        Self::Rename,
        Self::Trash,
        Self::Preview,
        Self::ToggleView,
        Self::Split,
        Self::Tools,
    ];

    fn display_label(self, state: AquaContext) -> &'static str {
        if self == Self::Favorite && state.pinned {
            "Unpin"
        } else if self == Self::ToggleView && state.grid {
            "List view"
        } else if self == Self::ToggleView {
            "Grid view"
        } else {
            self.label()
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NewFolder => "New folder",
            Self::Favorite => "Pin here",
            Self::Copy => "Copy",
            Self::Cut => "Cut",
            Self::Paste => "Paste",
            Self::Rename => "Rename",
            Self::Trash => "Trash",
            Self::Preview => "Preview",
            Self::ToggleView => "View",
            Self::Split => "Split",
            Self::Tools => "Tools",
            Self::Discover => "Network",
            Self::Refresh => "Refresh",
            Self::Transfers => "Transfers",
        }
    }

    fn needs_local(self) -> bool {
        matches!(
            self,
            Self::NewFolder
                | Self::Favorite
                | Self::Copy
                | Self::Cut
                | Self::Paste
                | Self::Rename
                | Self::Trash
                | Self::Preview
                | Self::Split
        )
    }

    fn needs_selection(self) -> bool {
        matches!(
            self,
            Self::Copy | Self::Cut | Self::Rename | Self::Trash | Self::Preview
        )
    }
}

#[derive(Clone, Copy)]
pub struct AquaContext {
    pub local: bool,
    pub selected: usize,
    pub pinned: bool,
    pub grid: bool,
    pub split: bool,
    pub advanced: bool,
}

impl AquaContext {
    pub fn enabled(self, action: AquaAction) -> bool {
        (!action.needs_local() || self.local)
            && (!action.needs_selection() || self.selected > 0)
            && (!matches!(action, AquaAction::Rename | AquaAction::Preview) || self.selected == 1)
    }

    fn active(self, action: AquaAction) -> bool {
        match action {
            AquaAction::Favorite => self.pinned,
            AquaAction::Split => self.split,
            AquaAction::Tools => self.advanced,
            AquaAction::ToggleView => self.grid,
            _ => false,
        }
    }
}

/// Light, non-interactive sky/water identity. The real location and search
/// controls sit immediately below it, so this is never a fake address bar.
pub fn masthead(ui: &mut egui::Ui, theme: &Theme, folder: &str, remote: bool) {
    let width = ui.available_width().max(190.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 48.0), Sense::hover());
    let paint = ui.painter().with_clip_rect(rect);
    let base = theme.palette.chrome();
    let sky = Color32::from_rgb(113, 201, 225);
    let water = Color32::from_rgb(59, 159, 191);
    let leaf = Color32::from_rgb(138, 208, 140);
    paint.rect_filled(rect, 12, base);
    // Flat translucent-looking bands in place of resource-intensive blur.
    paint.rect_filled(
        Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + 20.0)),
        12,
        sky,
    );
    paint.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(rect.left(), rect.top() + 36.0),
            Pos2::new(rect.left() + width * 0.38, rect.top() + 30.0),
            Pos2::new(rect.left() + width * 0.76, rect.top() + 39.0),
            Pos2::new(rect.right(), rect.top() + 31.0),
            rect.right_bottom(),
            rect.left_bottom(),
        ],
        water.gamma_multiply(0.54),
        Stroke::NONE,
    ));
    paint.rect_filled(
        Rect::from_min_max(
            rect.min + Vec2::new(2.0, 2.0),
            Pos2::new(rect.right() - 2.0, rect.top() + 7.0),
        ),
        6,
        Color32::WHITE.gamma_multiply(0.40),
    );
    let orb = rect.left_center() + Vec2::new(26.0, 0.0);
    paint.circle_filled(orb, 18.0, Color32::WHITE.gamma_multiply(0.85));
    paint.circle_filled(orb, 15.0, Color32::from_rgb(65, 190, 208));
    paint.circle_filled(
        orb + Vec2::new(-3.0, -5.0),
        8.0,
        Color32::from_rgb(177, 240, 239),
    );
    paint.circle_filled(orb + Vec2::new(5.0, 6.0), 4.0, leaf);
    let title = if folder.is_empty() {
        "My spaces".into()
    } else {
        let label: String = folder.chars().take(46).collect();
        if folder.chars().count() > 46 {
            format!("{label}…")
        } else {
            label
        }
    };
    paint.text(
        rect.left_center() + Vec2::new(56.0, -5.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(17.0),
        Color32::from_rgb(16, 66, 86),
    );
    paint.text(
        rect.left_center() + Vec2::new(57.0, 12.0),
        Align2::LEFT_CENTER,
        if remote {
            "NETWORK · READ ONLY"
        } else {
            "RICEFIELD · AQUA EDITION"
        },
        FontId::proportional(10.0),
        Color32::from_rgb(33, 91, 104),
    );
    if width > 580.0 {
        for (x, y, r) in [(0.77, 0.27, 7.0), (0.84, 0.65, 12.0), (0.95, 0.38, 5.0)] {
            let point = Pos2::new(rect.left() + x * width, rect.top() + y * rect.height());
            paint.circle_stroke(
                point,
                r,
                Stroke::new(1.6, Color32::WHITE.gamma_multiply(0.68)),
            );
            paint.circle_filled(
                point - Vec2::new(r * 0.25, r * 0.25),
                r * 0.20,
                Color32::WHITE,
            );
        }
    }
}

pub fn nav_button(
    ui: &mut egui::Ui,
    theme: &Theme,
    text: &str,
    tooltip: &str,
    enabled: bool,
) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(
            egui::RichText::new(text)
                .strong()
                .color(theme.palette.text()),
        )
        .fill(Color32::from_rgb(230, 252, 253))
        .stroke(Stroke::new(1.0, theme.palette.border()))
        .corner_radius(16),
    )
    .on_hover_text(tooltip)
    .clicked()
}

/// A responsive glass-shelf dock. Only semantic actions leave this module.
pub fn dock(ui: &mut egui::Ui, theme: &Theme, state: AquaContext) -> Option<AquaAction> {
    let mut action = None;
    egui::Frame::new()
        .fill(theme.palette.chrome())
        .stroke(Stroke::new(1.0, theme.palette.border()))
        .corner_radius(15)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            if ui.available_width() < 830.0 {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    for command in AquaAction::QUICK {
                        let button = egui::Button::new(command.display_label(state))
                            .selected(state.active(command))
                            .fill(Color32::from_rgb(229, 250, 249));
                        if ui.add_enabled(state.enabled(command), button).clicked() {
                            action = Some(command);
                        }
                    }
                    ui.menu_button("More…", |ui| {
                        for command in AquaAction::ALL {
                            if ui
                                .add_enabled(
                                    state.enabled(command),
                                    egui::Button::new(command.display_label(state)),
                                )
                                .clicked()
                            {
                                action = Some(command);
                                ui.close();
                            }
                        }
                    });
                });
            } else {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    for command in AquaAction::DISPLAY {
                        if dock_tile(ui, theme, state, command) {
                            action = Some(command);
                        }
                    }
                    ui.menu_button("More…", |ui| {
                        for command in [
                            AquaAction::Cut,
                            AquaAction::Discover,
                            AquaAction::Refresh,
                            AquaAction::Transfers,
                        ] {
                            if ui
                                .add_enabled(
                                    state.enabled(command),
                                    egui::Button::new(command.display_label(state)),
                                )
                                .clicked()
                            {
                                action = Some(command);
                                ui.close();
                            }
                        }
                    });
                });
            }
        });
    action
}

fn dock_tile(ui: &mut egui::Ui, theme: &Theme, state: AquaContext, action: AquaAction) -> bool {
    let enabled = state.enabled(action);
    let (rect, response) = ui
        .add_enabled_ui(enabled, |ui| {
            ui.allocate_exact_size(Vec2::new(73.0, 60.0), Sense::click())
        })
        .inner;
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, enabled, action.display_label(state))
    });
    let paint = ui.painter().with_clip_rect(rect);
    let color = if state.active(action) {
        Color32::from_rgb(204, 247, 235)
    } else if response.hovered() || response.has_focus() {
        Color32::from_rgb(235, 254, 255)
    } else {
        Color32::from_rgb(226, 249, 249)
    };
    paint.rect_filled(rect.shrink(1.0), 10, color);
    paint.rect_stroke(
        rect.shrink(1.0),
        10,
        Stroke::new(1.0, theme.palette.border()),
        egui::StrokeKind::Inside,
    );
    paint.rect_filled(
        Rect::from_min_max(
            rect.left_top() + Vec2::new(3.0, 3.0),
            Pos2::new(rect.right() - 3.0, rect.top() + 17.0),
        ),
        8,
        Color32::WHITE.gamma_multiply(0.68),
    );
    paint_action_icon(
        &paint,
        Pos2::new(rect.center().x, rect.top() + 26.0),
        action,
        enabled,
    );
    paint.text(
        Pos2::new(rect.center().x, rect.bottom() - 11.0),
        Align2::CENTER_CENTER,
        action.display_label(state),
        FontId::proportional(10.5),
        if enabled {
            theme.palette.text()
        } else {
            theme.palette.muted().gamma_multiply(0.60)
        },
    );
    let activate = response.clicked()
        || (enabled
            && response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }));
    response.on_hover_text(action.display_label(state));
    activate
}

fn paint_action_icon(p: &egui::Painter, center: Pos2, action: AquaAction, enabled: bool) {
    let alpha = if enabled { 1.0 } else { 0.4 };
    let blue = Color32::from_rgb(30, 153, 188).gamma_multiply(alpha);
    let green = Color32::from_rgb(83, 187, 124).gamma_multiply(alpha);
    let white = Color32::WHITE.gamma_multiply(alpha);
    let area = |x1: f32, y1: f32, x2: f32, y2: f32| {
        Rect::from_min_max(center + Vec2::new(x1, y1), center + Vec2::new(x2, y2))
    };
    match action {
        AquaAction::NewFolder | AquaAction::Favorite => {
            p.rect_filled(
                area(-15.0, -8.0, -1.0, -1.0),
                2,
                Color32::from_rgb(218, 168, 69).gamma_multiply(alpha),
            );
            p.rect_filled(
                area(-15.0, -4.0, 15.0, 12.0),
                3,
                Color32::from_rgb(244, 207, 99).gamma_multiply(alpha),
            );
            p.rect_filled(area(-13.0, -3.0, 13.0, 0.0), 2, white.gamma_multiply(0.6));
            if action == AquaAction::Favorite {
                p.circle_filled(center + Vec2::new(10.0, -7.0), 5.0, green);
            }
        }
        AquaAction::Copy | AquaAction::Paste | AquaAction::Rename | AquaAction::Cut => {
            p.rect_filled(area(-10.0, -12.0, 8.0, 12.0), 3, white);
            p.rect_stroke(
                area(-10.0, -12.0, 8.0, 12.0),
                3,
                Stroke::new(1.6, blue),
                egui::StrokeKind::Inside,
            );
            for y in [-5.0, 0.0, 5.0] {
                p.line_segment(
                    [center + Vec2::new(-6.0, y), center + Vec2::new(4.0, y)],
                    Stroke::new(1.4, blue),
                );
            }
            if action == AquaAction::Paste {
                p.rect_filled(area(-5.0, -15.0, 3.0, -9.0), 3, green);
            }
            if action == AquaAction::Cut {
                p.line_segment(
                    [
                        center + Vec2::new(-11.0, 9.0),
                        center + Vec2::new(12.0, -10.0),
                    ],
                    Stroke::new(2.0, green),
                );
            }
        }
        AquaAction::Trash => {
            p.rect_filled(
                area(-10.0, -8.0, 10.0, 11.0),
                3,
                Color32::from_rgb(217, 108, 100).gamma_multiply(alpha),
            );
            p.rect_filled(area(-13.0, -12.0, 13.0, -7.0), 2, blue);
        }
        AquaAction::Discover => {
            p.circle_stroke(center, 12.0, Stroke::new(2.5, blue));
            p.line_segment(
                [
                    center + Vec2::new(-11.0, 0.0),
                    center + Vec2::new(11.0, 0.0),
                ],
                Stroke::new(1.5, green),
            );
            p.line_segment(
                [
                    center + Vec2::new(0.0, -11.0),
                    center + Vec2::new(0.0, 11.0),
                ],
                Stroke::new(1.5, green),
            );
        }
        AquaAction::Refresh => {
            p.circle_stroke(center, 11.0, Stroke::new(2.5, green));
            p.circle_filled(center + Vec2::new(9.0, -8.0), 4.0, blue);
        }
        AquaAction::Tools => {
            p.circle_filled(center, 12.0, blue);
            p.circle_filled(center, 5.0, white);
        }
        AquaAction::Preview => {
            p.circle_stroke(center + Vec2::new(-3.0, -3.0), 9.0, Stroke::new(3.0, blue));
            p.line_segment(
                [center + Vec2::new(4.0, 4.0), center + Vec2::new(13.0, 13.0)],
                Stroke::new(3.0, green),
            );
        }
        _ => {
            p.rect_filled(area(-15.0, -10.0, 15.0, 11.0), 3, white);
            p.rect_stroke(
                area(-15.0, -10.0, 15.0, 11.0),
                3,
                Stroke::new(2.0, blue),
                egui::StrokeKind::Inside,
            );
            p.line_segment(
                [center + Vec2::new(0.0, -9.0), center + Vec2::new(0.0, 10.0)],
                Stroke::new(2.0, green),
            );
        }
    }
}

#[derive(Clone, Copy)]
pub enum AquaNavIcon {
    Home,
    Folder,
    Computer,
    Network,
    Favorite,
}

pub fn sidebar_link(
    ui: &mut egui::Ui,
    theme: &Theme,
    label: &str,
    selected: bool,
    icon: AquaNavIcon,
) -> bool {
    let response = sidebar_response(ui, theme, label, selected, icon);
    sidebar_activated(ui, &response)
}

pub fn sidebar_activated(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.clicked()
        || (response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }))
}

pub fn sidebar_response(
    ui: &mut egui::Ui,
    theme: &Theme,
    label: &str,
    selected: bool,
    icon: AquaNavIcon,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(85.0), 29.0),
        Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let painter = ui.painter().with_clip_rect(rect);
    if selected || response.hovered() || response.has_focus() {
        painter.rect_filled(
            rect.shrink(1.0),
            11,
            if selected {
                theme.palette.selection()
            } else {
                Color32::from_rgb(234, 252, 246)
            },
        );
        painter.rect_stroke(
            rect.shrink(1.0),
            11,
            Stroke::new(1.0, theme.palette.border()),
            egui::StrokeKind::Inside,
        );
    }
    let center = rect.left_center() + Vec2::new(17.0, 0.0);
    let cyan = Color32::from_rgb(41, 160, 187);
    let spring = Color32::from_rgb(96, 185, 119);
    match icon {
        AquaNavIcon::Home | AquaNavIcon::Favorite => {
            painter.circle_filled(center, 9.0, Color32::WHITE.gamma_multiply(0.8));
            painter.circle_filled(
                center,
                7.0,
                if matches!(icon, AquaNavIcon::Home) {
                    cyan
                } else {
                    spring
                },
            );
            painter.circle_filled(center + Vec2::new(-2.0, -3.0), 2.5, Color32::WHITE);
        }
        AquaNavIcon::Folder => {
            painter.rect_filled(
                Rect::from_center_size(center + Vec2::new(-4.0, -4.0), Vec2::new(12.0, 6.0)),
                2,
                Color32::from_rgb(216, 162, 62),
            );
            painter.rect_filled(
                Rect::from_center_size(center + Vec2::new(0.0, 2.0), Vec2::new(21.0, 13.0)),
                3,
                Color32::from_rgb(248, 208, 109),
            );
            painter.line_segment(
                [center + Vec2::new(-8.0, 0.0), center + Vec2::new(8.0, 0.0)],
                Stroke::new(1.0, Color32::WHITE),
            );
        }
        AquaNavIcon::Computer => {
            painter.rect_filled(
                Rect::from_center_size(center + Vec2::new(0.0, -3.0), Vec2::new(22.0, 14.0)),
                3,
                cyan,
            );
            painter.rect_filled(
                Rect::from_center_size(center + Vec2::new(0.0, -4.0), Vec2::new(18.0, 10.0)),
                2,
                Color32::from_rgb(215, 250, 247),
            );
            painter.line_segment(
                [center + Vec2::new(-7.0, 8.0), center + Vec2::new(7.0, 8.0)],
                Stroke::new(2.0, spring),
            );
        }
        AquaNavIcon::Network => {
            for (x, y, r) in [(-6.0, -4.0, 5.0), (6.0, -4.0, 5.0), (0.0, 7.0, 6.0)] {
                painter.circle_filled(center + Vec2::new(x, y), r, cyan);
            }
            painter.line_segment(
                [center + Vec2::new(-6.0, -4.0), center + Vec2::new(0.0, 7.0)],
                Stroke::new(1.5, spring),
            );
            painter.line_segment(
                [center + Vec2::new(6.0, -4.0), center + Vec2::new(0.0, 7.0)],
                Stroke::new(1.5, spring),
            );
        }
    }
    painter.text(
        rect.left_center() + Vec2::new(34.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        theme.palette.text(),
    );
    response.on_hover_text(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_is_labeled_and_compact_actions_are_available() {
        for action in AquaAction::ALL {
            assert!(!action.label().is_empty());
        }
        for action in AquaAction::QUICK {
            assert!(AquaAction::ALL.contains(&action));
        }
    }

    #[test]
    fn remote_and_empty_selection_disable_local_mutations() {
        let state = AquaContext {
            local: false,
            selected: 0,
            pinned: false,
            grid: false,
            split: false,
            advanced: false,
        };
        for command in [
            AquaAction::NewFolder,
            AquaAction::Copy,
            AquaAction::Paste,
            AquaAction::Trash,
            AquaAction::Rename,
            AquaAction::Split,
        ] {
            assert!(!state.enabled(command));
        }
        assert!(state.enabled(AquaAction::Discover));
        assert!(state.enabled(AquaAction::ToggleView));
    }

    #[test]
    fn toggle_labels_and_selection_state_follow_real_context() {
        let mut state = AquaContext {
            local: true,
            selected: 0,
            pinned: false,
            grid: false,
            split: false,
            advanced: false,
        };
        assert_eq!(AquaAction::ToggleView.display_label(state), "Grid view");
        assert!(!state.enabled(AquaAction::Preview));
        state.grid = true;
        state.pinned = true;
        state.selected = 1;
        assert_eq!(AquaAction::ToggleView.display_label(state), "List view");
        assert_eq!(AquaAction::Favorite.display_label(state), "Unpin");
        assert!(state.enabled(AquaAction::Preview));
        state.selected = 2;
        assert!(!state.enabled(AquaAction::Preview));
        assert!(state.enabled(AquaAction::Copy));
    }
}
