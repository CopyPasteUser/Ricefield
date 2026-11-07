//! ChromeOS Files-inspired presentation widgets.
//!
//! Rendering returns semantic actions; Explorer owns navigation, file safety,
//! network read-only rules, and the persistent user configuration.

use crate::theme::Theme;
use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesAction {
    Back,
    Forward,
    Parent,
    Grid,
    List,
    NewFolder,
    Refresh,
    Copy,
    Cut,
    Paste,
    Rename,
    Trash,
    Info,
    Preview,
    Pin,
    Split,
    Sidebar,
    Hidden,
    Network,
    Connect,
    Transfers,
    Tools,
}

#[derive(Clone, Copy)]
pub struct FilesContext {
    pub local: bool,
    pub selection: usize,
    pub back: bool,
    pub forward: bool,
    pub grid: bool,
    pub split: bool,
    pub sidebar: bool,
    pub hidden: bool,
    pub pinned: bool,
    pub tools: bool,
    pub volume_busy: bool,
}

impl FilesContext {
    pub fn enabled(self, action: FilesAction) -> bool {
        match action {
            FilesAction::Back => self.back,
            FilesAction::Forward => self.forward,
            FilesAction::NewFolder | FilesAction::Paste | FilesAction::Pin | FilesAction::Split => {
                self.local
            }
            FilesAction::Copy | FilesAction::Cut | FilesAction::Rename | FilesAction::Trash => {
                self.local && self.selection > 0
            }
            FilesAction::Info | FilesAction::Preview => self.local && self.selection == 1,
            FilesAction::Connect => !self.volume_busy,
            _ => true,
        }
    }

    fn active(self, action: FilesAction) -> bool {
        match action {
            FilesAction::Grid => self.grid,
            FilesAction::List => !self.grid,
            FilesAction::Split => self.split,
            FilesAction::Sidebar => self.sidebar,
            FilesAction::Hidden => self.hidden,
            FilesAction::Pin => self.pinned,
            FilesAction::Tools => self.tools,
            _ => false,
        }
    }

    fn label(self, action: FilesAction) -> &'static str {
        match action {
            FilesAction::Back => "Go back",
            FilesAction::Forward => "Go forward",
            FilesAction::Parent => "Parent folder",
            FilesAction::Grid => "Grid view",
            FilesAction::List => "List view",
            FilesAction::NewFolder => "New folder",
            FilesAction::Refresh => "Refresh",
            FilesAction::Copy => "Copy",
            FilesAction::Cut => "Cut",
            FilesAction::Paste => "Paste",
            FilesAction::Rename => "Rename",
            FilesAction::Trash => "Move to Trash",
            FilesAction::Info => "File information",
            FilesAction::Preview => "Preview file",
            FilesAction::Pin if self.pinned => "Remove from shortcuts",
            FilesAction::Pin => "Add shortcut",
            FilesAction::Split => "Split view",
            FilesAction::Sidebar if self.sidebar => "Hide navigation",
            FilesAction::Sidebar => "Show navigation",
            FilesAction::Hidden if self.hidden => "Hide hidden files",
            FilesAction::Hidden => "Show hidden files",
            FilesAction::Network => "Discover network devices",
            FilesAction::Connect => "Connect to server",
            FilesAction::Transfers => "Transfers & history",
            FilesAction::Tools if self.tools => "Hide advanced tools",
            FilesAction::Tools => "Show advanced tools",
        }
    }
}

/// Familiar ChromeOS Files controls: clean pill actions and a compact menu,
/// distinct from Finder's chrome and Explorer's ribbon.
pub fn toolbar(
    ui: &mut egui::Ui,
    theme: &Theme,
    title: &str,
    state: FilesContext,
) -> Option<FilesAction> {
    let mut selected = None;
    let compact = ui.available_width() < 610.0;
    egui::Frame::new()
        .fill(theme.palette.chrome())
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (action, glyph) in [(FilesAction::Back, "‹"), (FilesAction::Forward, "›")] {
                    if ui
                        .add_enabled(
                            state.enabled(action),
                            egui::Button::new(egui::RichText::new(glyph).size(19.0))
                                .min_size(Vec2::new(31.0, 31.0))
                                .corner_radius(16)
                                .fill(Color32::TRANSPARENT),
                        )
                        .on_hover_text(state.label(action))
                        .clicked()
                    {
                        selected = Some(action);
                    }
                }
                ui.separator();
                ui.label(
                    egui::RichText::new("Files")
                        .size(17.0)
                        .strong()
                        .color(theme.palette.text()),
                );
                ui.label(
                    egui::RichText::new(short_title(title, if compact { 12 } else { 36 }))
                        .size(13.0)
                        .color(theme.palette.muted()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    more_menu(ui, state, &mut selected);
                    if !compact {
                        if toggle(ui, theme, state, FilesAction::List, "☰") {
                            selected = Some(FilesAction::List);
                        }
                        if toggle(ui, theme, state, FilesAction::Grid, "▦") {
                            selected = Some(FilesAction::Grid);
                        }
                        ui.separator();
                        if ui
                            .add_enabled(
                                state.local,
                                egui::Button::new("+ New folder")
                                    .corner_radius(16)
                                    .fill(theme.palette.selection()),
                            )
                            .clicked()
                        {
                            selected = Some(FilesAction::NewFolder);
                        }
                    }
                });
            });
            if compact {
                ui.horizontal(|ui| {
                    for (action, glyph) in [(FilesAction::Grid, "▦"), (FilesAction::List, "☰")]
                    {
                        if toggle(ui, theme, state, action, glyph) {
                            selected = Some(action);
                        }
                    }
                    if ui
                        .add_enabled(
                            state.local,
                            egui::Button::new("+ New folder")
                                .corner_radius(16)
                                .fill(theme.palette.selection()),
                        )
                        .clicked()
                    {
                        selected = Some(FilesAction::NewFolder);
                    }
                });
            }
        });
    selected
}

fn short_title(title: &str, max_len: usize) -> String {
    if title.chars().count() <= max_len {
        title.to_owned()
    } else {
        format!(
            "{}…",
            title
                .chars()
                .take(max_len.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn toggle(
    ui: &mut egui::Ui,
    theme: &Theme,
    state: FilesContext,
    action: FilesAction,
    glyph: &str,
) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(glyph).size(15.0))
            .selected(state.active(action))
            .min_size(Vec2::new(33.0, 31.0))
            .corner_radius(16)
            .fill(if state.active(action) {
                theme.palette.selection()
            } else {
                theme.palette.chrome()
            }),
    )
    .on_hover_text(state.label(action))
    .clicked()
}

pub fn secondary_commands(ui: &mut egui::Ui, state: FilesContext) -> Option<FilesAction> {
    let mut selected = None;
    ui.horizontal_wrapped(|ui| {
        for action in [
            FilesAction::NewFolder,
            FilesAction::Copy,
            FilesAction::Paste,
            FilesAction::Rename,
            FilesAction::Info,
            FilesAction::Refresh,
        ] {
            if ui
                .add_enabled(
                    state.enabled(action),
                    egui::Button::new(state.label(action)),
                )
                .clicked()
            {
                selected = Some(action);
            }
        }
    });
    selected
}

fn more_menu(ui: &mut egui::Ui, state: FilesContext, selected: &mut Option<FilesAction>) {
    ui.menu_button("⋮", |ui| {
        ui.set_min_width(192.0);
        for (i, group) in [
            &[
                FilesAction::Parent,
                FilesAction::Refresh,
                FilesAction::NewFolder,
                FilesAction::Pin,
            ][..],
            &[
                FilesAction::Cut,
                FilesAction::Copy,
                FilesAction::Paste,
                FilesAction::Rename,
                FilesAction::Trash,
                FilesAction::Info,
                FilesAction::Preview,
            ][..],
            &[
                FilesAction::Grid,
                FilesAction::List,
                FilesAction::Split,
                FilesAction::Sidebar,
                FilesAction::Hidden,
                FilesAction::Tools,
            ][..],
            &[
                FilesAction::Network,
                FilesAction::Connect,
                FilesAction::Transfers,
            ][..],
        ]
        .iter()
        .enumerate()
        {
            if i > 0 {
                ui.separator();
            }
            for &action in *group {
                if ui
                    .add_enabled(
                        state.enabled(action),
                        egui::Button::new(state.label(action)).selected(state.active(action)),
                    )
                    .clicked()
                {
                    *selected = Some(action);
                    ui.close();
                }
            }
        }
    });
}

#[derive(Clone, Copy)]
pub enum FilesNavIcon {
    Home,
    Folder,
    Download,
    Image,
    Music,
    Video,
    Network,
    Drive,
    Shortcut,
}

pub fn sidebar_link(
    ui: &mut egui::Ui,
    theme: &Theme,
    label: &str,
    active: bool,
    icon: FilesNavIcon,
) -> bool {
    let response = sidebar_response(ui, theme, label, active, icon);
    activated(ui, &response)
}

pub fn activated(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.clicked()
        || (response.has_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space)))
}

pub fn sidebar_response(
    ui: &mut egui::Ui,
    theme: &Theme,
    label: &str,
    active: bool,
    icon: FilesNavIcon,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(70.0), 32.0),
        Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let p = ui.painter().with_clip_rect(rect);
    if active || response.hovered() || response.has_focus() {
        p.rect_filled(
            rect.shrink(1.0),
            16,
            if active {
                theme.palette.selection()
            } else {
                Color32::from_rgb(232, 238, 249)
            },
        );
    }
    let ink = if active {
        theme.palette.accent()
    } else {
        Color32::from_rgb(101, 114, 136)
    };
    paint_nav_icon(&p, rect.left_center() + Vec2::new(18.0, 0.0), icon, ink);
    p.text(
        rect.left_center() + Vec2::new(35.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        theme.palette.text(),
    );
    response.on_hover_text(label)
}

fn paint_nav_icon(p: &Painter, c: Pos2, icon: FilesNavIcon, ink: Color32) {
    let at = |x: f32, y: f32| c + Vec2::new(x, y);
    match icon {
        FilesNavIcon::Home => {
            p.add(egui::Shape::convex_polygon(
                vec![at(-8.0, -1.0), at(0.0, -8.0), at(8.0, -1.0)],
                ink,
                Stroke::NONE,
            ));
            p.rect_filled(Rect::from_min_max(at(-6.0, -1.0), at(6.0, 8.0)), 1, ink);
        }
        FilesNavIcon::Folder | FilesNavIcon::Download | FilesNavIcon::Shortcut => {
            p.rect_filled(Rect::from_min_max(at(-8.0, -5.0), at(0.0, -1.0)), 2, ink);
            p.rect_filled(Rect::from_min_max(at(-8.0, -2.0), at(8.0, 7.0)), 3, ink);
            if matches!(icon, FilesNavIcon::Download) {
                p.line_segment(
                    [at(0.0, -5.0), at(0.0, 4.0)],
                    Stroke::new(1.5, Color32::WHITE),
                );
                p.line_segment(
                    [at(-3.0, 1.0), at(0.0, 4.0)],
                    Stroke::new(1.5, Color32::WHITE),
                );
                p.line_segment(
                    [at(3.0, 1.0), at(0.0, 4.0)],
                    Stroke::new(1.5, Color32::WHITE),
                );
            }
        }
        FilesNavIcon::Image => {
            p.rect_stroke(
                Rect::from_min_max(at(-8.0, -7.0), at(8.0, 7.0)),
                2,
                Stroke::new(1.6, ink),
                egui::StrokeKind::Inside,
            );
            p.circle_filled(at(4.0, -3.0), 2.0, ink);
            p.line_segment([at(-6.0, 5.0), at(0.0, -2.0)], Stroke::new(1.7, ink));
            p.line_segment([at(0.0, -2.0), at(6.0, 5.0)], Stroke::new(1.7, ink));
        }
        FilesNavIcon::Music => {
            p.line_segment([at(2.0, -7.0), at(2.0, 4.0)], Stroke::new(2.0, ink));
            p.line_segment([at(2.0, -7.0), at(8.0, -8.0)], Stroke::new(2.0, ink));
            p.circle_filled(at(-1.0, 5.0), 3.0, ink);
        }
        FilesNavIcon::Video => {
            p.rect_stroke(
                Rect::from_min_max(at(-8.0, -6.0), at(8.0, 6.0)),
                3,
                Stroke::new(1.6, ink),
                egui::StrokeKind::Inside,
            );
            p.add(egui::Shape::convex_polygon(
                vec![at(-2.0, -3.0), at(4.0, 0.0), at(-2.0, 3.0)],
                ink,
                Stroke::NONE,
            ));
        }
        FilesNavIcon::Network => {
            p.circle_stroke(c, 7.0, Stroke::new(1.5, ink));
            p.line_segment([at(-7.0, 0.0), at(7.0, 0.0)], Stroke::new(1.3, ink));
            p.line_segment([at(0.0, -7.0), at(0.0, 7.0)], Stroke::new(1.3, ink));
        }
        FilesNavIcon::Drive => {
            p.rect_stroke(
                Rect::from_min_max(at(-8.0, -5.0), at(8.0, 6.0)),
                3,
                Stroke::new(1.8, ink),
                egui::StrokeKind::Inside,
            );
            p.circle_filled(at(4.0, 3.0), 1.5, ink);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> FilesContext {
        FilesContext {
            local: true,
            selection: 0,
            back: false,
            forward: false,
            grid: false,
            split: false,
            sidebar: true,
            hidden: false,
            pinned: false,
            tools: false,
            volume_busy: false,
        }
    }

    #[test]
    fn remote_entries_do_not_enable_local_writes() {
        let mut state = context();
        state.selection = 1;
        assert!(state.enabled(FilesAction::Copy));
        assert!(state.enabled(FilesAction::Info));
        state.local = false;
        for action in [
            FilesAction::NewFolder,
            FilesAction::Paste,
            FilesAction::Copy,
            FilesAction::Cut,
            FilesAction::Trash,
            FilesAction::Rename,
            FilesAction::Info,
        ] {
            assert!(!state.enabled(action));
        }
        assert!(state.enabled(FilesAction::Network));
    }

    #[test]
    fn shortcut_and_sidebar_labels_reflect_state() {
        let mut state = context();
        assert_eq!(state.label(FilesAction::Pin), "Add shortcut");
        state.pinned = true;
        assert_eq!(state.label(FilesAction::Pin), "Remove from shortcuts");
        assert_eq!(state.label(FilesAction::Sidebar), "Hide navigation");
        assert_eq!(short_title("Documents", 5), "Docu…");
    }
}
