//! Windows 98-era Explorer presentation, using original vector primitives.
//!
//! The shell paints old-fashioned beveled toolbars and emits semantic actions;
//! filesystem mutations, safety gates and browser history remain in Explorer.

use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicAction {
    Back,
    Forward,
    Up,
    NewFolder,
    Copy,
    Cut,
    Paste,
    Rename,
    Delete,
    Info,
    Grid,
    List,
    Preview,
    Favorites,
    Split,
    Hidden,
    Sidebar,
    Refresh,
    Network,
    Connect,
    Transfers,
    Tools,
}

#[derive(Clone, Copy)]
pub struct ClassicContext {
    pub local: bool,
    pub selection: usize,
    pub back: bool,
    pub forward: bool,
    pub grid: bool,
    pub split: bool,
    pub hidden: bool,
    pub sidebar: bool,
    pub pinned: bool,
    pub tools: bool,
    pub volume_busy: bool,
}

impl ClassicContext {
    pub fn enabled(self, action: ClassicAction) -> bool {
        match action {
            ClassicAction::Back => self.back,
            ClassicAction::Forward => self.forward,
            ClassicAction::NewFolder
            | ClassicAction::Paste
            | ClassicAction::Favorites
            | ClassicAction::Split => self.local,
            ClassicAction::Copy
            | ClassicAction::Cut
            | ClassicAction::Rename
            | ClassicAction::Delete => self.local && self.selection > 0,
            ClassicAction::Info | ClassicAction::Preview => self.local && self.selection == 1,
            ClassicAction::Connect => !self.volume_busy,
            _ => true,
        }
    }

    fn active(self, action: ClassicAction) -> bool {
        match action {
            ClassicAction::Grid => self.grid,
            ClassicAction::List => !self.grid,
            ClassicAction::Split => self.split,
            ClassicAction::Hidden => self.hidden,
            ClassicAction::Sidebar => self.sidebar,
            ClassicAction::Favorites => self.pinned,
            ClassicAction::Tools => self.tools,
            _ => false,
        }
    }

    fn label(self, action: ClassicAction) -> &'static str {
        match action {
            ClassicAction::Back => "Back",
            ClassicAction::Forward => "Forward",
            ClassicAction::Up => "Up",
            ClassicAction::NewFolder => "New Folder",
            ClassicAction::Copy => "Copy",
            ClassicAction::Cut => "Cut",
            ClassicAction::Paste => "Paste",
            ClassicAction::Rename => "Rename",
            ClassicAction::Delete => "Delete",
            ClassicAction::Info => "Properties",
            ClassicAction::Grid => "Large Icons",
            ClassicAction::List => "Details",
            ClassicAction::Preview => "Preview",
            ClassicAction::Favorites if self.pinned => "Remove Favorite",
            ClassicAction::Favorites => "Add Favorite",
            ClassicAction::Split => "Two Panes",
            ClassicAction::Hidden if self.hidden => "Hide Hidden Files",
            ClassicAction::Hidden => "Show Hidden Files",
            ClassicAction::Sidebar if self.sidebar => "Hide Folders",
            ClassicAction::Sidebar => "Show Folders",
            ClassicAction::Refresh => "Refresh",
            ClassicAction::Network => "Network Neighborhood",
            ClassicAction::Connect => "Map Network Location",
            ClassicAction::Transfers => "Transfers",
            ClassicAction::Tools if self.tools => "Hide Advanced Tools",
            ClassicAction::Tools => "Advanced Tools",
        }
    }
}

#[derive(Clone, Copy)]
enum ClassicGlyph {
    Back,
    Forward,
    Up,
    Scissors,
    Copy,
    Paste,
    Delete,
    Info,
    View,
    Folder,
}

pub fn toolbar(ui: &mut egui::Ui, state: ClassicContext) -> Option<ClassicAction> {
    let mut chosen = None;
    let compact = ui.available_width() < 730.0;
    egui::Frame::new()
        .fill(Color32::from_rgb(192, 192, 192))
        .inner_margin(egui::Margin::symmetric(5, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (action, glyph) in [
                    (ClassicAction::Back, ClassicGlyph::Back),
                    (ClassicAction::Forward, ClassicGlyph::Forward),
                    (ClassicAction::Up, ClassicGlyph::Up),
                ] {
                    if toolbar_item(ui, state, action, glyph, !compact) {
                        chosen = Some(action);
                    }
                }
                if !compact {
                    ui.separator();
                    for (action, glyph) in [
                        (ClassicAction::Cut, ClassicGlyph::Scissors),
                        (ClassicAction::Copy, ClassicGlyph::Copy),
                        (ClassicAction::Paste, ClassicGlyph::Paste),
                        (ClassicAction::Delete, ClassicGlyph::Delete),
                        (ClassicAction::Info, ClassicGlyph::Info),
                    ] {
                        if toolbar_item(ui, state, action, glyph, true) {
                            chosen = Some(action);
                        }
                    }
                    ui.separator();
                    if toolbar_item(ui, state, ClassicAction::List, ClassicGlyph::View, true) {
                        chosen = Some(ClassicAction::List);
                    }
                }
                ui.separator();
                if toolbar_item(
                    ui,
                    state,
                    ClassicAction::NewFolder,
                    ClassicGlyph::Folder,
                    !compact,
                ) {
                    chosen = Some(ClassicAction::NewFolder);
                }
                ui.menu_button("Views", |ui| {
                    ui.set_min_width(170.0);
                    for group in [
                        &[
                            ClassicAction::Grid,
                            ClassicAction::List,
                            ClassicAction::Sidebar,
                            ClassicAction::Hidden,
                            ClassicAction::Split,
                        ][..],
                        &[
                            ClassicAction::Copy,
                            ClassicAction::Cut,
                            ClassicAction::Paste,
                            ClassicAction::Rename,
                            ClassicAction::Delete,
                            ClassicAction::Info,
                            ClassicAction::Preview,
                            ClassicAction::Favorites,
                        ][..],
                        &[
                            ClassicAction::Refresh,
                            ClassicAction::Network,
                            ClassicAction::Connect,
                            ClassicAction::Transfers,
                            ClassicAction::Tools,
                        ][..],
                    ] {
                        for &action in group {
                            if ui
                                .add_enabled(
                                    state.enabled(action),
                                    egui::Button::new(state.label(action))
                                        .selected(state.active(action)),
                                )
                                .clicked()
                            {
                                chosen = Some(action);
                                ui.close();
                            }
                        }
                        ui.separator();
                    }
                });
            });
        });
    chosen
}

fn toolbar_item(
    ui: &mut egui::Ui,
    state: ClassicContext,
    action: ClassicAction,
    glyph: ClassicGlyph,
    show_label: bool,
) -> bool {
    let enabled = state.enabled(action);
    let size = Vec2::new(
        if show_label { 58.0 } else { 32.0 },
        if show_label { 47.0 } else { 31.0 },
    );
    let (_, response) = ui
        .add_enabled_ui(enabled, |ui| ui.allocate_exact_size(size, Sense::click()))
        .inner;
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, enabled, state.label(action))
    });
    let painter = ui.painter().with_clip_rect(response.rect);
    if response.hovered() || response.has_focus() || state.active(action) {
        let rect = response.rect.shrink(0.5);
        painter.line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1.0, Color32::WHITE),
        );
        painter.line_segment(
            [rect.left_top(), rect.left_bottom()],
            Stroke::new(1.0, Color32::WHITE),
        );
        painter.line_segment(
            [rect.right_top(), rect.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(79)),
        );
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(79)),
        );
    }
    let icon_center = response.rect.center() + Vec2::new(0.0, if show_label { -9.0 } else { 0.0 });
    paint_toolbar_glyph(
        &painter,
        icon_center,
        glyph,
        if enabled { 1.0 } else { 0.35 },
    );
    if show_label {
        painter.text(
            response.rect.center_bottom() + Vec2::new(0.0, -7.0),
            Align2::CENTER_CENTER,
            state.label(action),
            FontId::proportional(10.5),
            Color32::BLACK.gamma_multiply(if enabled { 1.0 } else { 0.5 }),
        );
    }
    let clicked = response.clicked()
        || (enabled
            && response.has_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space)));
    response.on_hover_text(state.label(action));
    clicked
}

fn paint_toolbar_glyph(p: &Painter, c: Pos2, glyph: ClassicGlyph, opacity: f32) {
    let blue = Color32::from_rgb(22, 89, 181).gamma_multiply(opacity);
    let yellow = Color32::from_rgb(243, 197, 47).gamma_multiply(opacity);
    let ink = Color32::from_gray(35).gamma_multiply(opacity);
    let at = |x: f32, y: f32| c + Vec2::new(x, y);
    match glyph {
        ClassicGlyph::Back | ClassicGlyph::Forward => {
            let d = if matches!(glyph, ClassicGlyph::Back) {
                -1.0
            } else {
                1.0
            };
            p.circle_filled(
                c,
                9.0,
                Color32::from_rgb(62, 142, 66).gamma_multiply(opacity),
            );
            p.add(egui::Shape::convex_polygon(
                vec![at(6.0 * d, 0.0), at(-3.0 * d, -5.0), at(-3.0 * d, 5.0)],
                Color32::WHITE.gamma_multiply(opacity),
                Stroke::NONE,
            ));
        }
        ClassicGlyph::Up => {
            p.rect_filled(Rect::from_min_max(at(-9.0, 0.0), at(9.0, 7.0)), 0, yellow);
            p.line_segment([at(0.0, 5.0), at(0.0, -8.0)], Stroke::new(3.0, blue));
            p.add(egui::Shape::convex_polygon(
                vec![at(-5.0, -4.0), at(0.0, -9.0), at(5.0, -4.0)],
                blue,
                Stroke::NONE,
            ));
        }
        ClassicGlyph::Scissors => {
            for y in [-4.0, 4.0] {
                p.circle_stroke(at(-6.0, y), 3.0, Stroke::new(1.5, ink));
            }
            p.line_segment([at(-3.0, -3.0), at(8.0, 7.0)], Stroke::new(1.7, ink));
            p.line_segment([at(-3.0, 3.0), at(8.0, -7.0)], Stroke::new(1.7, ink));
        }
        ClassicGlyph::Copy | ClassicGlyph::Paste => {
            p.rect_filled(
                Rect::from_min_max(at(-7.0, -8.0), at(4.0, 5.0)),
                0,
                Color32::WHITE.gamma_multiply(opacity),
            );
            p.rect_stroke(
                Rect::from_min_max(at(-7.0, -8.0), at(4.0, 5.0)),
                0,
                Stroke::new(1.0, blue),
                egui::StrokeKind::Inside,
            );
            p.rect_filled(
                Rect::from_min_max(at(-2.0, -3.0), at(9.0, 9.0)),
                0,
                if matches!(glyph, ClassicGlyph::Paste) {
                    yellow
                } else {
                    Color32::WHITE.gamma_multiply(opacity)
                },
            );
            p.rect_stroke(
                Rect::from_min_max(at(-2.0, -3.0), at(9.0, 9.0)),
                0,
                Stroke::new(1.0, blue),
                egui::StrokeKind::Inside,
            );
        }
        ClassicGlyph::Delete => {
            p.rect_filled(
                Rect::from_min_max(at(-6.0, -4.0), at(6.0, 8.0)),
                1,
                Color32::from_rgb(173, 181, 190).gamma_multiply(opacity),
            );
            p.line_segment([at(-8.0, -6.0), at(8.0, -6.0)], Stroke::new(2.2, ink));
        }
        ClassicGlyph::Info => {
            p.rect_filled(
                Rect::from_min_max(at(-7.0, -9.0), at(8.0, 9.0)),
                1,
                Color32::WHITE.gamma_multiply(opacity),
            );
            p.rect_stroke(
                Rect::from_min_max(at(-7.0, -9.0), at(8.0, 9.0)),
                1,
                Stroke::new(1.2, ink),
                egui::StrokeKind::Inside,
            );
            p.text(
                c,
                Align2::CENTER_CENTER,
                "i",
                FontId::proportional(14.0),
                blue,
            );
        }
        ClassicGlyph::View => {
            for y in [-6.0, 0.0, 6.0] {
                p.rect_filled(
                    Rect::from_min_max(at(-8.0, y - 2.0), at(-4.0, y + 2.0)),
                    0,
                    blue,
                );
                p.line_segment([at(-1.0, y), at(9.0, y)], Stroke::new(1.4, ink));
            }
        }
        ClassicGlyph::Folder => {
            p.rect_filled(
                Rect::from_min_max(at(-9.0, -7.0), at(-1.0, -2.0)),
                0,
                yellow,
            );
            p.rect_filled(Rect::from_min_max(at(-9.0, -3.0), at(8.0, 8.0)), 0, yellow);
            p.rect_stroke(
                Rect::from_min_max(at(-9.0, -3.0), at(8.0, 8.0)),
                0,
                Stroke::new(1.0, ink),
                egui::StrokeKind::Inside,
            );
        }
    }
}

pub fn secondary_commands(ui: &mut egui::Ui, state: ClassicContext) -> Option<ClassicAction> {
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        for action in [
            ClassicAction::NewFolder,
            ClassicAction::Copy,
            ClassicAction::Paste,
            ClassicAction::Delete,
            ClassicAction::Info,
        ] {
            if ui
                .add_enabled(
                    state.enabled(action),
                    egui::Button::new(state.label(action)),
                )
                .clicked()
            {
                chosen = Some(action);
            }
        }
    });
    chosen
}

#[derive(Clone, Copy)]
pub enum TreeIcon {
    Home,
    Folder,
    Disk,
    Network,
    Favorite,
}

pub fn tree_link(ui: &mut egui::Ui, label: &str, active: bool, icon: TreeIcon) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(85.0), 24.0),
        Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let p = ui.painter().with_clip_rect(rect);
    if active || response.hovered() || response.has_focus() {
        p.rect_filled(
            rect.shrink(1.0),
            0,
            if active {
                Color32::from_rgb(0, 0, 128)
            } else {
                Color32::from_rgb(208, 208, 208)
            },
        );
    }
    let icon_at = rect.left_center() + Vec2::new(16.0, 0.0);
    let at = |x: f32, y: f32| icon_at + Vec2::new(x, y);
    match icon {
        TreeIcon::Home | TreeIcon::Folder | TreeIcon::Favorite => {
            p.rect_filled(
                Rect::from_min_max(at(-8.0, -5.0), at(-1.0, -2.0)),
                0,
                Color32::from_rgb(247, 206, 55),
            );
            p.rect_filled(
                Rect::from_min_max(at(-8.0, -2.0), at(8.0, 6.0)),
                0,
                Color32::from_rgb(232, 178, 42),
            );
            p.rect_stroke(
                Rect::from_min_max(at(-8.0, -2.0), at(8.0, 6.0)),
                0,
                Stroke::new(0.8, Color32::from_rgb(120, 91, 25)),
                egui::StrokeKind::Inside,
            );
        }
        TreeIcon::Disk => {
            p.rect_filled(
                Rect::from_min_max(at(-8.0, -5.0), at(8.0, 6.0)),
                0,
                Color32::from_gray(235),
            );
            p.rect_stroke(
                Rect::from_min_max(at(-8.0, -5.0), at(8.0, 6.0)),
                0,
                Stroke::new(1.2, Color32::from_gray(70)),
                egui::StrokeKind::Inside,
            );
            p.circle_filled(at(4.0, 3.0), 1.5, Color32::from_rgb(46, 134, 70));
        }
        TreeIcon::Network => {
            for (x, y) in [(-6.0, -4.0), (6.0, -4.0), (0.0, 6.0)] {
                p.rect_filled(
                    Rect::from_center_size(at(x, y), Vec2::new(6.0, 6.0)),
                    0,
                    Color32::from_rgb(60, 107, 168),
                );
            }
            p.line_segment(
                [at(-6.0, 0.0), at(0.0, 4.0)],
                Stroke::new(1.2, Color32::from_gray(55)),
            );
            p.line_segment(
                [at(6.0, 0.0), at(0.0, 4.0)],
                Stroke::new(1.2, Color32::from_gray(55)),
            );
        }
    }
    p.text(
        rect.left_center() + Vec2::new(31.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        if active {
            Color32::WHITE
        } else {
            Color32::BLACK
        },
    );
    response.on_hover_text(label)
}

pub fn activated(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.clicked()
        || (response.has_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dangerous_actions_require_local_selection() {
        let mut state = ClassicContext {
            local: true,
            selection: 0,
            back: false,
            forward: false,
            grid: false,
            split: false,
            hidden: false,
            sidebar: true,
            pinned: false,
            tools: false,
            volume_busy: false,
        };
        assert!(!state.enabled(ClassicAction::Delete));
        state.selection = 1;
        assert!(state.enabled(ClassicAction::Copy));
        state.local = false;
        for a in [
            ClassicAction::Cut,
            ClassicAction::Copy,
            ClassicAction::Delete,
            ClassicAction::Rename,
            ClassicAction::NewFolder,
            ClassicAction::Paste,
        ] {
            assert!(!state.enabled(a));
        }
        assert!(state.enabled(ClassicAction::Network));
    }
}
