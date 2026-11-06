//! Windows 10-inspired File Explorer ribbon, using original vector artwork.
//!
//! This module owns only controls and layout. Every returned command is
//! dispatched by `Explorer`, so file safety and navigation stay skin-neutral.

use crate::theme::Theme;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RibbonPage {
    #[default]
    Home,
    Share,
    View,
}

impl RibbonPage {
    const ALL: [Self; 3] = [Self::Home, Self::Share, Self::View];

    fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Share => "Share",
            Self::View => "View",
        }
    }

    fn groups(self) -> &'static [(&'static str, &'static [RibbonAction])] {
        match self {
            Self::Home => &[
                (
                    "Clipboard",
                    &[
                        RibbonAction::Paste,
                        RibbonAction::Cut,
                        RibbonAction::Copy,
                        RibbonAction::CopyPath,
                    ],
                ),
                (
                    "Organize",
                    &[
                        RibbonAction::Rename,
                        RibbonAction::Trash,
                        RibbonAction::Properties,
                    ],
                ),
                (
                    "New",
                    &[
                        RibbonAction::NewFolder,
                        RibbonAction::NewFile,
                        RibbonAction::Favorite,
                    ],
                ),
                (
                    "Select",
                    &[
                        RibbonAction::SelectAll,
                        RibbonAction::SelectNone,
                        RibbonAction::InvertSelection,
                    ],
                ),
            ],
            Self::Share => &[
                (
                    "Send",
                    &[
                        RibbonAction::CopyPath,
                        RibbonAction::Archive,
                        RibbonAction::Copy,
                    ],
                ),
                (
                    "Access",
                    &[
                        RibbonAction::Properties,
                        RibbonAction::FindNetwork,
                        RibbonAction::ConnectNetwork,
                    ],
                ),
                ("History", &[RibbonAction::Transfers]),
            ],
            Self::View => &[
                ("Layout", &[RibbonAction::Details, RibbonAction::LargeIcons]),
                (
                    "Panes",
                    &[
                        RibbonAction::NavigationPane,
                        RibbonAction::PreviewPane,
                        RibbonAction::SplitView,
                    ],
                ),
                (
                    "Show / hide",
                    &[
                        RibbonAction::HiddenItems,
                        RibbonAction::MoreTools,
                        RibbonAction::Refresh,
                    ],
                ),
            ],
        }
    }

    fn quick(self) -> &'static [RibbonAction] {
        match self {
            Self::Home => &[
                RibbonAction::Paste,
                RibbonAction::Copy,
                RibbonAction::NewFolder,
                RibbonAction::Rename,
                RibbonAction::Trash,
            ],
            Self::Share => &[
                RibbonAction::CopyPath,
                RibbonAction::Archive,
                RibbonAction::FindNetwork,
            ],
            Self::View => &[
                RibbonAction::Details,
                RibbonAction::LargeIcons,
                RibbonAction::NavigationPane,
                RibbonAction::HiddenItems,
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RibbonAction {
    NewWindow,
    NewTab,
    NewFolder,
    NewFile,
    Cut,
    Copy,
    Paste,
    CopyPath,
    Rename,
    Trash,
    Properties,
    Favorite,
    SelectAll,
    SelectNone,
    InvertSelection,
    Archive,
    FindNetwork,
    ConnectNetwork,
    Transfers,
    Details,
    LargeIcons,
    NavigationPane,
    PreviewPane,
    SplitView,
    HiddenItems,
    MoreTools,
    Refresh,
}

impl RibbonAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::NewWindow => "New window",
            Self::NewTab => "New tab",
            Self::NewFolder => "New folder",
            Self::NewFile => "New file",
            Self::Cut => "Cut",
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::CopyPath => "Copy path",
            Self::Rename => "Rename",
            Self::Trash => "Delete",
            Self::Properties => "Properties",
            Self::Favorite => "Pin folder",
            Self::SelectAll => "Select all",
            Self::SelectNone => "Select none",
            Self::InvertSelection => "Invert",
            Self::Archive => "Compress",
            Self::FindNetwork => "Find network",
            Self::ConnectNetwork => "Connect",
            Self::Transfers => "Transfers",
            Self::Details => "Details",
            Self::LargeIcons => "Large icons",
            Self::NavigationPane => "Nav pane",
            Self::PreviewPane => "Preview",
            Self::SplitView => "Split view",
            Self::HiddenItems => "Hidden items",
            Self::MoreTools => "More tools",
            Self::Refresh => "Refresh",
        }
    }

    fn needs_selection(self) -> bool {
        matches!(
            self,
            Self::Cut
                | Self::Copy
                | Self::CopyPath
                | Self::Rename
                | Self::Trash
                | Self::Properties
                | Self::Archive
                | Self::PreviewPane
        )
    }

    fn needs_local(self) -> bool {
        matches!(
            self,
            Self::NewFolder
                | Self::NewFile
                | Self::Cut
                | Self::Copy
                | Self::Paste
                | Self::CopyPath
                | Self::Rename
                | Self::Trash
                | Self::Properties
                | Self::Favorite
                | Self::SelectAll
                | Self::SelectNone
                | Self::InvertSelection
                | Self::Archive
                | Self::PreviewPane
                | Self::SplitView
        )
    }
}

#[derive(Clone, Copy)]
pub struct RibbonContext {
    pub local: bool,
    pub selected: usize,
    pub has_items: bool,
    pub show_sidebar: bool,
    pub show_hidden: bool,
    pub split: bool,
    pub preview: bool,
    pub grid: bool,
    pub advanced: bool,
    pub volume_busy: bool,
}

impl RibbonContext {
    fn enabled(self, action: RibbonAction) -> bool {
        (!action.needs_local() || self.local)
            && (!action.needs_selection() || self.selected > 0)
            && (!matches!(
                action,
                RibbonAction::Rename | RibbonAction::Properties | RibbonAction::PreviewPane
            ) || self.selected == 1)
            && (!matches!(action, RibbonAction::SelectAll) || self.has_items)
            && (!matches!(action, RibbonAction::SelectNone) || self.selected > 0)
            && (!matches!(action, RibbonAction::InvertSelection) || self.has_items)
            && (!matches!(action, RibbonAction::ConnectNetwork) || !self.volume_busy)
    }

    fn active(self, action: RibbonAction) -> bool {
        match action {
            RibbonAction::Details => !self.grid,
            RibbonAction::LargeIcons => self.grid,
            RibbonAction::NavigationPane => self.show_sidebar,
            RibbonAction::PreviewPane => self.preview,
            RibbonAction::HiddenItems => self.show_hidden,
            RibbonAction::SplitView => self.split,
            RibbonAction::MoreTools => self.advanced,
            _ => false,
        }
    }
}

/// Draw a Windows 10-style tabbed ribbon, returning at most one semantic event.
pub fn ribbon(
    ui: &mut egui::Ui,
    theme: &Theme,
    page: &mut RibbonPage,
    expanded: &mut bool,
    state: RibbonContext,
) -> Option<RibbonAction> {
    let mut action = None;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.horizontal(|ui| {
            ui.scope(|ui| {
                ui.visuals_mut().widgets.inactive.bg_fill = theme.palette.accent();
                ui.visuals_mut().widgets.hovered.bg_fill = theme.palette.accent();
                ui.menu_button(
                    egui::RichText::new("File").color(Color32::WHITE).strong(),
                    |ui| {
                        for command in [
                            RibbonAction::NewWindow,
                            RibbonAction::NewTab,
                            RibbonAction::NewFolder,
                            RibbonAction::Properties,
                            RibbonAction::Transfers,
                            RibbonAction::MoreTools,
                        ] {
                            if ui
                                .add_enabled(
                                    state.enabled(command),
                                    egui::Button::new(command.label()),
                                )
                                .clicked()
                            {
                                action = Some(command);
                                ui.close();
                            }
                        }
                    },
                );
            });
            for target in RibbonPage::ALL {
                let selected = *page == target;
                let button = egui::Button::new(
                    egui::RichText::new(target.label())
                        .color(if selected {
                            theme.palette.accent()
                        } else {
                            theme.palette.text()
                        })
                        .strong(),
                )
                .fill(if selected {
                    theme.palette.panel()
                } else {
                    theme.palette.chrome()
                })
                .stroke(if selected {
                    Stroke::new(1.0, theme.palette.border())
                } else {
                    Stroke::NONE
                });
                if ui.add_sized([62.0, 26.0], button).clicked() {
                    *page = target;
                    *expanded = true;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button(if *expanded { "^" } else { "v" })
                    .on_hover_text(if *expanded {
                        "Collapse ribbon"
                    } else {
                        "Expand ribbon"
                    })
                    .clicked()
                {
                    *expanded = !*expanded;
                }
            });
        });
        if !*expanded {
            return;
        }
        egui::Frame::new()
            .fill(theme.palette.panel())
            .stroke(Stroke::new(1.0, theme.palette.border()))
            .inner_margin(egui::Margin::symmetric(7, 5))
            .show(ui, |ui| {
                if ui.available_width() < 890.0 {
                    ui.horizontal_wrapped(|ui| {
                        for &command in page.quick() {
                            let button =
                                egui::Button::new(command.label()).selected(state.active(command));
                            if ui.add_enabled(state.enabled(command), button).clicked() {
                                action = Some(command);
                            }
                        }
                        ui.menu_button("More...", |ui| {
                            for (_, commands) in page.groups() {
                                for &command in *commands {
                                    if ui
                                        .add_enabled(
                                            state.enabled(command),
                                            egui::Button::new(command.label()),
                                        )
                                        .clicked()
                                    {
                                        action = Some(command);
                                        ui.close();
                                    }
                                }
                            }
                        });
                    });
                } else {
                    ui.horizontal(|ui| {
                        for (index, &(heading, commands)) in page.groups().iter().enumerate() {
                            if index > 0 {
                                ui.separator();
                            }
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 1.0;
                                    for &command in commands {
                                        if ribbon_tile(ui, command, state, theme) {
                                            action = Some(command);
                                        }
                                    }
                                });
                                ui.label(
                                    egui::RichText::new(heading)
                                        .size(10.5)
                                        .color(theme.palette.muted()),
                                );
                            });
                        }
                    });
                }
            });
    });
    action
}

fn ribbon_tile(
    ui: &mut egui::Ui,
    action: RibbonAction,
    state: RibbonContext,
    theme: &Theme,
) -> bool {
    let enabled = state.enabled(action);
    let (rect, response) = ui
        .add_enabled_ui(enabled, |ui| {
            ui.allocate_exact_size(Vec2::new(72.0, 62.0), Sense::click())
        })
        .inner;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, action.label()));
    let painter = ui.painter().with_clip_rect(rect);
    if state.active(action) || response.hovered() || response.has_focus() {
        painter.rect_filled(rect.shrink(1.0), 1, theme.palette.selection());
        painter.rect_stroke(
            rect.shrink(1.0),
            1,
            Stroke::new(1.0, theme.palette.accent()),
            egui::StrokeKind::Inside,
        );
    }
    let icon = Pos2::new(rect.center().x, rect.top() + 21.0);
    paint_ribbon_icon(&painter, icon, action, theme.palette.accent(), enabled);
    painter.text(
        Pos2::new(rect.center().x, rect.bottom() - 12.0),
        Align2::CENTER_CENTER,
        action.label(),
        FontId::proportional(11.0),
        if enabled {
            theme.palette.text()
        } else {
            theme.palette.muted().gamma_multiply(0.55)
        },
    );
    let activated = response.clicked()
        || (enabled
            && response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }));
    response.on_hover_text(action.label());
    activated
}

fn paint_ribbon_icon(
    p: &egui::Painter,
    center: Pos2,
    action: RibbonAction,
    accent: Color32,
    enabled: bool,
) {
    let alpha = if enabled { 1.0 } else { 0.35 };
    let ink = accent.gamma_multiply(alpha);
    let gray = Color32::from_rgb(107, 120, 132).gamma_multiply(alpha);
    let rect = |x1: f32, y1: f32, x2: f32, y2: f32| {
        Rect::from_min_max(center + Vec2::new(x1, y1), center + Vec2::new(x2, y2))
    };
    match action {
        RibbonAction::NewFolder | RibbonAction::Favorite => {
            p.rect_filled(
                rect(-14.0, -5.0, 0.0, 1.0),
                1,
                Color32::from_rgb(225, 172, 59).gamma_multiply(alpha),
            );
            p.rect_filled(
                rect(-14.0, -1.0, 15.0, 12.0),
                2,
                Color32::from_rgb(243, 201, 92).gamma_multiply(alpha),
            );
            p.line_segment(
                [
                    center + Vec2::new(-12.0, 2.0),
                    center + Vec2::new(12.0, 2.0),
                ],
                Stroke::new(1.0, Color32::WHITE.gamma_multiply(alpha)),
            );
            if action == RibbonAction::Favorite {
                p.circle_filled(center + Vec2::new(9.0, -6.0), 5.0, ink);
            }
        }
        RibbonAction::Cut => {
            for x in [-8.0, 7.0] {
                p.circle_stroke(center + Vec2::new(x, 7.0), 5.0, Stroke::new(2.0, gray));
            }
            p.line_segment(
                [
                    center + Vec2::new(-4.0, 4.0),
                    center + Vec2::new(10.0, -10.0),
                ],
                Stroke::new(2.0, ink),
            );
            p.line_segment(
                [
                    center + Vec2::new(3.0, 4.0),
                    center + Vec2::new(-10.0, -10.0),
                ],
                Stroke::new(2.0, ink),
            );
        }
        RibbonAction::Trash => {
            p.rect_filled(
                rect(-10.0, -7.0, 10.0, 11.0),
                2,
                Color32::from_rgb(221, 95, 78).gamma_multiply(alpha),
            );
            p.rect_filled(rect(-13.0, -10.0, 13.0, -6.0), 1, gray);
            for x in [-4.0, 3.0] {
                p.line_segment(
                    [center + Vec2::new(x, -3.0), center + Vec2::new(x, 7.0)],
                    Stroke::new(1.5, Color32::WHITE.gamma_multiply(alpha)),
                );
            }
        }
        RibbonAction::Paste => {
            p.rect_filled(rect(-10.0, -9.0, 12.0, 12.0), 2, gray);
            p.rect_filled(
                rect(-7.0, -5.0, 9.0, 9.0),
                1,
                Color32::WHITE.gamma_multiply(alpha),
            );
            p.rect_filled(rect(-5.0, -13.0, 7.0, -6.0), 2, ink);
        }
        RibbonAction::Copy
        | RibbonAction::CopyPath
        | RibbonAction::NewFile
        | RibbonAction::Properties
        | RibbonAction::Archive => {
            p.rect_filled(
                rect(-8.0, -10.0, 10.0, 12.0),
                1,
                Color32::WHITE.gamma_multiply(alpha),
            );
            p.rect_stroke(
                rect(-8.0, -10.0, 10.0, 12.0),
                1,
                Stroke::new(2.0, ink),
                egui::StrokeKind::Inside,
            );
            for y in [-3.0, 2.0, 7.0] {
                p.line_segment(
                    [center + Vec2::new(-4.0, y), center + Vec2::new(6.0, y)],
                    Stroke::new(1.4, ink),
                );
            }
            if matches!(action, RibbonAction::Copy | RibbonAction::CopyPath) {
                p.rect_stroke(
                    rect(-13.0, -5.0, 5.0, 14.0),
                    1,
                    Stroke::new(1.5, gray),
                    egui::StrokeKind::Inside,
                );
            }
            if action == RibbonAction::Archive {
                p.rect_filled(
                    rect(5.0, -11.0, 9.0, 13.0),
                    0,
                    Color32::from_rgb(211, 166, 55).gamma_multiply(alpha),
                );
            }
        }
        RibbonAction::FindNetwork | RibbonAction::ConnectNetwork | RibbonAction::Transfers => {
            for (x, y) in [(-9.0, -6.0), (9.0, -6.0), (0.0, 10.0)] {
                p.circle_filled(center + Vec2::new(x, y), 4.5, ink);
            }
            for (a, b) in [((-9.0, -6.0), (0.0, 10.0)), ((9.0, -6.0), (0.0, 10.0))] {
                p.line_segment(
                    [center + Vec2::new(a.0, a.1), center + Vec2::new(b.0, b.1)],
                    Stroke::new(1.5, gray),
                );
            }
        }
        RibbonAction::Refresh => {
            p.circle_stroke(center, 10.0, Stroke::new(2.5, ink));
            p.add(egui::Shape::convex_polygon(
                vec![
                    center + Vec2::new(5.0, -12.0),
                    center + Vec2::new(13.0, -11.0),
                    center + Vec2::new(9.0, -4.0),
                ],
                ink,
                Stroke::NONE,
            ));
        }
        _ => {
            p.rect_stroke(
                rect(-13.0, -10.0, 13.0, 11.0),
                1,
                Stroke::new(1.8, ink),
                egui::StrokeKind::Inside,
            );
            p.line_segment(
                [
                    center + Vec2::new(-5.0, -9.0),
                    center + Vec2::new(-5.0, 10.0),
                ],
                Stroke::new(1.7, gray),
            );
            p.line_segment(
                [center + Vec2::new(0.0, -2.0), center + Vec2::new(9.0, -2.0)],
                Stroke::new(1.6, gray),
            );
            p.line_segment(
                [center + Vec2::new(0.0, 3.0), center + Vec2::new(9.0, 3.0)],
                Stroke::new(1.6, gray),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ribbon_pages_expose_real_commands_without_duplicate_actions() {
        for page in RibbonPage::ALL {
            let actions = page
                .groups()
                .iter()
                .flat_map(|(_, items)| items.iter().copied())
                .collect::<Vec<_>>();
            assert!(!actions.is_empty());
            for (index, action) in actions.iter().enumerate() {
                assert!(!action.label().is_empty());
                assert!(!actions[..index].contains(action));
            }
            for action in page.quick() {
                assert!(actions.contains(action));
            }
        }
    }

    #[test]
    fn remote_and_empty_selection_never_enable_mutating_ribbon_actions() {
        let state = RibbonContext {
            local: false,
            selected: 0,
            has_items: false,
            show_sidebar: true,
            show_hidden: false,
            split: false,
            preview: false,
            grid: false,
            advanced: false,
            volume_busy: false,
        };
        for action in [
            RibbonAction::NewFolder,
            RibbonAction::Cut,
            RibbonAction::Copy,
            RibbonAction::Paste,
            RibbonAction::Rename,
            RibbonAction::Trash,
            RibbonAction::Archive,
            RibbonAction::SplitView,
        ] {
            assert!(!state.enabled(action), "{}", action.label());
        }
        assert!(state.enabled(RibbonAction::FindNetwork));
        assert!(state.enabled(RibbonAction::Details));
    }
}
