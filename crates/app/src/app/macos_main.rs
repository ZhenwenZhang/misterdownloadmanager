//! The macOS main window: a native desktop-workspace layout rather than the IDM chrome used on
//! Windows and Linux. It deliberately reuses `App`'s commands, selection and download model.
//!
//! The layout is left-to-right only. The Windows shell mirrors itself for right-to-left
//! interfaces through `winui::Surface`, but a half-mirrored hand-painted layout reads worse than
//! an unmirrored one, so this surface stays LTR until it is mirrored as a whole (#A12).

use std::time::Duration;

use egui::{
    Align, Align2, Color32, FontId, Layout, Pos2, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, pos2, vec2,
};

use super::{App, Cmd, MenuEntry, MenuKind, Popup, PopupResult, View};
use crate::icons::Icon;
use crate::model::{self, Category, Item, Status};
use crate::{chrome, winui};

const SIDEBAR_W: f32 = 238.0;
const HEADER_H: f32 = 58.0;
/// Below the width that fits a third pane the inspector narrows instead of going away (#A11).
const ROW_H: f32 = 60.0;
const GROUP_H: f32 = 30.0;
/// The status bar along the bottom of the content area (#A7).
const STATUS_H: f32 = 26.0;
/// The column titles above the list, which also carry the sort order (#A8).
const COLUMNS_H: f32 = 28.0;
/// The order the list paints download states in, which is also the order shift-click extends
/// a range through (#A3).
const GROUP_ORDER: [Status; 5] = [
    Status::Downloading,
    Status::Queued,
    Status::Stopped,
    Status::Error,
    Status::Complete,
];

#[derive(Clone, Copy)]
struct MacColors {
    window: Color32,
    sidebar: Color32,
    surface: Color32,
    border: Color32,
    text: Color32,
    muted: Color32,
    subtle: Color32,
    hover: Color32,
    field: Color32,
    accent: Color32,
    green: Color32,
    amber: Color32,
    red: Color32,
}

/// `color` over `under` at `t` (0..=1): a tint that follows the accent and the surface.
fn blend(under: Color32, color: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        mix(under.r(), color.r()),
        mix(under.g(), color.g()),
        mix(under.b(), color.b()),
        255,
    )
}

impl MacColors {
    fn current() -> Self {
        let accent = crate::theme::colors().accent;
        if crate::theme::dark_surface() {
            Self {
                window: Color32::from_rgb(0x10, 0x10, 0x12),
                sidebar: Color32::from_rgb(0x0c, 0x0c, 0x0e),
                surface: Color32::from_rgb(0x12, 0x12, 0x14),
                border: Color32::from_rgb(0x2a, 0x2a, 0x2d),
                text: Color32::from_rgb(0xf5, 0xf5, 0xf7),
                muted: Color32::from_rgb(0x9a, 0x9a, 0xa1),
                subtle: Color32::from_rgb(0x68, 0x68, 0x70),
                hover: Color32::from_rgb(0x1c, 0x1c, 0x1f),
                field: Color32::from_rgb(0x18, 0x18, 0x1b),
                accent,
                green: Color32::from_rgb(0x32, 0xd7, 0x74),
                amber: Color32::from_rgb(0xff, 0x9f, 0x0a),
                red: Color32::from_rgb(0xff, 0x45, 0x3a),
            }
        } else {
            Self {
                window: Color32::WHITE,
                sidebar: Color32::from_rgb(0xee, 0xee, 0xf1),
                surface: Color32::WHITE,
                border: Color32::from_rgb(0xe1, 0xe1, 0xe5),
                text: Color32::from_rgb(0x1d, 0x1d, 0x1f),
                muted: Color32::from_rgb(0x6e, 0x6e, 0x73),
                subtle: Color32::from_rgb(0x98, 0x98, 0xa0),
                hover: Color32::from_rgb(0xed, 0xed, 0xf0),
                field: Color32::from_rgb(0xf8, 0xf8, 0xfa),
                accent,
                green: Color32::from_rgb(0x24, 0x9b, 0x5a),
                amber: Color32::from_rgb(0xc7, 0x7d, 0x00),
                red: Color32::from_rgb(0xd7, 0x3a, 0x35),
            }
        }
    }
}

#[derive(Clone)]
struct MacRow {
    id: u64,
    name: String,
    extension: String,
    status: Status,
    status_text: String,
    downloaded: u64,
    size: Option<u64>,
    speed: f64,
    error: String,
    /// When it finished ("Completed" column), already in the list's date format; "-" otherwise.
    completed: String,
}

impl MacRow {
    fn from_item(item: &Item) -> Self {
        let extension = item
            .file_name
            .rsplit_once('.')
            .map_or("", |(_, ext)| ext)
            .chars()
            .take(5)
            .collect::<String>()
            .to_uppercase();
        Self {
            id: item.id,
            name: item.file_name.clone(),
            extension,
            status: item.status,
            status_text: item.status_text(),
            downloaded: item.downloaded,
            size: item.size,
            speed: item.rate(),
            error: item.error.clone().unwrap_or_default(),
            completed: if item.status == Status::Complete {
                item.completed
                    .as_deref()
                    .map_or_else(|| "-".to_owned(), model::date_text)
            } else {
                "-".to_owned()
            },
        }
    }

    fn progress(&self) -> f32 {
        self.size
            .filter(|size| *size > 0)
            .map_or(0.0, |size| self.downloaded as f32 / size as f32)
            .clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy)]
enum NavIcon {
    All,
    Active,
    Complete,
    Failed,
    Type(Category, Color32),
}

impl App {
    pub(super) fn paint_macos(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let full = ui.max_rect();
        let colors = MacColors::current();
        ui.painter().rect_filled(full, 0.0, colors.window);

        self.sample_macos_speed();
        // Built once per frame and shared by the header count, the list and the status bar
        // (#A13). `macos_rows` clones the strings it shows, so twice a frame was twice the work.
        let rows = self.macos_rows();

        let sidebar_w = SIDEBAR_W.min((full.width() * 0.3).max(200.0));
        let sidebar = Rect::from_min_max(full.min, pos2(full.left() + sidebar_w, full.bottom()));
        ui.painter().rect_filled(sidebar, 0.0, colors.sidebar);
        ui.painter().line_segment(
            [
                pos2(sidebar.right(), full.top()),
                pos2(sidebar.right(), full.bottom()),
            ],
            Stroke::new(1.0, colors.border),
        );

        // The list spans the whole content area: the details panel that used to open on the
        // right for the selected row is gone (Properties, on the context menu and on a double
        // click, shows the same facts in a window of its own).
        let content_left = sidebar.right();
        let content_right = full.right();
        let header = Rect::from_min_max(
            pos2(content_left, full.top()),
            pos2(content_right, full.top() + HEADER_H),
        );
        let status = Rect::from_min_max(
            pos2(content_left, full.bottom() - STATUS_H),
            pos2(content_right, full.bottom()),
        );
        let list_rect = Rect::from_min_max(
            pos2(content_left, header.bottom()),
            pos2(content_right, status.top()),
        );

        let mut chosen = None;
        self.paint_macos_sidebar(ui, sidebar, colors, &mut chosen);
        self.paint_macos_header(ui, header, colors, &rows, &mut chosen);
        self.paint_macos_list(ui, list_rect, colors, &rows, &mut chosen);
        self.paint_macos_status(ui, status, colors, rows.len());

        if let Some(choice) = self.paint_macos_popup(&ctx) {
            chosen = Some(choice);
        }
        self.macos_shortcuts(&ctx, &mut chosen);
        if let Some((command, target)) = chosen {
            self.run(command, target, &ctx);
        }
    }

    fn sample_macos_speed(&mut self) {
        if self.mac_speed_sampled.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.mac_speed_sampled = std::time::Instant::now();
        let total = self
            .items
            .iter()
            .filter(|item| item.live.is_some())
            .map(Item::rate)
            .sum();
        self.mac_speed_history.push(total);
        if self.mac_speed_history.len() > 32 {
            self.mac_speed_history.remove(0);
        }
    }

    fn paint_macos_sidebar(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        colors: MacColors,
        chosen: &mut Option<(Cmd, Option<u64>)>,
    ) {
        let mut side = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(12.0, 10.0)))
                .layout(Layout::top_down(Align::Min)),
        );
        side.set_clip_rect(rect);

        side.horizontal(|ui| {
            let logo = Rect::from_min_size(ui.cursor().min, vec2(26.0, 26.0));
            let response = ui.allocate_rect(logo, winui::pointer_only());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, crate::i18n::t("About"))
            });
            let p = ui.painter();
            p.rect_filled(logo, 6.0, colors.text);
            let ink = colors.sidebar;
            p.line_segment(
                [
                    pos2(logo.left() + 8.0, logo.top() + 9.0),
                    pos2(logo.center().x, logo.top() + 14.0),
                ],
                Stroke::new(1.8, ink),
            );
            p.line_segment(
                [
                    pos2(logo.center().x, logo.top() + 14.0),
                    pos2(logo.right() - 8.0, logo.top() + 9.0),
                ],
                Stroke::new(1.8, ink),
            );
            p.line_segment(
                [
                    pos2(logo.left() + 8.0, logo.bottom() - 7.0),
                    pos2(logo.right() - 8.0, logo.bottom() - 7.0),
                ],
                Stroke::new(1.8, ink),
            );
            if response.clicked() {
                *chosen = Some((Cmd::About, None));
            }
            response.on_hover_text(crate::MAIN_TITLE);
            ui.add_space(7.0);
            // The whole name (it read "Mister"): as wide as the sidebar can show, with the
            // rest in the tooltip when a long translation has to be shortened.
            let name = crate::i18n::t("Mister Download Manager");
            let room = (rect.width() - 24.0 - 26.0 - 7.0).max(60.0);
            let font = FontId::proportional(13.5);
            let shown = winui::ellipsize(ui.painter(), &font, name, room);
            let response = ui.label(RichText::new(shown).size(13.5).strong().color(colors.text));
            response.on_hover_text(name);
        });
        side.add_space(18.0);

        let all = self.items.len();
        let active = self
            .items
            .iter()
            .filter(|item| {
                matches!(
                    item.status,
                    Status::Downloading | Status::Queued | Status::Stopped
                )
            })
            .count();
        let complete = self
            .items
            .iter()
            .filter(|item| item.status == Status::Complete)
            .count();
        let failed = self
            .items
            .iter()
            .filter(|item| item.status == Status::Error)
            .count();

        if nav_row(
            &mut side,
            "All",
            all,
            NavIcon::All,
            self.view == View::All && !self.mac_failed_only,
            colors,
        )
        .clicked()
        {
            self.view = View::All;
            self.mac_failed_only = false;
        }
        if nav_row(
            &mut side,
            "Unfinished",
            active,
            NavIcon::Active,
            self.view == View::Unfinished(None) && !self.mac_failed_only,
            colors,
        )
        .clicked()
        {
            self.view = View::Unfinished(None);
            self.mac_failed_only = false;
        }
        if nav_row(
            &mut side,
            "Finished",
            complete,
            NavIcon::Complete,
            self.view == View::Finished(None) && !self.mac_failed_only,
            colors,
        )
        .clicked()
        {
            self.view = View::Finished(None);
            self.mac_failed_only = false;
        }
        if nav_row(
            &mut side,
            "Error",
            failed,
            NavIcon::Failed,
            self.mac_failed_only,
            colors,
        )
        .clicked()
        {
            self.view = View::Unfinished(None);
            self.mac_failed_only = true;
        }

        side.add_space(18.0);
        side.label(
            RichText::new(crate::i18n::t("Categories"))
                .size(11.0)
                .strong()
                .color(colors.subtle),
        );
        side.add_space(5.0);

        // The bottom of the sidebar is laid out upwards (#A14): Options is pinned to the bottom
        // edge, the network card sits above it, and the categories take whatever is left. A
        // short window used to draw the card over the categories; now the card goes first.
        let options_rect = Rect::from_min_size(
            pos2(rect.left() + 12.0, rect.bottom() - 36.0),
            vec2(rect.width() - 24.0, 28.0),
        );
        let categories_top = side.cursor().min.y;
        let gap = 12.0;
        let card_h = 118.0;
        let card_top = options_rect.top() - 10.0 - card_h;
        let show_card = card_top - gap - categories_top >= 120.0;
        let categories_bottom = if show_card {
            card_top - gap
        } else {
            options_rect.top() - gap
        };
        let categories_rect = Rect::from_min_max(
            pos2(rect.left() + 12.0, categories_top),
            pos2(rect.right() - 12.0, categories_bottom),
        );
        if categories_rect.height() > 0.0 {
            let mut categories_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(categories_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            categories_ui.set_clip_rect(categories_rect);
            egui::ScrollArea::vertical()
                .id_salt("macos-categories")
                .auto_shrink([false, false])
                .show(&mut categories_ui, |ui| {
                    ui.set_width(categories_rect.width());
                    for (category, color) in category_colors() {
                        if category.deleted() {
                            continue;
                        }
                        let count = self
                            .items
                            .iter()
                            .filter(|item| item.category == category)
                            .count();
                        let name = category.label();
                        if nav_row(
                            ui,
                            name.as_ref(),
                            count,
                            NavIcon::Type(category, color),
                            self.view == View::Cat(category) && !self.mac_failed_only,
                            colors,
                        )
                        .clicked()
                        {
                            self.view = View::Cat(category);
                            self.mac_failed_only = false;
                        }
                    }
                });
        }

        // Options sits at the bottom of the sidebar with the same icon-and-label shape as the
        // navigation rows above it, so it reads as part of the sidebar rather than a stray link.
        let settings_probe = ui.interact(
            options_rect,
            egui::Id::unique("macos-sidebar-options"),
            winui::pointer_only(),
        );
        if settings_probe.hovered() {
            ui.painter().rect_filled(options_rect, 7.0, colors.hover);
        }
        Icon::Options.image(true).paint_at(
            ui,
            Rect::from_center_size(
                pos2(options_rect.left() + 16.0, options_rect.center().y),
                vec2(16.0, 16.0),
            ),
        );
        ui.painter().text(
            pos2(options_rect.left() + 37.0, options_rect.center().y),
            Align2::LEFT_CENTER,
            crate::i18n::t("Options"),
            FontId::proportional(13.0),
            colors.muted,
        );
        settings_probe.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, crate::i18n::t("Options"))
        });
        if settings_probe.clicked() {
            *chosen = Some((Cmd::Options, None));
        }

        if show_card {
            let network = Rect::from_min_max(
                pos2(rect.left() + 12.0, card_top),
                pos2(rect.right() - 12.0, card_top + card_h),
            );
            self.paint_network_card(ui, network, colors, chosen);
        }
    }

    fn paint_network_card(
        &self,
        ui: &mut Ui,
        rect: Rect,
        colors: MacColors,
        chosen: &mut Option<(Cmd, Option<u64>)>,
    ) {
        let response = ui.interact(
            rect,
            egui::Id::unique("macos-network-card"),
            winui::pointer_only(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, crate::i18n::t("Speed Limiter"))
        });
        if response.clicked() {
            *chosen = Some((Cmd::LimiterSettings, None));
        }
        let p = ui.painter();
        // No box: a sidebar footer reads as a block of its own with a hairline above it, and
        // the hover tint says it opens the limiter without a border around the numbers.
        p.rect_filled(
            rect,
            9.0,
            if response.hovered() {
                colors.hover
            } else {
                Color32::TRANSPARENT
            },
        );
        p.line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(0.8, blend(colors.sidebar, colors.border, 0.75)),
        );
        let left = rect.left() + 8.0;
        let right = rect.right() - 8.0;
        let speed: f64 = self
            .items
            .iter()
            .filter(|item| item.live.is_some())
            .map(Item::rate)
            .sum();
        let active = self.items.iter().filter(|item| item.live.is_some()).count() as u64;
        p.text(
            pos2(left, rect.top() + 11.0),
            Align2::LEFT_CENTER,
            crate::i18n::t("Transfer rate"),
            FontId::proportional(11.0),
            colors.subtle,
        );
        if active > 0 {
            p.circle_filled(pos2(right - 3.0, rect.top() + 11.0), 3.2, colors.green);
        }
        // The number carries the card; its unit stays small, so the figure reads at a glance
        // the way a network widget in the sidebar does. Speeds stay in MB here even under a
        // megabyte: a three-decimal KB figure reads like a sliver next to this size of type.
        let rate = model::rate_mb(speed);
        let (value, unit) = rate.split_once(' ').unwrap_or((rate.as_str(), ""));
        let base = rect.top() + 46.0;
        let value_font = FontId::proportional(23.0);
        let value_w = winui::text_width_font(p, value_font.clone(), value);
        p.text(
            pos2(left, base),
            Align2::LEFT_BOTTOM,
            value,
            value_font,
            colors.text,
        );
        if !unit.is_empty() {
            p.text(
                pos2(left + value_w + 4.0, base - 3.0),
                Align2::LEFT_BOTTOM,
                unit,
                FontId::proportional(11.0),
                colors.subtle,
            );
        }
        let chart = Rect::from_min_max(
            pos2(left, rect.top() + 56.0),
            pos2(right, rect.bottom() - 22.0),
        );
        let max = self
            .mac_speed_history
            .iter()
            .copied()
            .fold(0.0_f64, f64::max);
        let points: Vec<Pos2> = self
            .mac_speed_history
            .iter()
            .enumerate()
            .map(|(i, value)| {
                let t = i as f32 / (self.mac_speed_history.len().saturating_sub(1).max(1)) as f32;
                pos2(
                    egui::lerp(chart.left()..=chart.right(), t),
                    chart.bottom() - chart.height() * (*value / max) as f32,
                )
            })
            .collect();
        if points.len() > 1 && max > 0.0 {
            // A soft gradient under the line: the sidebar keeps one accent, and the fading
            // fill gives the chart weight without another colour.
            let mut mesh = egui::Mesh::default();
            let top = Color32::from_rgba_unmultiplied(
                colors.accent.r(),
                colors.accent.g(),
                colors.accent.b(),
                50,
            );
            let bottom = Color32::from_rgba_unmultiplied(
                colors.accent.r(),
                colors.accent.g(),
                colors.accent.b(),
                0,
            );
            for pair in points.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let i = mesh.vertices.len() as u32;
                mesh.colored_vertex(a, top);
                mesh.colored_vertex(b, top);
                mesh.colored_vertex(pos2(b.x, chart.bottom()), bottom);
                mesh.colored_vertex(pos2(a.x, chart.bottom()), bottom);
                mesh.add_triangle(i, i + 1, i + 2);
                mesh.add_triangle(i, i + 2, i + 3);
            }
            let chart_painter = p.with_clip_rect(chart);
            chart_painter.add(egui::Shape::mesh(mesh));
            chart_painter.add(egui::Shape::line(points, Stroke::new(1.6, colors.accent)));
        } else {
            // No traffic: an empty chart keeps its foot, and "Idle" sits at the card's top
            // right, on the figure's baseline (aligned with the rate it belongs to) rather
            // than in the middle of the plot area.
            p.line_segment(
                [chart.left_bottom(), chart.right_bottom()],
                Stroke::new(1.0, blend(colors.sidebar, colors.border, 0.7)),
            );
            p.text(
                pos2(right, base),
                Align2::RIGHT_BOTTOM,
                // i18n: the sidebar speed chart while nothing is downloading
                crate::i18n::t("Idle"),
                FontId::proportional(11.0),
                colors.subtle,
            );
        }
        let remaining: u64 = self
            .items
            .iter()
            .filter(|item| item.status != Status::Complete)
            .map(|item| {
                item.size
                    .unwrap_or(item.downloaded)
                    .saturating_sub(item.downloaded)
            })
            .sum();
        let mut footer = crate::i18n::tn(active, "{n} downloading", "{n} downloading");
        if remaining > 0 {
            footer.push_str(" · ");
            footer.push_str(&model::size_list(remaining));
        }
        p.text(
            pos2(left, rect.bottom() - 7.0),
            Align2::LEFT_BOTTOM,
            footer,
            FontId::proportional(10.5),
            colors.subtle,
        );
        response.on_hover_text(crate::i18n::t("Speed Limiter"));
    }

    fn paint_macos_header(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        colors: MacColors,
        rows: &[MacRow],
        chosen: &mut Option<(Cmd, Option<u64>)>,
    ) {
        ui.painter().rect_filled(rect, 0.0, colors.surface);
        ui.painter().line_segment(
            [
                pos2(rect.left(), rect.bottom()),
                pos2(rect.right(), rect.bottom()),
            ],
            Stroke::new(1.0, colors.border),
        );
        let mut header = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(16.0, 10.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        header.label(
            RichText::new(self.macos_view_title())
                .size(20.0)
                .strong()
                .color(colors.text),
        );
        header.add_space(2.0);
        header.label(
            RichText::new(crate::i18n::integer(rows.len() as u64))
                .size(12.0)
                .color(colors.subtle),
        );

        header.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let more = menu_button(ui, colors);
            if more.clicked() {
                self.open_popup(
                    Popup::MacMain,
                    pos2(more.rect.left(), more.rect.bottom() + 4.0),
                );
            }
            ui.add_space(4.0);
            // The search box is shown at every width (#A2); it is drawn here rather than by
            // `TextEdit` so it can carry the magnifier and the same radius as the toolbar's
            // buttons.
            let search_w = (ui.available_width() - 232.0).clamp(120.0, 200.0);
            let (field, _) = ui.allocate_exact_size(vec2(search_w, 28.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(field, 8.0, colors.field);
            p.rect_stroke(
                field,
                8.0,
                Stroke::new(1.0, colors.border),
                StrokeKind::Inside,
            );
            let lens = pos2(field.left() + 13.0, field.center().y - 1.0);
            p.circle_stroke(lens, 4.2, Stroke::new(1.4, colors.subtle));
            p.line_segment(
                [
                    pos2(lens.x + 3.0, lens.y + 3.0),
                    pos2(lens.x + 5.8, lens.y + 5.8),
                ],
                Stroke::new(1.4, colors.subtle),
            );
            let mut search_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(field.shrink2(vec2(23.0, 1.0)))
                    .layout(Layout::left_to_right(Align::Center)),
            );
            let search = search_ui.add(
                egui::TextEdit::singleline(&mut self.mac_search)
                    .id(egui::Id::unique("macos-main-search"))
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY),
            );
            if search.changed() {
                self.selection.retain(&self.macos_row_ids());
            }
            ui.add_space(6.0);

            let torrent = icon_button(ui, Icon::AddTorrent, true, colors, "Open torrent file");
            if torrent.clicked() {
                *chosen = Some((Cmd::OpenTorrentFile, None));
            } else if torrent.secondary_clicked() {
                self.open_popup(
                    Popup::TorrentDrop,
                    pos2(torrent.rect.left(), torrent.rect.bottom() + 4.0),
                );
            }
            if icon_button(ui, Icon::AddUrl, true, colors, "Add new download").clicked() {
                *chosen = Some((Cmd::AddUrl, None));
            }
        });
    }

    fn paint_macos_list(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        colors: MacColors,
        rows: &[MacRow],
        chosen: &mut Option<(Cmd, Option<u64>)>,
    ) {
        ui.painter().rect_filled(rect, 0.0, colors.surface);
        let ids: Vec<u64> = rows.iter().map(|row| row.id).collect();
        self.selection.retain(&ids);

        let columns = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + COLUMNS_H));
        self.paint_macos_columns(ui, columns, colors);
        let body = Rect::from_min_max(pos2(rect.left(), columns.bottom()), rect.max);

        // The order rows are drawn in is the order a shift-click extends through (#A3). The
        // groups reorder the list, so the model's id order no longer matches the screen and a
        // range picked from it landed on the wrong rows.
        let mut ordered: Vec<u64> = Vec::with_capacity(rows.len());
        for status in GROUP_ORDER {
            ordered.extend(
                rows.iter()
                    .filter(|row| row.status == status)
                    .map(|row| row.id),
            );
        }

        let mut list_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(body)
                .layout(Layout::top_down(Align::Min)),
        );
        list_ui.set_clip_rect(body);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut list_ui, |ui| {
                ui.set_width(body.width());
                if rows.is_empty() {
                    self.paint_macos_empty(ui, body, colors, chosen);
                    return;
                }
                let mut index = 0usize;
                for status in GROUP_ORDER {
                    let group: Vec<&MacRow> =
                        rows.iter().filter(|row| row.status == status).collect();
                    if group.is_empty() {
                        continue;
                    }
                    self.paint_macos_group(ui, status, &group, colors);
                    for row in group {
                        self.paint_macos_row(ui, row, index, &ordered, colors, chosen);
                        index += 1;
                    }
                }
                ui.add_space(12.0);
            });

        let dragging =
            self.drops.over() || ui.ctx().input(|input| !input.raw.hovered_files.is_empty());
        if dragging {
            ui.painter().rect_stroke(
                rect.shrink(3.0),
                8.0,
                Stroke::new(2.0, colors.accent),
                StrokeKind::Inside,
            );
        }
    }

    /// The titles above the list (#A8): the Windows shell sorts from its column headers, and
    /// the macOS one stored a sort order that nothing could set.
    fn paint_macos_columns(&mut self, ui: &Ui, rect: Rect, colors: MacColors) {
        let p = ui.painter();
        p.rect_filled(rect, 0.0, colors.surface);
        p.line_segment(
            [
                pos2(rect.left(), rect.bottom()),
                pos2(rect.right(), rect.bottom()),
            ],
            Stroke::new(1.0, blend(colors.surface, colors.border, 0.8)),
        );
        let w = rect.width();
        let right_edge = rect.right() - 16.0;
        let time_w = 132.0;
        let value_w = if w > 690.0 { 108.0 } else { 74.0 };
        let gap = 12.0;
        let percent_w = 40.0;
        let value_right = right_edge - time_w - gap;
        let progress_right = value_right - value_w - gap;
        let progress_left = progress_right - percent_w;
        let cy = rect.center().y;
        let mut sort = None;
        if column_header(
            ui,
            "File Name",
            0,
            rect.left() + 38.0,
            cy,
            self.list.sort,
            colors,
        ) {
            sort = Some(0);
        }
        if column_header(ui, "Size", 2, value_right, cy, self.list.sort, colors) {
            sort = Some(2);
        }
        if column_header(
            ui,
            "Progress",
            3,
            (progress_left + progress_right) / 2.0,
            cy,
            self.list.sort,
            colors,
        ) {
            sort = Some(3);
        }
        ui.painter().text(
            pos2(right_edge, cy),
            Align2::RIGHT_CENTER,
            // i18n: column title in the download list: when the download finished
            crate::i18n::t("Completed"),
            FontId::proportional(11.5),
            colors.subtle,
        );
        if let Some(column) = sort {
            self.list.sort = Some(match self.list.sort {
                Some((current, ascending)) if current == column => (column, !ascending),
                _ => (column, true),
            });
        }
    }

    /// The bar along the bottom (#A7): what the Windows shell draws in `status_strip`, which
    /// the macOS layout dropped entirely.
    fn paint_macos_status(&self, ui: &Ui, rect: Rect, colors: MacColors, shown: usize) {
        let p = ui.painter();
        p.rect_filled(rect, 0.0, colors.window);
        p.line_segment(
            [
                pos2(rect.left(), rect.top()),
                pos2(rect.right(), rect.top()),
            ],
            Stroke::new(1.0, colors.border),
        );
        let mut left = crate::i18n::tn(shown as u64, "{n} download", "{n} downloads");
        let selected = self.selection.ids().len() as u64;
        if selected > 1 {
            left.push_str("   ");
            left.push_str(&crate::i18n::tn(selected, "{n} selected", "{n} selected"));
        }
        p.text(
            pos2(rect.left() + 16.0, rect.center().y),
            Align2::LEFT_CENTER,
            left,
            FontId::proportional(11.0),
            colors.subtle,
        );
        if let Some(activity) = self.activity() {
            let font = FontId::proportional(11.0);
            let room = (rect.width() - 260.0).max(80.0);
            let text = winui::ellipsize(p, &font, &activity, room);
            p.text(
                pos2(rect.right() - 16.0, rect.center().y),
                Align2::RIGHT_CENTER,
                text,
                font,
                colors.muted,
            );
        }
    }

    fn paint_macos_group(&self, ui: &mut Ui, status: Status, rows: &[&MacRow], colors: MacColors) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), GROUP_H), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, colors.window);
        let color = status_color(status, colors);
        paint_status_icon(
            ui.painter(),
            pos2(rect.left() + 18.0, rect.center().y),
            status,
            color,
        );
        let name = status_group_name(status);
        ui.painter().text(
            pos2(rect.left() + 37.0, rect.center().y),
            Align2::LEFT_CENTER,
            format!(
                "{}   {}",
                crate::i18n::t(name),
                crate::i18n::integer(rows.len() as u64)
            ),
            FontId::proportional(11.0),
            colors.subtle,
        );
        if status == Status::Downloading {
            let total: f64 = rows.iter().map(|row| row.speed).sum();
            ui.painter().text(
                pos2(rect.right() - 18.0, rect.center().y),
                Align2::RIGHT_CENTER,
                model::rate(total),
                FontId::proportional(12.0),
                colors.muted,
            );
        }
    }

    fn paint_macos_row(
        &mut self,
        ui: &mut Ui,
        row: &MacRow,
        index: usize,
        ids: &[u64],
        colors: MacColors,
        chosen: &mut Option<(Cmd, Option<u64>)>,
    ) {
        // `Sense::click()` alone is click *and keyboard focus* in egui 0.37: it put every row
        // in the Tab order without a focus ring, and it took focus away from the download list
        // when a dialog's text box was open (#A4). Click-only is what the rest of the app uses.
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), winui::pointer_only());
        let selected = self.selection.contains(row.id);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::ListItem, true, selected, row.name.clone())
        });
        let row_bg = rect.shrink2(vec2(8.0, 2.0));
        if selected {
            ui.painter()
                .rect_filled(row_bg, 8.0, blend(colors.surface, colors.accent, 0.13));
        } else if response.hovered() {
            ui.painter().rect_filled(row_bg, 8.0, colors.hover);
        }
        let state_color = status_color(row.status, colors);
        paint_status_icon(
            ui.painter(),
            pos2(rect.left() + 20.0, rect.center().y),
            row.status,
            state_color,
        );

        let name_left = rect.left() + 40.0;
        let right_edge = rect.right() - 16.0;
        let time_w = 132.0;
        let value_w = if rect.width() > 690.0 { 108.0 } else { 74.0 };
        let gap = 12.0;
        let percent_w = 40.0;
        let value_right = right_edge - time_w - gap;
        let progress_right = value_right - value_w - gap;
        let progress_left = progress_right - percent_w;
        let metrics_left = progress_left;
        let badge_space = if row.extension.is_empty() { 0.0 } else { 48.0 };
        let max_name = (metrics_left - name_left - 12.0 - badge_space).max(80.0);
        let font = FontId::proportional(13.0);
        let shown = winui::ellipsize(ui.painter(), &font, &row.name, max_name);
        ui.painter().text(
            pos2(name_left, rect.center().y - 7.0),
            Align2::LEFT_CENTER,
            &shown,
            font,
            colors.text,
        );
        if !row.extension.is_empty() {
            let ext_x = (name_left + winui::text_width(ui.painter(), &shown) + 10.0)
                .min(metrics_left - 42.0);
            let badge = Rect::from_min_size(pos2(ext_x, rect.center().y - 12.0), vec2(38.0, 22.0));
            ui.painter().rect_stroke(
                badge,
                5.0,
                Stroke::new(1.0, colors.border),
                StrokeKind::Inside,
            );
            ui.painter().text(
                badge.center(),
                Align2::CENTER_CENTER,
                &row.extension,
                FontId::proportional(10.0),
                colors.subtle,
            );
        }
        if !row.error.is_empty() {
            let error = winui::ellipsize(
                ui.painter(),
                &FontId::proportional(11.0),
                &row.error,
                max_name,
            );
            ui.painter().text(
                pos2(name_left, rect.center().y + 12.5),
                Align2::LEFT_CENTER,
                error,
                FontId::proportional(11.0),
                colors.red,
            );
        } else if row.status != Status::Complete {
            ui.painter().text(
                pos2(name_left, rect.center().y + 12.5),
                Align2::LEFT_CENTER,
                &row.status_text,
                FontId::proportional(11.0),
                colors.muted,
            );
        }

        // Progress as its figure only (the bar was the busy part of the row): 12 % between the
        // name and the size, right-aligned like the other figures.
        let progress = row.progress();
        ui.painter().text(
            pos2(progress_right, rect.center().y),
            Align2::RIGHT_CENTER,
            crate::i18n::percent(progress.into(), 0),
            FontId::proportional(11.5),
            colors.subtle,
        );
        let right = if row.status == Status::Downloading {
            model::rate(row.speed)
        } else if row.status == Status::Complete {
            row.size.map_or_else(|| "-".to_owned(), model::size_list)
        } else {
            row.status_text.clone()
        };
        let right = winui::ellipsize(ui.painter(), &FontId::proportional(11.5), &right, value_w);
        ui.painter().text(
            pos2(value_right, rect.center().y),
            Align2::RIGHT_CENTER,
            right,
            FontId::proportional(11.5),
            if row.status == Status::Error {
                colors.red
            } else {
                colors.muted
            },
        );
        let completed = winui::ellipsize(
            ui.painter(),
            &FontId::proportional(11.5),
            &row.completed,
            time_w,
        );
        ui.painter().text(
            pos2(right_edge, rect.center().y),
            Align2::RIGHT_CENTER,
            completed,
            FontId::proportional(11.5),
            colors.subtle,
        );

        if response.clicked() {
            let modifiers = ui.input(|input| input.modifiers);
            let pick = if modifiers.shift {
                crate::selection::Pick::Range {
                    to: index,
                    add: modifiers.command,
                }
            } else if modifiers.command {
                crate::selection::Pick::Toggle(index)
            } else {
                crate::selection::Pick::Only(index)
            };
            self.selection.apply(pick, ids);
        }
        if response.double_clicked() {
            if row.status == Status::Complete && !self.settings.double_click_open {
                *chosen = Some((Cmd::Properties, Some(row.id)));
            } else {
                *chosen = Some((Cmd::Open, Some(row.id)));
            }
        }
        if response.secondary_clicked()
            && let Some(at) = response.interact_pointer_pos()
        {
            if !self.selection.contains(row.id) {
                self.selection.set(row.id);
            }
            self.open_popup(Popup::Item(Some(row.id)), at);
        }
    }

    fn paint_macos_empty(
        &self,
        ui: &mut Ui,
        rect: Rect,
        colors: MacColors,
        chosen: &mut Option<(Cmd, Option<u64>)>,
    ) {
        let body = Rect::from_min_max(
            rect.min,
            pos2(rect.right(), rect.bottom().max(rect.top() + 320.0)),
        );
        let center = pos2(
            body.center().x,
            body.top() + body.height().min(420.0) * 0.45,
        );
        let p = ui.painter();
        p.circle_filled(center, 34.0, blend(colors.surface, colors.accent, 0.08));
        p.circle_stroke(
            center,
            34.0,
            Stroke::new(1.0, blend(colors.surface, colors.border, 0.9)),
        );
        p.line_segment(
            [
                pos2(center.x, center.y - 12.0),
                pos2(center.x, center.y + 10.0),
            ],
            Stroke::new(2.0, colors.accent),
        );
        p.line_segment(
            [
                pos2(center.x - 6.5, center.y + 3.0),
                pos2(center.x, center.y + 10.0),
            ],
            Stroke::new(2.0, colors.accent),
        );
        p.line_segment(
            [
                pos2(center.x + 6.5, center.y + 3.0),
                pos2(center.x, center.y + 10.0),
            ],
            Stroke::new(2.0, colors.accent),
        );
        let title = if self.mac_search.is_empty() {
            "No downloads here"
        } else {
            "No matching downloads"
        };
        p.text(
            pos2(center.x, center.y + 42.0),
            Align2::CENTER_CENTER,
            crate::i18n::t(title),
            FontId::proportional(14.0),
            colors.muted,
        );
        let button = Rect::from_center_size(pos2(center.x, center.y + 86.0), vec2(154.0, 30.0));
        let response = ui.interact(
            button,
            egui::Id::unique("mac-empty-action"),
            winui::pointer_only(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, crate::i18n::t("Add new download"))
        });
        p.rect_filled(
            button,
            8.0,
            if response.hovered() {
                blend(colors.surface, colors.accent, 0.16)
            } else {
                colors.accent
            },
        );
        p.text(
            button.center(),
            Align2::CENTER_CENTER,
            crate::i18n::t("Add new download"),
            FontId::proportional(12.5),
            Color32::WHITE,
        );
        if response.clicked() {
            *chosen = Some((Cmd::AddUrl, None));
        }
    }

    fn macos_rows(&self) -> Vec<MacRow> {
        let query = self.mac_search.trim().to_lowercase();
        self.view_items()
            .into_iter()
            .filter(|item| !self.mac_failed_only || item.status == Status::Error)
            .filter(|item| {
                query.is_empty()
                    || item.file_name.to_lowercase().contains(&query)
                    || item.url.as_str().to_lowercase().contains(&query)
                    || item.description.to_lowercase().contains(&query)
            })
            .map(MacRow::from_item)
            .collect()
    }

    fn macos_row_ids(&self) -> Vec<u64> {
        self.macos_rows().into_iter().map(|row| row.id).collect()
    }

    fn macos_view_title(&self) -> String {
        if self.mac_failed_only {
            return crate::i18n::t("Error").to_owned();
        }
        match self.view {
            View::All => crate::i18n::t("All").to_owned(),
            View::Cat(category) => {
                let name = category.label();
                crate::i18n::t(name.as_ref()).to_owned()
            }
            View::Unfinished(_) => crate::i18n::t("Unfinished").to_owned(),
            View::Finished(_) => crate::i18n::t("Finished").to_owned(),
            View::Queue(index) => self.queues.get(index).map_or_else(
                || crate::i18n::t("Queue").to_owned(),
                |queue| crate::i18n::t(&queue.name).to_owned(),
            ),
        }
    }

    fn paint_macos_popup(&mut self, ctx: &egui::Context) -> Option<(Cmd, Option<u64>)> {
        let (kind, position, serial) = self.popup?;
        let (entries, target) = match kind {
            Popup::MacMain => {
                let labels = ["&Tasks", "&File", "&Downloads", "&View", "&Help"];
                let entries = labels
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| MenuEntry::Sub(label, true, self.menu_entries(index)))
                    .collect();
                (entries, None)
            }
            Popup::Item(None) => (self.blank_entries(), None),
            Popup::Item(id) => (self.item_entries(id), id),
            Popup::QueueDrop(start) => (self.queue_drop(start), None),
            Popup::TorrentDrop => (
                vec![
                    MenuEntry::Item("Open &torrent file...", Cmd::OpenTorrentFile, true, false),
                    MenuEntry::Item("Add &magnet link...", Cmd::AddMagnet, true, false),
                    MenuEntry::Item("&RSS feeds...", Cmd::RssFeeds(false), true, false),
                    MenuEntry::Sep,
                    MenuEntry::Item("Torrent setti&ngs...", Cmd::TorrentSettings, true, false),
                ],
                None,
            ),
            Popup::Node(node) => (self.node_entries(node), None),
            // The macOS shell has no column chooser: the titles above the list carry the sort
            // order instead (#A8). The Windows View → Columns list is not reachable here.
            Popup::Columns => (Vec::new(), None),
            Popup::DropTarget => (
                vec![
                    MenuEntry::Item("Show Mister Download Manager", Cmd::ShowMain, true, false),
                    MenuEntry::Item("&Add new download", Cmd::AddUrl, true, false),
                    MenuEntry::Sep,
                    MenuEntry::Item("Hide drop target", Cmd::ToggleDropTarget, true, false),
                ],
                None,
            ),
        };
        // The shell's menu bar is a popup of its own on macOS: its titles are written for the
        // menu bar ("menubar"), and looking them up as ordinary menu items left them in English
        // in every other language (#A9).
        let menu_kind = match kind {
            Popup::MacMain => MenuKind::Bar,
            _ => MenuKind::Context,
        };
        match chrome::popup(
            ctx,
            menu_kind,
            &format!("popup-{serial}"),
            position,
            &entries,
            &|command: &Cmd| command.hint(),
        ) {
            PopupResult::Open | PopupResult::Neighbor(_) => None,
            PopupResult::Chosen(command) => {
                self.popup = None;
                Some((command, target))
            }
            PopupResult::Dismissed => {
                self.popup = None;
                None
            }
        }
    }

    fn macos_shortcuts(&mut self, ctx: &egui::Context, chosen: &mut Option<(Cmd, Option<u64>)>) {
        // Keys belong to whichever window has them: a dialog's text box or its buttons, an open
        // menu, or a text box being typed in. The shell used to act on all of them from behind,
        // so Delete in a dialog deleted downloads and Cmd+N stacked a second Add-URL window
        // (#A1).
        if ctx.egui_wants_keyboard_input()
            || self.popup.is_some()
            || self.open_menu.is_some()
            || crate::dialogs::dialogs_open()
        {
            return;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::unique("macos-main-search")));
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::N)) {
            *chosen = Some((Cmd::AddUrl, None));
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma)) {
            *chosen = Some((Cmd::Options, None));
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::A)) {
            let ids = self.macos_row_ids();
            self.selection.apply(crate::selection::Pick::All, &ids);
        }
        // Shift-Delete trashes the files as well, as the Windows list does; a Mac keyboard's
        // delete key is Backspace, so both remove (#A1).
        if ctx.input_mut(|input| {
            input.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Delete,
            )
        }) {
            *chosen = Some((Cmd::RemoveWithFiles, None));
        } else if ctx.input(|input| {
            input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace)
        }) {
            *chosen = Some((Cmd::Remove, None));
        }
    }
}

fn nav_row(
    ui: &mut Ui,
    label: &str,
    count: usize,
    icon: NavIcon,
    selected: bool,
    colors: MacColors,
) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 30.0), winui::pointer_only());
    response.widget_info(|| {
        let name = crate::i18n::t(label);
        let name = if count > 0 {
            format!("{name} {}", crate::i18n::integer(count as u64))
        } else {
            name.to_owned()
        };
        egui::WidgetInfo::selected(egui::Role::ListItem, true, selected, name)
    });
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            7.0,
            if selected {
                blend(colors.sidebar, colors.accent, 0.14)
            } else {
                colors.hover
            },
        );
    }
    paint_nav_icon(
        ui.painter(),
        pos2(rect.left() + 15.0, rect.center().y),
        icon,
        selected,
        colors,
    );
    ui.painter().text(
        pos2(rect.left() + 37.0, rect.center().y),
        Align2::LEFT_CENTER,
        crate::i18n::t(label),
        FontId::proportional(13.0),
        colors.text,
    );
    if count > 0 {
        ui.painter().text(
            pos2(rect.right() - 12.0, rect.center().y),
            Align2::RIGHT_CENTER,
            crate::i18n::integer(count as u64),
            FontId::proportional(11.5),
            colors.subtle,
        );
    }
    response
}

fn paint_nav_icon(
    p: &egui::Painter,
    center: Pos2,
    icon: NavIcon,
    selected: bool,
    colors: MacColors,
) {
    match icon {
        NavIcon::Type(category, color) => paint_category_icon(p, center, category, color),
        NavIcon::All | NavIcon::Active | NavIcon::Complete | NavIcon::Failed => {
            let ink = if selected {
                colors.accent
            } else {
                colors.muted
            };
            let stroke = Stroke::new(1.5, ink);
            match icon {
                NavIcon::All => {
                    let rect = Rect::from_center_size(center, vec2(15.0, 12.5));
                    p.rect_stroke(rect, 3.0, stroke, StrokeKind::Inside);
                    p.line_segment(
                        [
                            pos2(rect.left() + 2.0, center.y - 2.0),
                            pos2(rect.right() - 2.0, center.y - 2.0),
                        ],
                        stroke,
                    );
                }
                NavIcon::Active => {
                    p.line_segment(
                        [
                            pos2(center.x, center.y - 8.0),
                            pos2(center.x, center.y + 5.0),
                        ],
                        stroke,
                    );
                    p.line_segment(
                        [
                            pos2(center.x - 4.5, center.y + 1.0),
                            pos2(center.x, center.y + 5.5),
                        ],
                        stroke,
                    );
                    p.line_segment(
                        [
                            pos2(center.x + 4.5, center.y + 1.0),
                            pos2(center.x, center.y + 5.5),
                        ],
                        stroke,
                    );
                    p.line_segment(
                        [
                            pos2(center.x - 6.0, center.y + 7.5),
                            pos2(center.x + 6.0, center.y + 7.5),
                        ],
                        stroke,
                    );
                }
                NavIcon::Complete => {
                    p.circle_stroke(center, 7.0, stroke);
                    p.line_segment(
                        [
                            pos2(center.x - 3.5, center.y + 0.2),
                            pos2(center.x - 0.8, center.y + 3.0),
                        ],
                        stroke,
                    );
                    p.line_segment(
                        [
                            pos2(center.x - 0.8, center.y + 3.0),
                            pos2(center.x + 4.0, center.y - 2.8),
                        ],
                        stroke,
                    );
                }
                _ => {
                    p.circle_stroke(center, 7.0, stroke);
                    p.line_segment(
                        [
                            pos2(center.x, center.y - 3.8),
                            pos2(center.x, center.y + 1.2),
                        ],
                        stroke,
                    );
                    p.circle_filled(pos2(center.x, center.y + 4.2), 1.0, ink);
                }
            }
        }
    }
}

/// The category's mark in a Finder-like colour: a folder, a box, a page, a note, a window, a
/// film and a magnet, all stroked at the same weight as the other sidebar marks.
fn paint_category_icon(p: &egui::Painter, center: Pos2, category: Category, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let r = Rect::from_center_size(center, vec2(16.0, 16.0));
    let x = center.x;
    let y = center.y;
    match category {
        Category::General => {
            let body = Rect::from_min_max(
                pos2(r.left() + 0.5, r.top() + 4.0),
                pos2(r.right() - 0.5, r.bottom() - 1.0),
            );
            p.rect_stroke(body, 2.5, stroke, StrokeKind::Inside);
            p.line_segment(
                [
                    pos2(body.left() + 1.0, body.top() + 0.2),
                    pos2(body.left() + 5.5, body.top() + 0.2),
                ],
                stroke,
            );
        }
        Category::Compressed => {
            let box_rect = Rect::from_center_size(center, vec2(13.0, 15.0));
            p.rect_stroke(box_rect, 2.5, stroke, StrokeKind::Inside);
            p.line_segment(
                [
                    pos2(x, box_rect.top() + 2.0),
                    pos2(x, box_rect.bottom() - 2.0),
                ],
                stroke,
            );
            p.line_segment([pos2(x - 2.0, y - 1.0), pos2(x + 2.0, y - 1.0)], stroke);
        }
        Category::Documents => {
            let page = Rect::from_min_max(
                pos2(r.left() + 2.5, r.top() + 0.5),
                pos2(r.right() - 2.0, r.bottom() - 0.5),
            );
            p.line_segment(
                [page.left_top(), pos2(page.right() - 4.0, page.top())],
                stroke,
            );
            p.line_segment(
                [pos2(page.right() - 4.0, page.top()), page.right_top()],
                stroke,
            );
            p.line_segment(
                [page.right_top(), pos2(page.right(), page.top() + 4.5)],
                stroke,
            );
            p.line_segment(
                [pos2(page.right(), page.top() + 4.5), page.right_bottom()],
                stroke,
            );
            p.line_segment([page.right_bottom(), page.left_bottom()], stroke);
            p.line_segment([page.left_bottom(), page.left_top()], stroke);
            p.line_segment(
                [
                    pos2(page.left() + 3.0, y - 2.0),
                    pos2(page.right() - 3.0, y - 2.0),
                ],
                stroke,
            );
            p.line_segment(
                [
                    pos2(page.left() + 3.0, y + 1.5),
                    pos2(page.right() - 3.0, y + 1.5),
                ],
                stroke,
            );
        }
        Category::Music => {
            p.circle_stroke(pos2(x - 3.5, y + 4.0), 2.6, stroke);
            p.line_segment([pos2(x - 1.0, y + 4.0), pos2(x - 1.0, y - 6.0)], stroke);
            p.line_segment([pos2(x - 1.0, y - 6.0), pos2(x + 4.5, y - 7.5)], stroke);
        }
        Category::Programs => {
            let app = Rect::from_center_size(center, vec2(15.0, 14.0));
            p.rect_stroke(app, 3.0, stroke, StrokeKind::Inside);
            p.line_segment(
                [
                    pos2(app.left() + 1.0, app.top() + 4.0),
                    pos2(app.right() - 1.0, app.top() + 4.0),
                ],
                stroke,
            );
            p.circle_filled(pos2(app.left() + 3.5, app.top() + 2.2), 0.9, color);
        }
        Category::Video => {
            let film = Rect::from_center_size(center, vec2(16.0, 12.0));
            p.rect_stroke(film, 2.5, stroke, StrokeKind::Inside);
            p.add(egui::Shape::convex_polygon(
                vec![
                    pos2(x - 2.5, y - 3.4),
                    pos2(x - 2.5, y + 3.4),
                    pos2(x + 3.6, y),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Category::Torrents => {
            let arc: Vec<Pos2> = (0..=8)
                .map(|i| {
                    let t = i as f32 / 8.0;
                    let a = std::f32::consts::PI * (0.15 + 0.70 * t);
                    pos2(x - 6.5 * a.cos(), y + 1.0 - 6.5 * a.sin())
                })
                .collect();
            p.add(egui::Shape::line(arc, stroke));
            p.line_segment([pos2(x - 6.5, y + 1.0), pos2(x - 6.5, y + 6.0)], stroke);
            p.line_segment([pos2(x + 6.5, y + 1.0), pos2(x + 6.5, y + 6.0)], stroke);
        }
        Category::Custom(_) => {
            p.circle_stroke(center, 7.0, stroke);
            p.circle_filled(center, 1.6, color);
        }
    }
}

fn icon_button(ui: &mut Ui, icon: Icon, enabled: bool, colors: MacColors, tip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        vec2(30.0, 30.0),
        if enabled {
            winui::pointer_only()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, enabled, crate::i18n::t(tip))
    });
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 7.0, colors.hover);
    }
    icon.image(enabled)
        .tint(if enabled { colors.text } else { colors.subtle })
        .paint_at(ui, rect.shrink(7.0));
    response.on_hover_text(crate::i18n::t(tip))
}

/// The app menu's button: three rules, 7 points each side of the centre. A plain "..." used
/// to sit here, which read as text next to the painted icons.
fn menu_button(ui: &mut Ui, colors: MacColors) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(30.0, 30.0), winui::pointer_only());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, crate::i18n::t("More"))
    });
    let p = ui.painter();
    if response.hovered() {
        p.rect_filled(rect, 7.0, colors.hover);
    }
    let stroke = Stroke::new(
        1.7,
        if response.hovered() {
            colors.text
        } else {
            colors.muted
        },
    );
    for dy in [-4.5_f32, 0.0, 4.5] {
        p.line_segment(
            [
                pos2(rect.center().x - 7.0, rect.center().y + dy),
                pos2(rect.center().x + 7.0, rect.center().y + dy),
            ],
            stroke,
        );
    }
    response.on_hover_text(crate::i18n::t("More"))
}

/// One title above the list. It sorts the way the Windows column headers do (#A8): clicking a
/// different column starts ascending, clicking the same one flips it.
fn column_header(
    ui: &Ui,
    label: &str,
    column: usize,
    x: f32,
    center_y: f32,
    sort: Option<(usize, bool)>,
    colors: MacColors,
) -> bool {
    let font = FontId::proportional(11.5);
    let sorted = sort.map(|(current, _)| current) == Some(column);
    let mut text = crate::i18n::t(label).to_owned();
    if sorted {
        text.push(' ');
        text.push(if sort.is_some_and(|(_, ascending)| ascending) {
            '\u{25B2}'
        } else {
            '\u{25BC}'
        });
    }
    let width = winui::text_width_font(ui.painter(), font.clone(), &text) + 16.0;
    // The file name starts at the row's left edge; size and progress are right-aligned columns.
    let center_x = if column == 0 {
        x + width / 2.0
    } else {
        x - width / 2.0
    };
    let rect = Rect::from_center_size(pos2(center_x, center_y), vec2(width, COLUMNS_H - 8.0));
    let response = ui.interact(
        rect,
        egui::Id::unique(("macos-column", column)),
        winui::pointer_only(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text.clone()));
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, colors.hover);
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        font,
        if sorted { colors.text } else { colors.muted },
    );
    response.clicked()
}

fn paint_status_icon(p: &egui::Painter, center: Pos2, status: Status, color: Color32) {
    p.circle_filled(center, 7.0, color);
    let white = Color32::WHITE;
    match status {
        Status::Complete => {
            p.line_segment(
                [
                    pos2(center.x - 3.0, center.y),
                    pos2(center.x - 0.5, center.y + 2.5),
                ],
                Stroke::new(1.5, white),
            );
            p.line_segment(
                [
                    pos2(center.x - 0.5, center.y + 2.5),
                    pos2(center.x + 3.8, center.y - 2.8),
                ],
                Stroke::new(1.5, white),
            );
        }
        Status::Error => {
            p.line_segment(
                [
                    pos2(center.x, center.y - 3.2),
                    pos2(center.x, center.y + 1.3),
                ],
                Stroke::new(1.5, white),
            );
            p.circle_filled(pos2(center.x, center.y + 3.6), 0.9, white);
        }
        Status::Stopped => {
            p.line_segment(
                [
                    pos2(center.x - 2.0, center.y - 3.0),
                    pos2(center.x - 2.0, center.y + 3.0),
                ],
                Stroke::new(1.5, white),
            );
            p.line_segment(
                [
                    pos2(center.x + 2.0, center.y - 3.0),
                    pos2(center.x + 2.0, center.y + 3.0),
                ],
                Stroke::new(1.5, white),
            );
        }
        Status::Queued => {
            p.circle_stroke(center, 3.0, Stroke::new(1.2, white));
        }
        Status::Downloading => {
            p.line_segment(
                [
                    pos2(center.x, center.y - 3.5),
                    pos2(center.x, center.y + 3.0),
                ],
                Stroke::new(1.5, white),
            );
            p.line_segment(
                [
                    pos2(center.x - 2.8, center.y + 0.5),
                    pos2(center.x, center.y + 3.2),
                ],
                Stroke::new(1.5, white),
            );
            p.line_segment(
                [
                    pos2(center.x + 2.8, center.y + 0.5),
                    pos2(center.x, center.y + 3.2),
                ],
                Stroke::new(1.5, white),
            );
        }
    }
}

fn status_color(status: Status, colors: MacColors) -> Color32 {
    match status {
        Status::Downloading => colors.accent,
        Status::Queued | Status::Stopped => colors.amber,
        Status::Complete => colors.green,
        Status::Error => colors.red,
    }
}

fn status_group_name(status: Status) -> &'static str {
    match status {
        Status::Downloading => "Downloading",
        Status::Queued => "Waiting",
        Status::Stopped => "Paused",
        Status::Complete => "Finished",
        Status::Error => "Error",
    }
}

fn category_colors() -> Vec<(Category, Color32)> {
    let colors = [
        Color32::from_rgb(0x8e, 0x8e, 0x93),
        Color32::from_rgb(0xaf, 0x52, 0xde),
        Color32::from_rgb(0xff, 0x2d, 0x55),
        Color32::from_rgb(0x00, 0x7a, 0xff),
        Color32::from_rgb(0xff, 0x95, 0x00),
        Color32::from_rgb(0x34, 0xc7, 0x59),
        Color32::from_rgb(0x5a, 0xc8, 0xfa),
        Color32::from_rgb(0x58, 0x56, 0xd6),
    ];
    Category::all()
        .iter()
        .copied()
        .enumerate()
        .map(|(index, category)| (category, colors[index % colors.len()]))
        .collect()
}
