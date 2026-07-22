use crate::*;
use eframe::egui;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use winit::event_loop::{EventLoop, EventLoopProxy};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const APP_TITLE: &str = "LumiControl";
const SENSOR_CURVE_HISTORY_LIMIT: usize = 3;

#[derive(Clone, Copy, Debug)]
enum AppEvent {
    Show,
    RunNow,
    TogglePause,
    Calibration,
    Settings,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlSource {
    Sensor,
}

impl ControlSource {
    fn label(self) -> &'static str {
        match self {
            ControlSource::Sensor => "Sensor",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            ControlSource::Sensor => "BH1750 lux curve",
        }
    }
}

impl AppEvent {
    fn opens_window_before_egui_update(self) -> bool {
        matches!(
            self,
            AppEvent::Show
                | AppEvent::RunNow
                | AppEvent::TogglePause
                | AppEvent::Calibration
                | AppEvent::Settings
        )
    }

    fn exits_without_egui_update(self) -> bool {
        matches!(self, AppEvent::Quit)
    }
}

fn initial_viewport_visible_on_launch() -> bool {
    true
}

fn initial_viewport_resizable() -> bool {
    true
}

fn initial_viewport_size() -> [f32; 2] {
    [520.0, 300.0]
}

fn settings_brightness_range() -> RangeInclusive<i32> {
    monitor_brightness_range()
}

fn theme_label(theme_mode: &str) -> &'static str {
    match theme_mode {
        "light" => "Light",
        "system" => "System",
        _ => "Dark",
    }
}

fn dark_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(13, 19, 30);
    visuals.window_fill = egui::Color32::from_rgb(21, 29, 42);
    visuals.extreme_bg_color = egui::Color32::from_rgb(9, 13, 22);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(21, 29, 42);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(31, 42, 58);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(39, 54, 75);
    visuals.selection.bg_fill = egui::Color32::from_rgb(76, 158, 255);
    visuals
}

fn use_light_visuals(theme_mode: &str, ctx: &egui::Context) -> bool {
    match theme_mode {
        "light" => true,
        "system" => matches!(ctx.system_theme(), Some(egui::Theme::Light)),
        _ => false,
    }
}

fn primary_status_text(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(22.0)
        .strong()
        .color(egui::Color32::from_rgb(238, 245, 255))
}

fn secondary_status_text(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(19.0)
        .strong()
        .color(egui::Color32::from_rgb(214, 224, 238))
}

fn sensor_value_text(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(20.0)
        .strong()
        .color(egui::Color32::from_rgb(224, 236, 252))
}

fn light_strip_status_text(light_on: Option<bool>) -> egui::RichText {
    let (label, color) = match light_on {
        Some(true) => ("On", egui::Color32::from_rgb(255, 207, 92)),
        Some(false) => ("Off", egui::Color32::from_rgb(255, 164, 92)),
        None => ("Unknown", egui::Color32::from_rgb(255, 100, 116)),
    };
    egui::RichText::new(label).strong().color(color)
}

fn normalize_settings_brightness(value: i32) -> i32 {
    normalize_monitor_brightness(value)
}

fn sensor_curve_editor_default_point(points: &[SensorCurvePoint]) -> SensorCurvePoint {
    points.last().cloned().unwrap_or(SensorCurvePoint {
        lux: 80.0,
        brightness: 72,
    })
}

fn sensor_curve_plot_position(
    rect: egui::Rect,
    lux: f64,
    brightness: i32,
    min_lux: f64,
    max_lux: f64,
) -> egui::Pos2 {
    let min_log = min_lux.log10();
    let max_log = max_lux.log10();
    let span = (max_log - min_log).max(0.001);
    let x = ((lux.log10() - min_log) / span).clamp(0.0, 1.0) as f32;
    let y = (brightness as f32 / 100.0).clamp(0.0, 1.0);
    egui::pos2(
        egui::lerp(rect.left()..=rect.right(), x),
        egui::lerp(rect.bottom()..=rect.top(), y),
    )
}

fn sensor_curve_point_from_plot_position(
    rect: egui::Rect,
    position: egui::Pos2,
    min_lux: f64,
    max_lux: f64,
) -> SensorCurvePoint {
    let x = ((position.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
    let y = ((rect.bottom() - position.y) / rect.height().max(1.0)).clamp(0.0, 1.0);
    let min_log = min_lux.max(0.001).log10();
    let max_log = max_lux.max(min_lux * 1.01).log10();
    let lux = 10_f64.powf(min_log + (max_log - min_log) * x);
    SensorCurvePoint {
        lux,
        brightness: normalize_monitor_brightness((y * 100.0).round() as i32),
    }
}

fn remember_sensor_curve_history(
    history: &mut Vec<Vec<SensorCurvePoint>>,
    curve: &[SensorCurvePoint],
) {
    let normalized = normalize_sensor_curve(curve);
    if history.last() == Some(&normalized) {
        return;
    }
    history.push(normalized);
    while history.len() > SENSOR_CURVE_HISTORY_LIMIT {
        history.remove(0);
    }
}

fn pop_sensor_curve_history(
    history: &mut Vec<Vec<SensorCurvePoint>>,
) -> Option<Vec<SensorCurvePoint>> {
    history.pop()
}

fn sensor_curve_summary(points: &[SensorCurvePoint]) -> String {
    normalize_sensor_curve(points)
        .iter()
        .map(|point| format!("{:.0} lx -> {}%", point.lux, point.brightness))
        .collect::<Vec<_>>()
        .join("   ")
}

fn compact_run_time(last_run: Option<&str>) -> String {
    let Some(last_run) = last_run else {
        return "Waiting".to_string();
    };
    last_run
        .split('T')
        .nth(1)
        .and_then(|time| time.split('+').next())
        .filter(|time| !time.is_empty())
        .unwrap_or(last_run)
        .to_string()
}

fn elapsed_update_label(last_run_instant: Option<Instant>) -> String {
    let Some(last_run_instant) = last_run_instant else {
        return "Waiting".to_string();
    };
    let seconds = last_run_instant.elapsed().as_secs();
    if seconds < 60 {
        format!("{seconds}s ago")
    } else {
        format!("{}m ago", seconds / 60)
    }
}

fn draw_sensor_curve_mini(ui: &mut egui::Ui, points: &[SensorCurvePoint]) {
    let width = ui.available_width().clamp(220.0, 500.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 78.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 5.0, ui.visuals().extreme_bg_color);
    painter.rect_stroke(
        rect,
        5.0,
        egui::Stroke::new(1.0, ui.visuals().widgets.inactive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );

    let plot = rect.shrink2(egui::vec2(12.0, 10.0));
    let curve = normalize_sensor_curve(points);
    let min_lux = curve
        .first()
        .map(|point| point.lux)
        .unwrap_or(20.0)
        .max(0.001);
    let max_lux = curve
        .last()
        .map(|point| point.lux)
        .unwrap_or(250.0)
        .max(min_lux * 1.01);
    let positions = curve
        .iter()
        .map(|point| {
            sensor_curve_plot_position(plot, point.lux, point.brightness, min_lux, max_lux)
        })
        .collect::<Vec<_>>();

    if positions.len() >= 2 {
        painter.add(egui::Shape::line(
            positions.clone(),
            egui::Stroke::new(2.0, egui::Color32::from_rgb(94, 184, 255)),
        ));
    }
    for position in positions {
        painter.circle_filled(position, 3.0, egui::Color32::from_rgb(255, 207, 92));
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SensorCurvePreviewResponse {
    changed: bool,
    commit: bool,
}

fn draw_sensor_curve_preview(
    ui: &mut egui::Ui,
    points: &mut Vec<SensorCurvePoint>,
) -> SensorCurvePreviewResponse {
    let width = ui.available_width().clamp(260.0, 360.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 210.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
    painter.rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, ui.visuals().widgets.inactive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );

    let plot = rect.shrink2(egui::vec2(20.0, 18.0));
    let curve = normalize_sensor_curve(points);
    if *points != curve {
        *points = curve.clone();
    }
    let min_lux = curve
        .first()
        .map(|point| point.lux)
        .unwrap_or(20.0)
        .max(0.001);
    let max_lux = curve
        .last()
        .map(|point| point.lux)
        .unwrap_or(250.0)
        .max(min_lux * 1.01);

    for fraction in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        let x = egui::lerp(plot.left()..=plot.right(), fraction);
        painter.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_fill),
        );
        let y = egui::lerp(plot.bottom()..=plot.top(), fraction);
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_fill),
        );
    }

    let positions = curve
        .iter()
        .map(|point| {
            sensor_curve_plot_position(plot, point.lux, point.brightness, min_lux, max_lux)
        })
        .collect::<Vec<_>>();
    let mut preview_response = SensorCurvePreviewResponse::default();
    for (index, position) in positions.iter().enumerate() {
        let response = ui
            .interact(
                egui::Rect::from_center_size(*position, egui::vec2(18.0, 18.0)),
                ui.id().with(("sensor-curve-point", index)),
                egui::Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Grab);
        if response.dragged() {
            if let Some(pointer) = response.interact_pointer_pos() {
                points[index] =
                    sensor_curve_point_from_plot_position(plot, pointer, min_lux, max_lux);
                preview_response.changed = true;
            }
        }
        if response.drag_stopped() {
            preview_response.commit = true;
        }
    }

    if positions.len() >= 2 {
        painter.add(egui::Shape::line(
            positions.clone(),
            egui::Stroke::new(2.0, egui::Color32::from_rgb(94, 184, 255)),
        ));
    }
    for (point, position) in curve.iter().zip(positions.iter()) {
        painter.circle_filled(*position, 4.5, egui::Color32::from_rgb(255, 207, 92));
        painter.text(
            *position + egui::vec2(7.0, -7.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{:.0} lx / {}%", point.lux, point.brightness),
            egui::FontId::monospace(10.0),
            ui.visuals().text_color(),
        );
    }

    painter.text(
        egui::pos2(plot.left(), rect.bottom() - 4.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{min_lux:.0} lx"),
        egui::FontId::monospace(10.0),
        ui.visuals().weak_text_color(),
    );
    painter.text(
        egui::pos2(plot.right(), rect.bottom() - 4.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("{max_lux:.0} lx"),
        egui::FontId::monospace(10.0),
        ui.visuals().weak_text_color(),
    );
    painter.text(
        egui::pos2(rect.left() + 4.0, plot.top()),
        egui::Align2::LEFT_TOP,
        "100%",
        egui::FontId::monospace(10.0),
        ui.visuals().weak_text_color(),
    );
    painter.text(
        egui::pos2(rect.left() + 4.0, plot.bottom()),
        egui::Align2::LEFT_BOTTOM,
        "0%",
        egui::FontId::monospace(10.0),
        ui.visuals().weak_text_color(),
    );
    preview_response
}

fn window_open_after_action(open: bool, close_requested: bool) -> bool {
    open && !close_requested
}

fn pause_menu_label(paused: bool) -> &'static str {
    if paused {
        "Resume"
    } else {
        "Pause"
    }
}

fn visible_error<'a>(
    action_error: &'a Option<String>,
    worker_error: &'a Option<String>,
) -> Option<&'a str> {
    action_error.as_deref().or(worker_error.as_deref())
}

fn tray_click_opens_main_window(button: MouseButton, button_state: MouseButtonState) -> bool {
    button == MouseButton::Left && button_state == MouseButtonState::Up
}

fn tray_double_click_opens_main_window(button: MouseButton) -> bool {
    button == MouseButton::Left
}

const LIGHT_RULE_CONDITION_LABELS: [&str; 12] = [
    "Time after",
    "Time before",
    "After sunrise",
    "Before sunset",
    "After sunset",
    "Lux below",
    "Lux above",
    "Current brightness below",
    "Current brightness above",
    "Target brightness below",
    "Target brightness above",
    "Weather is",
];

fn default_light_rule_condition(kind: usize) -> LightRuleCondition {
    match kind {
        0 => LightRuleCondition::TimeAfter { minutes: 19 * 60 },
        1 => LightRuleCondition::TimeBefore { minutes: 7 * 60 },
        2 => LightRuleCondition::AfterSunrise { offset_minutes: 0 },
        3 => LightRuleCondition::BeforeSunset { offset_minutes: 0 },
        4 => LightRuleCondition::AfterSunset { offset_minutes: 0 },
        5 => LightRuleCondition::LuxBelow { lux: 30.0 },
        6 => LightRuleCondition::LuxAbove { lux: 80.0 },
        7 => LightRuleCondition::CurrentBrightnessBelow { brightness: 40 },
        8 => LightRuleCondition::CurrentBrightnessAbove { brightness: 60 },
        9 => LightRuleCondition::TargetBrightnessBelow { brightness: 40 },
        10 => LightRuleCondition::TargetBrightnessAbove { brightness: 60 },
        _ => LightRuleCondition::WeatherIs {
            kind: WeatherKind::Cloudy,
        },
    }
}

fn light_rule_condition_kind(condition: &LightRuleCondition) -> usize {
    match condition {
        LightRuleCondition::TimeAfter { .. } => 0,
        LightRuleCondition::TimeBefore { .. } => 1,
        LightRuleCondition::AfterSunrise { .. } => 2,
        LightRuleCondition::BeforeSunset { .. } => 3,
        LightRuleCondition::AfterSunset { .. } => 4,
        LightRuleCondition::LuxBelow { .. } => 5,
        LightRuleCondition::LuxAbove { .. } => 6,
        LightRuleCondition::CurrentBrightnessBelow { .. } => 7,
        LightRuleCondition::CurrentBrightnessAbove { .. } => 8,
        LightRuleCondition::TargetBrightnessBelow { .. } => 9,
        LightRuleCondition::TargetBrightnessAbove { .. } => 10,
        LightRuleCondition::WeatherIs { .. } => 11,
    }
}

fn minutes_label(minutes: i32) -> String {
    let minutes = normalize_day_minutes(minutes);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn ui_time_minutes(ui: &mut egui::Ui, minutes: &mut i32) -> bool {
    let changed = ui
        .add(egui::DragValue::new(minutes).speed(5).range(0..=1439))
        .changed();
    *minutes = normalize_day_minutes(*minutes);
    ui.label(minutes_label(*minutes));
    changed
}

fn ui_light_rule_action(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    action: &mut LightRuleAction,
) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(action.label())
        .show_ui(ui, |ui| {
            changed |= ui
                .selectable_value(action, LightRuleAction::Keep, "Keep")
                .changed();
            changed |= ui
                .selectable_value(action, LightRuleAction::On, "On")
                .changed();
            changed |= ui
                .selectable_value(action, LightRuleAction::Off, "Off")
                .changed();
        });
    changed
}

fn ui_light_rule_condition(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + Clone,
    condition: &mut LightRuleCondition,
) -> bool {
    let mut changed = false;
    let mut kind = light_rule_condition_kind(condition);
    egui::ComboBox::from_id_salt(("light-rule-condition-kind", id.clone()))
        .selected_text(LIGHT_RULE_CONDITION_LABELS[kind])
        .show_ui(ui, |ui| {
            for (index, label) in LIGHT_RULE_CONDITION_LABELS.iter().enumerate() {
                changed |= ui.selectable_value(&mut kind, index, *label).changed();
            }
        });
    if kind != light_rule_condition_kind(condition) {
        *condition = default_light_rule_condition(kind);
        changed = true;
    }

    match condition {
        LightRuleCondition::TimeAfter { minutes } | LightRuleCondition::TimeBefore { minutes } => {
            changed |= ui_time_minutes(ui, minutes);
        }
        LightRuleCondition::AfterSunrise { offset_minutes }
        | LightRuleCondition::BeforeSunset { offset_minutes }
        | LightRuleCondition::AfterSunset { offset_minutes } => {
            ui.label("offset min");
            changed |= ui
                .add(
                    egui::DragValue::new(offset_minutes)
                        .speed(5)
                        .range(-180..=180),
                )
                .changed();
        }
        LightRuleCondition::LuxBelow { lux } | LightRuleCondition::LuxAbove { lux } => {
            ui.label("lux");
            changed |= ui
                .add(egui::DragValue::new(lux).speed(1.0).range(0.0..=100_000.0))
                .changed();
        }
        LightRuleCondition::CurrentBrightnessBelow { brightness }
        | LightRuleCondition::CurrentBrightnessAbove { brightness }
        | LightRuleCondition::TargetBrightnessBelow { brightness }
        | LightRuleCondition::TargetBrightnessAbove { brightness } => {
            ui.label("%");
            changed |= ui
                .add(egui::DragValue::new(brightness).range(settings_brightness_range()))
                .changed();
        }
        LightRuleCondition::WeatherIs { kind } => {
            egui::ComboBox::from_id_salt(("light-rule-weather", id))
                .selected_text(kind.label())
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(kind, WeatherKind::Clear, "Clear")
                        .changed();
                    changed |= ui
                        .selectable_value(kind, WeatherKind::Cloudy, "Cloudy")
                        .changed();
                    changed |= ui
                        .selectable_value(kind, WeatherKind::Rain, "Rain")
                        .changed();
                    changed |= ui.selectable_value(kind, WeatherKind::Fog, "Fog").changed();
                });
        }
    }
    changed
}

fn ui_light_rule_condition_group(
    ui: &mut egui::Ui,
    id_prefix: &str,
    label: &str,
    conditions: &mut Vec<LightRuleCondition>,
) -> bool {
    let mut changed = false;
    let mut remove_index = None;
    ui.horizontal(|ui| {
        ui.strong(label);
        if ui.small_button("+").clicked() {
            conditions.push(default_light_rule_condition(5));
            changed = true;
        }
    });
    if conditions.is_empty() {
        ui.label(egui::RichText::new("No conditions").small());
    }
    for (index, condition) in conditions.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= ui_light_rule_condition(ui, (id_prefix.to_string(), index), condition);
            if ui.small_button("Remove").clicked() {
                remove_index = Some(index);
            }
        });
    }
    if let Some(index) = remove_index {
        conditions.remove(index);
        changed = true;
    }
    changed
}

pub fn run_native_gui(
    config: &AppConfig,
    runtime_path: &Path,
    runtime: &mut RuntimeConfig,
    gateway: &Dxva2MonitorGateway,
    cache: &mut WeatherCache,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = config.clone();
    let runtime_path = runtime_path.to_path_buf();
    let runtime = runtime.clone();
    let gateway = gateway.clone();
    let cache = cache.clone();
    let event_loop = EventLoop::<eframe::UserEvent>::with_user_event().build()?;
    let repaint_proxy = event_loop.create_proxy();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_TITLE)
            .with_inner_size(initial_viewport_size())
            .with_resizable(initial_viewport_resizable())
            .with_visible(initial_viewport_visible_on_launch()),
        ..Default::default()
    };

    let mut app = eframe::create_native(
        APP_TITLE,
        options,
        Box::new(move |cc| {
            let app = LumiApp::new(
                config.clone(),
                runtime_path.clone(),
                runtime.clone(),
                gateway.clone(),
                cache.clone(),
                cc.egui_ctx.clone(),
                repaint_proxy.clone(),
            )
            .map_err(|err| -> Box<dyn std::error::Error + Send + Sync> {
                err.to_string().into()
            })?;
            Ok(Box::new(app))
        }),
        &event_loop,
    );

    event_loop
        .run_app(&mut app)
        .map_err(|err| format!("failed to run egui event loop: {err}").into())
}

fn wake_root_viewport(repaint_proxy: &EventLoopProxy<eframe::UserEvent>, ctx: &egui::Context) {
    let _ = repaint_proxy.send_event(eframe::UserEvent::RequestRepaint {
        viewport_id: egui::ViewportId::ROOT,
        when: Instant::now(),
        cumulative_pass_nr: ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT),
    });
}

fn log_tray_event(message: &str) {
    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let dir = PathBuf::from(local_app_data).join(APP_TITLE);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("tray.log");
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{:?} {}", SystemTime::now(), message);
}

#[cfg(target_os = "windows")]
fn show_main_window_os() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow,
        SW_RESTORE,
    };

    struct WindowSearch {
        pid: u32,
        hwnd: HWND,
    }

    unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        let search = unsafe { &mut *(lparam as *mut WindowSearch) };
        let mut owner_pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut owner_pid);
        }
        if owner_pid != search.pid {
            return 1;
        }

        let mut title = [0u16; 256];
        let len = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
        if len <= 0 {
            return 1;
        }

        let title = String::from_utf16_lossy(&title[..len as usize]);
        if title == APP_TITLE {
            search.hwnd = hwnd;
            return 0;
        }

        1
    }

    let mut search = WindowSearch {
        pid: std::process::id(),
        hwnd: std::ptr::null_mut(),
    };
    unsafe {
        EnumWindows(
            Some(enum_window),
            &mut search as *mut WindowSearch as LPARAM,
        );
    }

    if search.hwnd.is_null() {
        log_tray_event("show requested but main hwnd was not found for current process");
        return;
    }

    unsafe {
        ShowWindow(search.hwnd, SW_RESTORE);
        SetForegroundWindow(search.hwnd);
    }
    log_tray_event("showed main window with Win32");
}

#[cfg(not(target_os = "windows"))]
fn show_main_window_os() {}

fn quit_from_tray() -> ! {
    log_tray_event("quit requested from tray");
    std::process::exit(0);
}

struct TrayState {
    _icon: TrayIcon,
    pause_item: MenuItem,
}

#[derive(Clone, Debug)]
struct MonitorSnapshot {
    monitors: Vec<MonitorInfo>,
    monitor_brightness: BTreeMap<String, i32>,
}

fn apply_monitor_snapshot(
    current_monitors: &mut Vec<MonitorInfo>,
    current_brightness: &mut BTreeMap<String, i32>,
    selected_monitor: &mut usize,
    snapshot: MonitorSnapshot,
) {
    *current_monitors = snapshot.monitors;
    *current_brightness = snapshot.monitor_brightness;
    if current_monitors.is_empty() {
        *selected_monitor = 0;
    } else {
        *selected_monitor = (*selected_monitor).min(current_monitors.len() - 1);
    }
}

#[derive(Clone, Debug)]
struct ControlSnapshot {
    monitor_snapshot: Option<MonitorSnapshot>,
    light_rule_snapshot: Option<LightRuleSnapshot>,
    last_control_source: ControlSource,
    last_sensor_lux: Option<f64>,
    last_target: Option<i32>,
    last_next_brightness: Option<i32>,
    last_current_before_step: Option<i32>,
    last_relay_state: Option<RelayState>,
    last_relay_gpio: Option<i32>,
    last_run: Option<String>,
    last_run_instant: Option<Instant>,
}

#[derive(Clone, Debug)]
struct LightRuleSnapshot {
    matched_rule: Option<String>,
    action: LightRuleAction,
}

#[derive(Clone, Debug)]
struct ControlWorkerUpdate {
    snapshot: Option<ControlSnapshot>,
    error: Option<String>,
}

#[derive(Clone, Debug)]
enum ControlWorkerCommand {
    RunNow,
    UpdateRuntime(RuntimeConfig),
    SetLightStrip(bool),
    Stop,
}

struct ControlWorkerHandle {
    commands: Sender<ControlWorkerCommand>,
    updates: Receiver<ControlWorkerUpdate>,
    _thread: thread::JoinHandle<()>,
}

struct ControlWorkerIo {
    command_rx: Receiver<ControlWorkerCommand>,
    update_tx: Sender<ControlWorkerUpdate>,
    repaint_proxy: EventLoopProxy<eframe::UserEvent>,
    ctx: egui::Context,
}

impl ControlWorkerHandle {
    fn spawn(
        config: AppConfig,
        runtime: RuntimeConfig,
        gateway: Dxva2MonitorGateway,
        mut weather_cache: WeatherCache,
        repaint_proxy: EventLoopProxy<eframe::UserEvent>,
        ctx: egui::Context,
    ) -> Self {
        let (command_tx, command_rx) = channel();
        let (update_tx, update_rx) = channel();
        let thread = thread::spawn(move || {
            control_worker_loop(
                config,
                runtime,
                gateway,
                &mut weather_cache,
                ControlWorkerIo {
                    command_rx,
                    update_tx,
                    repaint_proxy,
                    ctx,
                },
            );
        });
        Self {
            commands: command_tx,
            updates: update_rx,
            _thread: thread,
        }
    }

    fn send(&self, command: ControlWorkerCommand) -> Result<(), Box<dyn std::error::Error>> {
        self.commands
            .send(command)
            .map_err(|err| format!("control worker unavailable: {err}").into())
    }
}

impl Drop for ControlWorkerHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(ControlWorkerCommand::Stop);
    }
}

fn send_worker_update(
    update_tx: &Sender<ControlWorkerUpdate>,
    repaint_proxy: &EventLoopProxy<eframe::UserEvent>,
    ctx: &egui::Context,
    update: ControlWorkerUpdate,
) {
    let _ = update_tx.send(update);
    wake_root_viewport(repaint_proxy, ctx);
}

fn control_worker_loop(
    config: AppConfig,
    mut runtime: RuntimeConfig,
    gateway: Dxva2MonitorGateway,
    weather_cache: &mut WeatherCache,
    io: ControlWorkerIo,
) {
    let ControlWorkerIo {
        command_rx,
        update_tx,
        repaint_proxy,
        ctx,
    } = io;
    let mut next_tick = Instant::now();
    loop {
        let now = Instant::now();
        let wait = next_tick.saturating_duration_since(now);
        match command_rx.recv_timeout(wait) {
            Ok(ControlWorkerCommand::RunNow) => {
                run_worker_control_once(
                    &config,
                    &runtime,
                    &gateway,
                    weather_cache,
                    &update_tx,
                    &repaint_proxy,
                    &ctx,
                );
                next_tick =
                    Instant::now() + Duration::from_secs(config.control_tick_seconds.max(1));
            }
            Ok(ControlWorkerCommand::UpdateRuntime(next_runtime)) => {
                runtime = next_runtime;
            }
            Ok(ControlWorkerCommand::SetLightStrip(light_on)) => {
                let relay_state = runtime.relay_contact_mode.relay_state_for_light(light_on);
                let update = match send_relay_command(&config.sensor_port, relay_state) {
                    Ok(response) => ControlWorkerUpdate {
                        snapshot: Some(ControlSnapshot {
                            monitor_snapshot: None,
                            light_rule_snapshot: None,
                            last_control_source: ControlSource::Sensor,
                            last_sensor_lux: None,
                            last_target: None,
                            last_next_brightness: None,
                            last_current_before_step: None,
                            last_relay_state: Some(response.relay),
                            last_relay_gpio: Some(response.relay_gpio),
                            last_run: None,
                            last_run_instant: None,
                        }),
                        error: None,
                    },
                    Err(err) => ControlWorkerUpdate {
                        snapshot: None,
                        error: Some(err.to_string()),
                    },
                };
                send_worker_update(&update_tx, &repaint_proxy, &ctx, update);
            }
            Ok(ControlWorkerCommand::Stop)
            | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if !runtime.paused {
                    run_worker_control_once(
                        &config,
                        &runtime,
                        &gateway,
                        weather_cache,
                        &update_tx,
                        &repaint_proxy,
                        &ctx,
                    );
                }
                next_tick =
                    Instant::now() + Duration::from_secs(config.control_tick_seconds.max(1));
            }
        }
    }
}

fn run_worker_control_once(
    config: &AppConfig,
    runtime: &RuntimeConfig,
    gateway: &Dxva2MonitorGateway,
    weather_cache: &mut WeatherCache,
    update_tx: &Sender<ControlWorkerUpdate>,
    repaint_proxy: &EventLoopProxy<eframe::UserEvent>,
    ctx: &egui::Context,
) {
    let update = match run_control_once(config, runtime, gateway, weather_cache) {
        Ok((snapshot, error)) => ControlWorkerUpdate {
            snapshot: Some(snapshot),
            error,
        },
        Err(err) => ControlWorkerUpdate {
            snapshot: None,
            error: Some(err.to_string()),
        },
    };
    send_worker_update(update_tx, repaint_proxy, ctx, update);
}

type MonitorReadout = (Vec<MonitorInfo>, BTreeMap<String, i32>);

fn read_monitor_snapshot<G: MonitorGateway>(
    gateway: &G,
) -> Result<MonitorReadout, Box<dyn std::error::Error>> {
    let monitors = gateway.list_monitor_info()?;
    let mut monitor_brightness = BTreeMap::new();
    for monitor in &monitors {
        if let Ok(value) = gateway.get_brightness(&monitor.identifier) {
            monitor_brightness.insert(monitor.identifier.clone(), value);
        }
    }
    Ok((monitors, monitor_brightness))
}

fn run_control_once<G: MonitorGateway>(
    config: &AppConfig,
    runtime: &RuntimeConfig,
    gateway: &G,
    weather_cache: &mut WeatherCache,
) -> Result<(ControlSnapshot, Option<String>), Box<dyn std::error::Error>> {
    let now = LocalDateTime::now_with_offset(timezone_offset_minutes(&config.timezone_name));
    let readings = read_sensor_samples_with_min_deadline(&config.sensor_port, 1, 3)?;
    let lux = readings.iter().map(|reading| reading.lux).sum::<f64>() / readings.len() as f64;
    let latest_reading = readings.last();
    let policy = policy_from_config(config);
    let target = compute_sensor_brightness_target(lux, runtime, policy);
    let (monitors_before, monitor_brightness_before) = read_monitor_snapshot(gateway)?;
    let current_before_step = monitors_before
        .first()
        .and_then(|monitor| monitor_brightness_before.get(&monitor.identifier).copied());
    let next_brightness =
        current_before_step.map(|current| smooth_brightness_step(current, target, policy));
    let mut relay_state = latest_reading.and_then(|reading| reading.relay);
    let mut relay_gpio = latest_reading.and_then(|reading| reading.relay_gpio);
    let mut light_rule_match = None;
    let mut light_rule_action = LightRuleAction::Keep;
    let mut relay_error = None;

    if runtime.light_rules_enabled {
        let weather = if light_rules_need_weather(&runtime.light_rules) {
            fetch_weather(config, weather_cache)
        } else {
            None
        };
        let context = build_light_rule_context(
            config,
            now,
            weather,
            Some(lux),
            current_before_step,
            Some(target),
        );
        let decision = evaluate_light_rules(
            &runtime.light_rules,
            runtime.light_rules_fallback_action,
            &context,
        );
        light_rule_match = decision.matched_rule.clone();
        light_rule_action = decision.action;
        if let Some(light_on) = decision.action.light_on() {
            let current_light_on =
                relay_state.map(|relay| relay.light_on(runtime.relay_contact_mode));
            if current_light_on != Some(light_on) {
                let relay_target = runtime.relay_contact_mode.relay_state_for_light(light_on);
                match send_relay_command(&config.sensor_port, relay_target) {
                    Ok(response) => {
                        relay_state = Some(response.relay);
                        relay_gpio = Some(response.relay_gpio);
                    }
                    Err(err) => {
                        relay_error = Some(err.to_string());
                    }
                }
            }
        }
    }

    let monitor_brightness = if runtime.paused {
        monitor_brightness_before
    } else {
        apply_brightness_step_from_snapshot(
            gateway,
            &monitors_before,
            &monitor_brightness_before,
            target,
            policy,
            false,
        )
    };
    Ok((
        ControlSnapshot {
            monitor_snapshot: Some(MonitorSnapshot {
                monitors: monitors_before,
                monitor_brightness,
            }),
            light_rule_snapshot: Some(LightRuleSnapshot {
                matched_rule: light_rule_match,
                action: light_rule_action,
            }),
            last_control_source: ControlSource::Sensor,
            last_sensor_lux: Some(lux),
            last_target: Some(target),
            last_next_brightness: next_brightness,
            last_current_before_step: current_before_step,
            last_relay_state: relay_state,
            last_relay_gpio: relay_gpio,
            last_run: Some(now.iso8601()),
            last_run_instant: Some(Instant::now()),
        },
        relay_error,
    ))
}

impl TrayState {
    fn new(
        paused: bool,
        event_tx: Sender<AppEvent>,
        repaint_proxy: EventLoopProxy<eframe::UserEvent>,
        ctx: egui::Context,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let menu = Menu::new();
        let open = MenuItem::new("Open", true, None);
        let run = MenuItem::new("Run now", true, None);
        let pause = MenuItem::new(pause_menu_label(paused), true, None);
        let calibration = MenuItem::new("Calibration", true, None);
        let settings = MenuItem::new("Settings", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let separator = PredefinedMenuItem::separator();

        let open_id = open.id().clone();
        let run_id = run.id().clone();
        let pause_id = pause.id().clone();
        let calibration_id = calibration.id().clone();
        let settings_id = settings.id().clone();
        let quit_id = quit.id().clone();

        menu.append(&open)?;
        menu.append(&run)?;
        menu.append(&pause)?;
        menu.append(&calibration)?;
        menu.append(&settings)?;
        menu.append(&separator)?;
        menu.append(&quit)?;

        let icon = make_icon()?;
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(APP_TITLE)
            .with_icon(icon)
            .build()?;

        let menu_tx = event_tx.clone();
        let menu_proxy = repaint_proxy.clone();
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let app_event = if event.id == open_id {
                Some(AppEvent::Show)
            } else if event.id == run_id {
                Some(AppEvent::RunNow)
            } else if event.id == pause_id {
                Some(AppEvent::TogglePause)
            } else if event.id == calibration_id {
                Some(AppEvent::Calibration)
            } else if event.id == settings_id {
                Some(AppEvent::Settings)
            } else if event.id == quit_id {
                Some(AppEvent::Quit)
            } else {
                None
            };

            if let Some(app_event) = app_event {
                log_tray_event(&format!("menu event: {app_event:?}"));
                if app_event.exits_without_egui_update() {
                    quit_from_tray();
                }
                if app_event.opens_window_before_egui_update() {
                    show_main_window_os();
                }
                let _ = menu_tx.send(app_event);
                wake_root_viewport(&menu_proxy, &menu_ctx);
            }
        }));

        let tray_tx = event_tx;
        let tray_ctx = ctx;
        let tray_proxy = repaint_proxy;
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let opens_main_window = match event {
                TrayIconEvent::Click {
                    button,
                    button_state,
                    ..
                } => tray_click_opens_main_window(button, button_state),
                TrayIconEvent::DoubleClick { button, .. } => {
                    tray_double_click_opens_main_window(button)
                }
                _ => false,
            };
            if opens_main_window {
                log_tray_event("tray icon click");
                show_main_window_os();
                let _ = tray_tx.send(AppEvent::Show);
                wake_root_viewport(&tray_proxy, &tray_ctx);
            }
        }));

        Ok(Self {
            _icon: tray_icon,
            pause_item: pause,
        })
    }

    fn set_paused(&self, paused: bool) {
        self.pause_item.set_text(pause_menu_label(paused));
    }
}

impl Drop for TrayState {
    fn drop(&mut self) {
        MenuEvent::set_event_handler::<fn(MenuEvent)>(None);
        TrayIconEvent::set_event_handler::<fn(TrayIconEvent)>(None);
    }
}

fn make_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let center = (size as f32 - 1.0) / 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let alpha = if dist < 9.5 {
                255
            } else if dist < 13.0 {
                100
            } else {
                0
            };
            rgba.extend_from_slice(&[76, 158, 255, alpha]);
        }
    }
    Icon::from_rgba(rgba, size, size).map_err(|err| format!("invalid tray icon: {err}").into())
}

struct LumiApp {
    config: AppConfig,
    runtime_path: PathBuf,
    runtime: RuntimeConfig,
    gateway: Dxva2MonitorGateway,
    worker: ControlWorkerHandle,
    tray: TrayState,
    events: Receiver<AppEvent>,
    monitors: Vec<MonitorInfo>,
    monitor_brightness: BTreeMap<String, i32>,
    selected_monitor: usize,
    last_control_source: ControlSource,
    last_sensor_lux: Option<f64>,
    last_target: Option<i32>,
    last_next_brightness: Option<i32>,
    last_current_before_step: Option<i32>,
    last_relay_state: Option<RelayState>,
    last_relay_gpio: Option<i32>,
    last_light_rule_match: Option<String>,
    last_light_rule_action: LightRuleAction,
    last_run: Option<String>,
    last_run_instant: Option<Instant>,
    last_error: Option<String>,
    last_worker_error: Option<String>,
    main_window_visible: bool,
    show_calibration: bool,
    show_settings: bool,
    show_light_rules: bool,
    settings_day_peak: i32,
    settings_night_target: i32,
    settings_theme: String,
    sensor_curve_points: Vec<SensorCurvePoint>,
    sensor_curve_history: Vec<Vec<SensorCurvePoint>>,
}

impl LumiApp {
    fn new(
        config: AppConfig,
        runtime_path: PathBuf,
        runtime: RuntimeConfig,
        gateway: Dxva2MonitorGateway,
        weather_cache: WeatherCache,
        ctx: egui::Context,
        repaint_proxy: EventLoopProxy<eframe::UserEvent>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (event_tx, event_rx) = channel();
        let tray = TrayState::new(runtime.paused, event_tx, repaint_proxy.clone(), ctx.clone())?;
        let settings_day_peak = runtime.daytime_peak_brightness;
        let settings_night_target = runtime.night_target_brightness;
        let settings_theme = runtime.theme_mode.clone();
        let sensor_curve_points = runtime.sensor_calibration_curve.clone();
        let worker = ControlWorkerHandle::spawn(
            config.clone(),
            runtime.clone(),
            gateway.clone(),
            weather_cache,
            repaint_proxy,
            ctx,
        );
        worker.send(ControlWorkerCommand::RunNow)?;
        let app = Self {
            config,
            runtime_path,
            runtime,
            gateway,
            worker,
            tray,
            events: event_rx,
            monitors: Vec::new(),
            monitor_brightness: BTreeMap::new(),
            selected_monitor: 0,
            last_control_source: ControlSource::Sensor,
            last_sensor_lux: None,
            last_target: None,
            last_next_brightness: None,
            last_current_before_step: None,
            last_relay_state: None,
            last_relay_gpio: None,
            last_light_rule_match: None,
            last_light_rule_action: LightRuleAction::Keep,
            last_run: None,
            last_run_instant: None,
            last_error: None,
            last_worker_error: None,
            main_window_visible: initial_viewport_visible_on_launch(),
            show_calibration: false,
            show_settings: false,
            show_light_rules: false,
            settings_day_peak,
            settings_night_target,
            settings_theme,
            sensor_curve_points,
            sensor_curve_history: Vec::new(),
        };
        Ok(app)
    }

    fn selected_monitor(&self) -> Option<&MonitorInfo> {
        self.monitors
            .get(self.selected_monitor)
            .or_else(|| self.monitors.first())
    }

    fn selected_brightness(&self) -> Option<i32> {
        self.selected_monitor()
            .and_then(|monitor| self.monitor_brightness.get(&monitor.identifier).copied())
    }

    fn light_strip_on(&self) -> Option<bool> {
        self.last_relay_state
            .map(|relay| relay.light_on(self.runtime.relay_contact_mode))
    }

    fn calibration_count(&self, monitor_id: &str) -> usize {
        self.runtime
            .monitor_calibrations
            .get(monitor_id)
            .map(|points| {
                ["manual_0", "manual_50", "manual_100"]
                    .iter()
                    .filter(|key| points.contains_key(**key))
                    .count()
            })
            .unwrap_or(0)
    }

    fn refresh_monitors(&mut self) {
        if let Err(err) = self.worker.send(ControlWorkerCommand::RunNow) {
            self.last_error = Some(err.to_string());
        }
    }

    fn request_worker_run_now(&mut self) {
        if let Err(err) = self.worker.send(ControlWorkerCommand::RunNow) {
            self.last_error = Some(err.to_string());
        }
    }

    fn sync_worker_runtime(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.worker
            .send(ControlWorkerCommand::UpdateRuntime(self.runtime.clone()))
    }

    fn apply_worker_snapshot(&mut self, snapshot: ControlSnapshot) {
        if let Some(monitor_snapshot) = snapshot.monitor_snapshot {
            apply_monitor_snapshot(
                &mut self.monitors,
                &mut self.monitor_brightness,
                &mut self.selected_monitor,
                monitor_snapshot,
            );
        }
        self.last_control_source = snapshot.last_control_source;
        if snapshot.last_sensor_lux.is_some() {
            self.last_sensor_lux = snapshot.last_sensor_lux;
        }
        if snapshot.last_target.is_some() {
            self.last_target = snapshot.last_target;
        }
        if snapshot.last_next_brightness.is_some() {
            self.last_next_brightness = snapshot.last_next_brightness;
        }
        if snapshot.last_current_before_step.is_some() {
            self.last_current_before_step = snapshot.last_current_before_step;
        }
        if snapshot.last_relay_state.is_some() {
            self.last_relay_state = snapshot.last_relay_state;
        }
        if snapshot.last_relay_gpio.is_some() {
            self.last_relay_gpio = snapshot.last_relay_gpio;
        }
        if let Some(light_rule_snapshot) = snapshot.light_rule_snapshot {
            self.last_light_rule_match = light_rule_snapshot.matched_rule;
            self.last_light_rule_action = light_rule_snapshot.action;
        }
        if snapshot.last_run.is_some() {
            self.last_run = snapshot.last_run;
        }
        if snapshot.last_run_instant.is_some() {
            self.last_run_instant = snapshot.last_run_instant;
        }
    }

    fn process_worker_updates(&mut self) {
        while let Ok(update) = self.worker.updates.try_recv() {
            if let Some(snapshot) = update.snapshot {
                self.apply_worker_snapshot(snapshot);
            }
            self.last_worker_error = update.error;
        }
    }

    fn visible_error(&self) -> Option<&str> {
        visible_error(&self.last_error, &self.last_worker_error)
    }

    fn action<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Self) -> Result<(), Box<dyn std::error::Error>>,
    {
        match f(self) {
            Ok(()) => self.last_error = None,
            Err(err) => self.last_error = Some(err.to_string()),
        }
    }

    fn save_runtime_config_and_sync(&mut self) -> bool {
        match save_runtime_config(&self.runtime_path, &self.runtime) {
            Ok(()) => match self.sync_worker_runtime() {
                Ok(()) => {
                    self.last_error = None;
                    true
                }
                Err(err) => {
                    self.last_error = Some(err.to_string());
                    false
                }
            },
            Err(err) => {
                self.last_error = Some(err.to_string());
                false
            }
        }
    }

    fn save_runtime(&mut self) -> bool {
        self.save_runtime_config_and_sync()
    }

    fn toggle_pause(&mut self) {
        self.runtime.paused = !self.runtime.paused;
        self.tray.set_paused(self.runtime.paused);
        self.save_runtime_config_and_sync();
    }

    fn set_relay_contact_mode(&mut self, contact_mode: RelayContactMode) {
        if self.runtime.relay_contact_mode == contact_mode {
            return;
        }
        self.runtime.relay_contact_mode = contact_mode;
        self.save_runtime_config_and_sync();
    }

    fn set_light_strip(&mut self, light_on: bool) -> Result<(), Box<dyn std::error::Error>> {
        self.worker
            .send(ControlWorkerCommand::SetLightStrip(light_on))
    }

    fn save_settings(&mut self) -> bool {
        self.runtime.daytime_peak_brightness =
            normalize_settings_brightness(self.settings_day_peak);
        self.runtime.night_target_brightness =
            normalize_settings_brightness(self.settings_night_target);
        self.runtime.theme_mode = self.settings_theme.clone();
        self.save_runtime_config_and_sync()
    }

    fn open_calibration(&mut self) {
        self.sensor_curve_points = self.runtime.sensor_calibration_curve.clone();
        self.show_calibration = true;
    }

    fn save_sensor_curve(&mut self) -> bool {
        self.commit_sensor_curve_change()
    }

    fn commit_sensor_curve_change(&mut self) -> bool {
        let previous = normalize_sensor_curve(&self.runtime.sensor_calibration_curve);
        let next = normalize_sensor_curve(&self.sensor_curve_points);
        if next == previous {
            self.sensor_curve_points = next;
            return true;
        }

        remember_sensor_curve_history(&mut self.sensor_curve_history, &previous);
        self.runtime.sensor_calibration_curve = next;
        if self.save_runtime_config_and_sync() {
            self.sensor_curve_points = self.runtime.sensor_calibration_curve.clone();
            true
        } else {
            false
        }
    }

    fn revert_sensor_curve(&mut self) {
        if let Some(previous) = pop_sensor_curve_history(&mut self.sensor_curve_history) {
            self.runtime.sensor_calibration_curve = previous;
            if self.save_runtime_config_and_sync() {
                self.sensor_curve_points = self.runtime.sensor_calibration_curve.clone();
            }
        }
    }

    fn capture_sensor_curve_point(&mut self) {
        match read_sensor_samples(&self.config.sensor_port, 1) {
            Ok(readings) => {
                let lux = readings[0].lux;
                let brightness = self.selected_brightness().unwrap_or_else(|| {
                    sensor_curve_editor_default_point(&self.sensor_curve_points).brightness
                });
                self.sensor_curve_points
                    .push(SensorCurvePoint { lux, brightness });
                self.commit_sensor_curve_change();
            }
            Err(err) => self.last_error = Some(err.to_string()),
        }
    }

    fn capture_point(&mut self, monitor_id: &str, point_key: &str, label: &str) {
        match capture_calibration_point(&self.gateway, monitor_id, label, 3) {
            Ok(point) => {
                self.runtime
                    .monitor_calibrations
                    .entry(monitor_id.to_string())
                    .or_default()
                    .insert(point_key.to_string(), point);
                self.save_runtime_config_and_sync();
                self.refresh_monitors();
            }
            Err(err) => self.last_error = Some(err.to_string()),
        }
    }

    fn show_main_window(ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn hide_main_window(ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn show_main(&mut self, ctx: &egui::Context) {
        self.main_window_visible = true;
        Self::show_main_window(ctx);
    }

    fn hide_main(&mut self, ctx: &egui::Context) {
        self.main_window_visible = false;
        Self::hide_main_window(ctx);
    }

    fn minimize_close_request_to_tray(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_main(ctx);
        }
    }

    fn process_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                AppEvent::Show => self.show_main(ctx),
                AppEvent::RunNow => {
                    self.show_main(ctx);
                    self.request_worker_run_now();
                }
                AppEvent::TogglePause => {
                    self.toggle_pause();
                    self.show_main(ctx);
                }
                AppEvent::Calibration => {
                    self.show_main(ctx);
                    self.open_calibration();
                }
                AppEvent::Settings => {
                    self.show_main(ctx);
                }
                AppEvent::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
    }

    fn apply_visuals(&self, ctx: &egui::Context) {
        if use_light_visuals(&self.runtime.theme_mode, ctx) {
            ctx.set_visuals(egui::Visuals::light());
        } else {
            ctx.set_visuals(dark_visuals());
        }
    }

    fn compact_dashboard_enabled(&self) -> bool {
        true
    }

    fn ui_sensor_card(&self, ui: &mut egui::Ui, lux: &str) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_height(56.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("SENSOR").small());
                ui.label(sensor_value_text(lux));
                ui.label(egui::RichText::new(format!("Port {}", self.config.sensor_port)).small());
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "Last {}",
                        compact_run_time(self.last_run.as_deref())
                    ))
                    .small(),
                );
                if self.visible_error().is_some() {
                    ui.label(
                        egui::RichText::new("Needs attention")
                            .strong()
                            .color(egui::Color32::from_rgb(255, 100, 116)),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("OK")
                            .strong()
                            .color(egui::Color32::from_rgb(59, 210, 142)),
                    );
                }
            });
        });
    }

    fn ui_monitors_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_height(94.0);
            ui.horizontal(|ui| {
                ui.strong("Monitors");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Refresh").clicked() {
                        self.refresh_monitors();
                    }
                });
            });
            ui.separator();

            if self.monitors.is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 100, 116),
                    "No DDC/CI monitor detected",
                );
            }

            for index in 0..self.monitors.len() {
                let monitor = &self.monitors[index];
                let current = self
                    .monitor_brightness
                    .get(&monitor.identifier)
                    .map(|value| format!("{value}%"))
                    .unwrap_or_else(|| "--".into());
                let calibration = self.calibration_count(&monitor.identifier);
                let selected = self.selected_monitor == index;
                let row = format!(
                    "{}   {}        {}        calibration {}/3",
                    index + 1,
                    monitor.description,
                    current,
                    calibration
                );
                if ui.selectable_label(selected, row).clicked() {
                    self.selected_monitor = index;
                }
            }
        });
    }

    fn ui_light_strip_controls(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Light strip");
            ui.label(light_strip_status_text(self.light_strip_on()));
            if let Some(gpio) = self.last_relay_gpio {
                ui.label(egui::RichText::new(format!("GPIO{gpio}")).small());
            }
        });
        ui.horizontal(|ui| {
            if ui.small_button("On").clicked() {
                self.action(|app| app.set_light_strip(true));
            }
            if ui.small_button("Off").clicked() {
                self.action(|app| app.set_light_strip(false));
            }
            if ui.small_button("Toggle").clicked() {
                let next = !self.light_strip_on().unwrap_or(false);
                self.action(|app| app.set_light_strip(next));
            }
            let mut contact_mode = self.runtime.relay_contact_mode;
            egui::ComboBox::from_id_salt("relay-contact-mode")
                .selected_text(contact_mode.label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut contact_mode, RelayContactMode::No, "NO");
                    ui.selectable_value(&mut contact_mode, RelayContactMode::Nc, "NC");
                });
            self.set_relay_contact_mode(contact_mode);
            if ui.small_button("Rules").clicked() {
                self.show_light_rules = true;
            }
        });
        let rule_status = if !self.runtime.light_rules_enabled {
            "Rules disabled".to_string()
        } else if let Some(rule) = &self.last_light_rule_match {
            format!("Matched: {rule} -> {}", self.last_light_rule_action.label())
        } else {
            format!("Fallback -> {}", self.last_light_rule_action.label())
        };
        ui.label(egui::RichText::new(rule_status).small());
    }

    fn ui_curve_actions_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("Curve & Tuning");
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Run now").clicked() {
                    self.request_worker_run_now();
                }
                if ui
                    .button(if self.runtime.paused {
                        "Resume"
                    } else {
                        "Pause"
                    })
                    .clicked()
                {
                    self.toggle_pause();
                }
                if ui.button("Calibration").clicked() {
                    self.open_calibration();
                }
            });
            ui.add_space(4.0);
            draw_sensor_curve_mini(ui, &self.runtime.sensor_calibration_curve);
            ui.label(format!(
                "{} points - tick every {}s",
                normalize_sensor_curve(&self.runtime.sensor_calibration_curve).len(),
                self.config.control_tick_seconds
            ));
            ui.separator();
            let mut settings_changed = false;
            ui.horizontal(|ui| {
                ui.label("Peak");
                settings_changed |= ui
                    .add(
                        egui::DragValue::new(&mut self.settings_day_peak)
                            .range(settings_brightness_range()),
                    )
                    .changed();
                ui.label("Night");
                settings_changed |= ui
                    .add(
                        egui::DragValue::new(&mut self.settings_night_target)
                            .range(settings_brightness_range()),
                    )
                    .changed();
                egui::ComboBox::from_id_salt("dashboard-theme")
                    .selected_text(theme_label(&self.settings_theme))
                    .show_ui(ui, |ui| {
                        settings_changed |= ui
                            .selectable_value(
                                &mut self.settings_theme,
                                "system".to_string(),
                                "System",
                            )
                            .changed();
                        settings_changed |= ui
                            .selectable_value(&mut self.settings_theme, "dark".to_string(), "Dark")
                            .changed();
                        settings_changed |= ui
                            .selectable_value(
                                &mut self.settings_theme,
                                "light".to_string(),
                                "Light",
                            )
                            .changed();
                    });
            });
            if settings_changed {
                self.save_settings();
            }
        });
    }

    fn ui_main_dashboard(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.heading("LumiControl");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Esc").clicked() {
                    self.hide_main(ctx);
                }
                let label = if self.runtime.paused {
                    "Paused"
                } else {
                    "Live"
                };
                let color = if self.runtime.paused {
                    egui::Color32::from_rgb(255, 100, 116)
                } else {
                    egui::Color32::from_rgb(59, 210, 142)
                };
                ui.colored_label(color, label);
            });
        });

        let current = self.selected_brightness().unwrap_or(0).clamp(0, 100);
        let target = self.last_target.unwrap_or(current).clamp(0, 100);
        let next = self.last_next_brightness.unwrap_or(current).clamp(0, 100);
        let lux = self
            .last_sensor_lux
            .map(|value| format!("{value:.1} lux"))
            .unwrap_or_else(|| "Waiting for lux".to_string());
        let updated = elapsed_update_label(self.last_run_instant);

        ui.add_space(8.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_height(150.0);
            ui.label(egui::RichText::new("NOW").small());
            ui.horizontal(|ui| {
                ui.label(primary_status_text(format!("{current}%")));
                ui.label(egui::RichText::new("current").small());
                ui.separator();
                ui.label(secondary_status_text(format!("{target}%")));
                ui.label(egui::RichText::new("target").small());
            });
            ui.add_space(6.0);
            ui.add(egui::ProgressBar::new(current as f32 / 100.0).show_percentage());
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!("Transition {current}% -> {next}%"))
                        .strong()
                        .color(egui::Color32::from_rgb(214, 224, 238)),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(self.last_control_source.label())
                        .strong()
                        .color(egui::Color32::from_rgb(224, 236, 252)),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(&lux)
                        .strong()
                        .color(egui::Color32::from_rgb(224, 236, 252)),
                );
                ui.separator();
                ui.label(format!("updated {updated}"));
            });
            self.ui_light_strip_controls(ui);
        });
        ui.add_space(8.0);
        self.ui_sensor_card(ui, &lux);
        ui.add_space(8.0);
        self.ui_curve_actions_card(ui);
        ui.add_space(8.0);
        self.ui_monitors_card(ui);

        if let Some(error) = self.visible_error() {
            ui.add_space(8.0);
            ui.colored_label(egui::Color32::from_rgb(255, 100, 116), error);
        }
    }

    fn ui_main(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.compact_dashboard_enabled() {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.ui_main_dashboard(ctx, ui);
                    });
            } else {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.heading("☀ LumiControl");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Esc").clicked() {
                            self.hide_main(ctx);
                        }
                        let label = if self.runtime.paused {
                            "Paused"
                        } else {
                            "Live"
                        };
                        let color = if self.runtime.paused {
                            egui::Color32::from_rgb(255, 100, 116)
                        } else {
                            egui::Color32::from_rgb(59, 210, 142)
                        };
                        ui.colored_label(color, format!("● {label}"));
                    });
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    metric(ui, "SOURCE", self.last_control_source.label().to_string());
                    metric(
                        ui,
                        "LUX",
                        self.last_sensor_lux
                            .map(|v| format!("{v:.1}"))
                            .unwrap_or_else(|| "--".into()),
                    );
                    metric(
                        ui,
                        "TARGET",
                        self.last_target
                            .map(|v| format!("{v}%"))
                            .unwrap_or_else(|| "--%".into()),
                    );
                    metric(
                        ui,
                        "CURRENT",
                        self.selected_brightness()
                            .map(|v| format!("{v}%"))
                            .unwrap_or_else(|| "--%".into()),
                    );
                    metric(
                        ui,
                        "LAST RUN",
                        self.last_run.clone().unwrap_or_else(|| "Waiting".into()),
                    );
                });

                ui.add_space(8.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong("Monitors");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Refresh").clicked() {
                                self.refresh_monitors();
                            }
                        });
                    });
                    ui.separator();

                    if self.monitors.is_empty() {
                        ui.colored_label(
                            egui::Color32::from_rgb(255, 100, 116),
                            "No DDC/CI monitor detected",
                        );
                    }

                    for index in 0..self.monitors.len() {
                        let monitor = &self.monitors[index];
                        let current = self
                            .monitor_brightness
                            .get(&monitor.identifier)
                            .map(|v| format!("{v}%"))
                            .unwrap_or_else(|| "--".into());
                        let calibration = self.calibration_count(&monitor.identifier);
                        let selected = self.selected_monitor == index;
                        let row = format!(
                            "{}   {}     current {}     calibration {}/3",
                            index + 1,
                            monitor.description,
                            current,
                            calibration
                        );
                        if ui.selectable_label(selected, row).clicked() {
                            self.selected_monitor = index;
                        }
                    }
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("▶ Run now").clicked() {
                        self.request_worker_run_now();
                    }
                    if ui
                        .button(if self.runtime.paused {
                            "▶ Resume"
                        } else {
                            "Ⅱ Pause"
                        })
                        .clicked()
                    {
                        self.toggle_pause();
                    }
                    if ui.button("Calibration").clicked() {
                        self.open_calibration();
                    }
                    if ui.button("Settings").clicked() {
                        self.settings_day_peak = self.runtime.daytime_peak_brightness;
                        self.settings_night_target = self.runtime.night_target_brightness;
                        self.settings_theme = self.runtime.theme_mode.clone();
                        self.show_settings = true;
                    }
                });

                ui.add_space(8.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    let name = self
                        .selected_monitor()
                        .map(|monitor| monitor.description.clone())
                        .unwrap_or_else(|| "No selected monitor".into());
                    let current = self.selected_brightness().unwrap_or(0).clamp(0, 100);
                    ui.horizontal(|ui| {
                        ui.strong(name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.heading(format!("{current}%"));
                        });
                    });
                    ui.add(egui::ProgressBar::new(current as f32 / 100.0).show_percentage());
                    let cal = self
                        .selected_monitor()
                        .map(|monitor| self.calibration_count(&monitor.identifier))
                        .unwrap_or(0);
                    ui.label(format!(
                        "Every {}s · Day peak {}% · Night {}% · Calibrated {}/3",
                        self.config.control_tick_seconds,
                        self.runtime.daytime_peak_brightness,
                        self.runtime.night_target_brightness,
                        cal,
                    ));
                });

                ui.add_space(8.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    let name = self
                        .selected_monitor()
                        .map(|monitor| monitor.description.clone())
                        .unwrap_or_else(|| "No selected monitor".into());
                    let current = self.selected_brightness().unwrap_or(0).clamp(0, 100);
                    let cal = self
                        .selected_monitor()
                        .map(|monitor| self.calibration_count(&monitor.identifier))
                        .unwrap_or(0);
                    ui.columns(2, |columns| {
                        columns[0].vertical(|ui| {
                            ui.strong("Control Status");
                            ui.separator();
                            ui.label(format!(
                                "Source: {} ({})",
                                self.last_control_source.label(),
                                self.last_control_source.detail()
                            ));
                            ui.label(format!("Sensor port: {}", self.config.sensor_port));
                            ui.label(format!(
                                "Latest lux: {}",
                                self.last_sensor_lux
                                    .map(|value| format!("{value:.2}"))
                                    .unwrap_or_else(|| "Waiting".to_string())
                            ));
                            ui.label(format!(
                                "Target: {}",
                                self.last_target
                                    .map(|value| format!("{value}%"))
                                    .unwrap_or_else(|| "Waiting".to_string())
                            ));
                            ui.label(format!(
                                "Transition target: {}",
                                self.last_next_brightness
                                    .map(|value| format!("{value}%"))
                                    .unwrap_or_else(|| "Waiting".to_string())
                            ));
                            ui.label(format!(
                                "Last run: {}",
                                self.last_run.clone().unwrap_or_else(|| "Waiting".into())
                            ));
                        });
                        columns[1].vertical(|ui| {
                            ui.strong(name);
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label("Current");
                                ui.heading(format!("{current}%"));
                            });
                            ui.add(
                                egui::ProgressBar::new(current as f32 / 100.0).show_percentage(),
                            );
                            ui.label(format!(
                                "Before last step: {}",
                                self.last_current_before_step
                                    .map(|value| format!("{value}%"))
                                    .unwrap_or_else(|| "Waiting".to_string())
                            ));
                            ui.label(format!("Calibrated {cal}/3"));
                            ui.label(format!("Tick every {}s", self.config.control_tick_seconds));
                            ui.label(format!(
                                "Curve: {}",
                                sensor_curve_summary(&self.runtime.sensor_calibration_curve)
                            ));
                        });
                    });
                });

                if let Some(error) = self.visible_error() {
                    ui.add_space(8.0);
                    ui.colored_label(egui::Color32::from_rgb(255, 100, 116), error);
                }
            }
        });
    }

    fn ui_calibration(&mut self, ctx: &egui::Context) {
        let mut open = self.show_calibration;
        egui::Window::new("Calibration")
            .open(&mut open)
            .default_size([820.0, 660.0])
            .show(ctx, |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Sensor Curve");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(format!("Port {}", self.config.sensor_port));
                        });
                    });
                    ui.label("Map measured lux to the brightness that feels right.");
                    ui.add_space(6.0);
                    ui.columns(2, |columns| {
                        columns[0].vertical(|ui| {
                            let mut delete_index = None;
                            let mut curve_changed = false;
                            for index in 0..self.sensor_curve_points.len() {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{}", index + 1));
                                    ui.label("lux");
                                    curve_changed |= ui
                                        .add(
                                            egui::DragValue::new(
                                                &mut self.sensor_curve_points[index].lux,
                                            )
                                            .speed(1.0)
                                            .range(0.1..=100_000.0),
                                        )
                                        .changed();
                                    ui.label("brightness");
                                    curve_changed |= ui
                                        .add(
                                            egui::DragValue::new(
                                                &mut self.sensor_curve_points[index].brightness,
                                            )
                                            .speed(1)
                                            .range(settings_brightness_range()),
                                        )
                                        .changed();
                                    if ui.small_button("Remove").clicked() {
                                        delete_index = Some(index);
                                    }
                                });
                            }
                            if let Some(index) = delete_index {
                                self.sensor_curve_points.remove(index);
                                curve_changed = true;
                            }
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if ui.button("Add point").clicked() {
                                    let mut point = sensor_curve_editor_default_point(
                                        &self.sensor_curve_points,
                                    );
                                    point.lux = (point.lux * 1.5).max(1.0);
                                    self.sensor_curve_points.push(point);
                                    curve_changed = true;
                                }
                                if ui.button("Capture current").clicked() {
                                    self.capture_sensor_curve_point();
                                }
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Reset defaults").clicked() {
                                    self.sensor_curve_points = default_sensor_calibration_curve();
                                    curve_changed = true;
                                }
                                if ui
                                    .add_enabled(
                                        !self.sensor_curve_history.is_empty(),
                                        egui::Button::new("Revert"),
                                    )
                                    .clicked()
                                {
                                    self.revert_sensor_curve();
                                }
                            });
                            ui.label(format!(
                                "Auto-saved changes. Revert history: {}/3",
                                self.sensor_curve_history.len()
                            ));
                            if curve_changed {
                                self.save_sensor_curve();
                            }
                        });
                        columns[1].vertical(|ui| {
                            let preview =
                                draw_sensor_curve_preview(ui, &mut self.sensor_curve_points);
                            if preview.commit {
                                self.save_sensor_curve();
                            }
                            ui.add_space(6.0);
                            let curve = normalize_sensor_curve(&self.sensor_curve_points);
                            let current_lux = curve
                                .iter()
                                .map(|point| {
                                    format!("{:.0} lx -> {}%", point.lux, point.brightness)
                                })
                                .collect::<Vec<_>>()
                                .join("   ");
                            ui.label(current_lux);
                        });
                    });
                });
                ui.add_space(10.0);
                ui.label("Set each monitor manually, then capture the current DDC/CI value.");
                ui.separator();
                for index in 0..self.monitors.len() {
                    let monitor = self.monitors[index].clone();
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.heading(format!("{}", index + 1));
                            ui.vertical(|ui| {
                                ui.strong(&monitor.description);
                                ui.label(&monitor.identifier);
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(format!(
                                        "{}/3 captured",
                                        self.calibration_count(&monitor.identifier)
                                    ));
                                },
                            );
                        });
                        ui.add_space(6.0);
                        ui.columns(3, |columns| {
                            for (column, (key, label)) in [
                                ("manual_0", "0% Black"),
                                ("manual_50", "50% Gray"),
                                ("manual_100", "100% White"),
                            ]
                            .iter()
                            .enumerate()
                            {
                                columns[column].vertical_centered(|ui| {
                                    ui.strong(*label);
                                    let captured = self
                                        .runtime
                                        .monitor_calibrations
                                        .get(&monitor.identifier)
                                        .and_then(|points| points.get(*key));
                                    if let Some(point) = captured {
                                        ui.label(format!(
                                            "avg {} · min {} · max {}",
                                            point.average_raw, point.min_raw, point.max_raw
                                        ));
                                    } else {
                                        ui.label("Not captured");
                                    }
                                    if ui.button("Capture").clicked() {
                                        self.capture_point(&monitor.identifier, key, label);
                                    }
                                });
                            }
                        });
                    });
                    ui.add_space(8.0);
                }
            });
        self.show_calibration = open;
    }

    fn ui_light_rules(&mut self, ctx: &egui::Context) {
        let mut open = self.show_light_rules;
        let mut changed = false;
        let mut move_up = None;
        let mut move_down = None;
        let mut remove_rule = None;
        egui::Window::new("Light Rules")
            .open(&mut open)
            .default_size([760.0, 620.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    changed |= ui
                        .checkbox(&mut self.runtime.light_rules_enabled, "Enable rules mode")
                        .changed();
                    ui.label("Fallback");
                    changed |= ui_light_rule_action(
                        ui,
                        "light-rule-fallback-action",
                        &mut self.runtime.light_rules_fallback_action,
                    );
                    if ui.button("Add rule").clicked() {
                        let mut rule = LightRule {
                            name: format!("Rule {}", self.runtime.light_rules.len() + 1),
                            ..LightRule::default()
                        };
                        rule.all.push(default_light_rule_condition(5));
                        self.runtime.light_rules.push(rule);
                        changed = true;
                    }
                });
                ui.separator();

                if self.runtime.light_rules.is_empty() {
                    ui.label("No rules yet.");
                }

                let rule_count = self.runtime.light_rules.len();
                for index in 0..rule_count {
                    let rule_id = format!("light-rule-{index}");
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        let rule = &mut self.runtime.light_rules[index];
                        ui.horizontal(|ui| {
                            changed |= ui.checkbox(&mut rule.enabled, "").changed();
                            changed |= ui.text_edit_singleline(&mut rule.name).changed();
                            ui.label("THEN");
                            changed |= ui_light_rule_action(
                                ui,
                                ("light-rule-then-action", index),
                                &mut rule.then_action,
                            );
                            if ui.small_button("Up").clicked() && index > 0 {
                                move_up = Some(index);
                            }
                            if ui.small_button("Down").clicked() && index + 1 < rule_count {
                                move_down = Some(index);
                            }
                            if ui.small_button("Delete").clicked() {
                                remove_rule = Some(index);
                            }
                        });
                        ui.add_space(4.0);
                        ui.columns(2, |columns| {
                            columns[0].vertical(|ui| {
                                changed |= ui_light_rule_condition_group(
                                    ui,
                                    &format!("{rule_id}-all"),
                                    "AND",
                                    &mut rule.all,
                                );
                            });
                            columns[1].vertical(|ui| {
                                changed |= ui_light_rule_condition_group(
                                    ui,
                                    &format!("{rule_id}-any"),
                                    "OR",
                                    &mut rule.any,
                                );
                            });
                        });
                    });
                    ui.add_space(8.0);
                }
            });
        self.show_light_rules = open;

        if let Some(index) = move_up {
            self.runtime.light_rules.swap(index, index - 1);
            changed = true;
        }
        if let Some(index) = move_down {
            self.runtime.light_rules.swap(index, index + 1);
            changed = true;
        }
        if let Some(index) = remove_rule {
            self.runtime.light_rules.remove(index);
            changed = true;
        }
        if changed {
            self.save_runtime();
        }
    }

    fn ui_settings(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut close_requested = false;
        egui::Window::new("Appearance & Tuning")
            .open(&mut open)
            .default_size([360.0, 240.0])
            .show(ctx, |ui| {
                ui.label("Tune brightness bounds and switch the theme.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Daytime peak brightness");
                    ui.add(
                        egui::DragValue::new(&mut self.settings_day_peak)
                            .range(settings_brightness_range()),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Night target brightness");
                    ui.add(
                        egui::DragValue::new(&mut self.settings_night_target)
                            .range(settings_brightness_range()),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Theme");
                    egui::ComboBox::from_id_salt("theme")
                        .selected_text(theme_label(&self.settings_theme))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.settings_theme,
                                "system".to_string(),
                                "System",
                            );
                            ui.selectable_value(
                                &mut self.settings_theme,
                                "dark".to_string(),
                                "Dark",
                            );
                            ui.selectable_value(
                                &mut self.settings_theme,
                                "light".to_string(),
                                "Light",
                            );
                        });
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        close_requested = self.save_settings();
                    }
                    if ui.button("Cancel").clicked() {
                        close_requested = true;
                    }
                });
            });
        self.show_settings = window_open_after_action(open, close_requested);
    }
}

impl eframe::App for LumiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_visuals(ctx);
        self.minimize_close_request_to_tray(ctx);
        self.process_worker_updates();
        self.process_events(ctx);

        self.ui_main(ctx);
        if self.show_calibration {
            self.ui_calibration(ctx);
        }
        if self.show_settings {
            self.ui_settings(ctx);
        }
        if self.show_light_rules {
            self.ui_light_rules(ctx);
        }
    }
}

fn metric(ui: &mut egui::Ui, label: &str, value: String) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_width(130.0);
        ui.label(egui::RichText::new(label).small());
        ui.heading(value);
    });
}

#[cfg(test)]
mod tray_wiring_tests {
    use super::AppEvent;
    use crate::{MonitorInfo, SensorCurvePoint};
    use std::collections::BTreeMap;
    use tray_icon::{MouseButton, MouseButtonState};

    fn production_source() -> &'static str {
        include_str!("egui_gui.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("egui_gui.rs should contain production code before tests")
    }

    #[test]
    fn tray_events_use_global_handlers_instead_of_blocking_receivers() {
        let source = production_source();
        let production_source = source
            .split("#[cfg(test)]")
            .next()
            .expect("egui_gui.rs should contain production code before tests");
        assert!(
            production_source.contains("MenuEvent::set_event_handler"),
            "menu events should use set_event_handler so winit gets awakened"
        );
        assert!(
            production_source.contains("TrayIconEvent::set_event_handler"),
            "tray events should use set_event_handler so winit gets awakened"
        );
        assert!(
            !production_source.contains("MenuEvent::receiver().recv()"),
            "blocking menu receivers can freeze the Windows tray menu"
        );
        assert!(
            !production_source.contains("TrayIconEvent::receiver().recv()"),
            "blocking tray receivers can freeze the Windows tray menu"
        );
    }

    #[test]
    fn hidden_window_tray_actions_do_not_wait_for_egui_repaint() {
        assert!(AppEvent::Show.opens_window_before_egui_update());
        assert!(AppEvent::RunNow.opens_window_before_egui_update());
        assert!(AppEvent::TogglePause.opens_window_before_egui_update());
        assert!(AppEvent::Calibration.opens_window_before_egui_update());
        assert!(AppEvent::Settings.opens_window_before_egui_update());
        assert!(!AppEvent::Quit.opens_window_before_egui_update());
        assert!(AppEvent::Quit.exits_without_egui_update());
    }

    #[test]
    fn only_left_tray_click_opens_main_window() {
        assert!(super::tray_click_opens_main_window(
            MouseButton::Left,
            MouseButtonState::Up
        ));
        assert!(!super::tray_click_opens_main_window(
            MouseButton::Right,
            MouseButtonState::Up
        ));
        assert!(!super::tray_click_opens_main_window(
            MouseButton::Right,
            MouseButtonState::Down
        ));
    }

    #[test]
    fn initial_viewport_is_visible_on_launch() {
        assert!(super::initial_viewport_visible_on_launch());
    }

    #[test]
    fn startup_update_does_not_auto_hide_main_window() {
        let source = production_source();
        let update_source = source
            .rsplit("impl eframe::App for LumiApp")
            .next()
            .expect("eframe app implementation should exist")
            .split("fn metric")
            .next()
            .expect("eframe update implementation should appear before helpers");
        assert!(
            !update_source.contains("first_update"),
            "startup should leave the main GUI visible instead of hiding it on the first update"
        );
        assert!(
            !update_source.contains("hide_main_window(ctx)"),
            "startup update should not directly hide the main window"
        );
    }

    #[test]
    fn egui_update_does_not_run_control_tick() {
        let source = production_source();
        let update_source = source
            .rsplit("impl eframe::App for LumiApp")
            .next()
            .expect("eframe app implementation should exist")
            .split("fn metric")
            .next()
            .expect("eframe update implementation should appear before helpers");
        assert!(
            update_source.contains("process_worker_updates"),
            "GUI update should only consume background worker status"
        );
        assert!(
            !update_source.contains("run_once"),
            "GUI update must not perform blocking control work"
        );
        assert!(
            !update_source.contains("next_tick"),
            "control timing must live in the background worker"
        );
    }

    #[test]
    fn dashboard_does_not_schedule_fixed_repaints() {
        let source = production_source();
        let update_source = source
            .rsplit("impl eframe::App for LumiApp")
            .next()
            .expect("eframe app implementation should exist")
            .split("fn metric")
            .next()
            .expect("eframe update implementation should appear before helpers");

        assert!(
            !update_source.contains("request_repaint_after"),
            "dashboard state changes already wake egui; a fixed repaint timer can leave a hidden Windows viewport polling"
        );
    }

    #[test]
    fn empty_full_monitor_snapshot_clears_stale_monitor_state() {
        let mut monitors = vec![MonitorInfo {
            identifier: "monitor-1".to_string(),
            description: "Old monitor".to_string(),
        }];
        let mut brightness = BTreeMap::from([("monitor-1".to_string(), 72)]);
        let mut selected_monitor = 0;

        super::apply_monitor_snapshot(
            &mut monitors,
            &mut brightness,
            &mut selected_monitor,
            super::MonitorSnapshot {
                monitors: Vec::new(),
                monitor_brightness: BTreeMap::new(),
            },
        );

        assert!(monitors.is_empty());
        assert!(brightness.is_empty());
        assert_eq!(selected_monitor, 0);
    }

    #[test]
    fn relay_only_worker_update_does_not_pretend_to_include_monitor_snapshot() {
        let source = production_source();
        let worker_source = source
            .split("Ok(ControlWorkerCommand::SetLightStrip")
            .nth(1)
            .expect("light strip worker command should exist")
            .split("Ok(ControlWorkerCommand::Stop)")
            .next()
            .expect("light strip worker command should appear before stop handling");

        assert!(
            worker_source.contains("monitor_snapshot: None"),
            "relay-only updates should not use an empty monitor list as a sentinel"
        );
        assert!(
            worker_source.contains("light_rule_snapshot: None"),
            "relay-only updates should not clear the displayed matched rule"
        );
    }

    #[test]
    fn relay_rule_failures_do_not_abort_brightness_control_snapshot() {
        let source = production_source();
        let run_control_source = source
            .split("fn run_control_once")
            .nth(1)
            .expect("run_control_once should exist")
            .split("impl TrayState")
            .next()
            .expect("run_control_once should appear before tray implementation");

        assert!(
            run_control_source.contains("relay_error = Some(err.to_string())"),
            "relay command failures should be reported without aborting the control tick"
        );
        assert!(
            !run_control_source.contains("send_relay_command(&config.sensor_port, relay_target)?"),
            "relay command failures must not skip the brightness update path"
        );
    }

    #[test]
    fn control_worker_owns_timed_loop() {
        let source = production_source();
        let production_source = source
            .split("#[cfg(test)]")
            .next()
            .expect("egui_gui.rs should contain production code before tests");
        assert!(production_source.contains("thread::spawn"));
        assert!(production_source.contains("recv_timeout"));
        assert!(production_source.contains("ControlWorkerCommand::RunNow"));
    }

    #[test]
    fn initial_viewport_is_resizable() {
        assert!(super::initial_viewport_resizable());
    }

    #[test]
    fn initial_viewport_size_fits_compact_dashboard() {
        assert_eq!(super::initial_viewport_size(), [520.0, 300.0]);
    }

    #[test]
    fn compact_dashboard_uses_single_column_flow() {
        let source = production_source();
        let dashboard_source = source
            .rsplit("fn ui_main_dashboard")
            .next()
            .expect("compact dashboard implementation should exist")
            .split("fn ui_main")
            .next()
            .expect("compact dashboard implementation should appear before ui_main");
        assert!(
            !dashboard_source.contains("ui.columns(3"),
            "compact dashboard should use one vertical column instead of three cramped columns"
        );
        let now_index = dashboard_source.find("NOW").expect("NOW card should exist");
        let sensor_index = dashboard_source
            .find("ui_sensor_card")
            .expect("Sensor card should be rendered");
        let curve_index = dashboard_source
            .find("ui_curve_actions_card")
            .expect("Curve card should be rendered");
        let monitors_index = dashboard_source
            .find("ui_monitors_card")
            .expect("Monitors card should be rendered");
        assert!(now_index < sensor_index);
        assert!(sensor_index < curve_index);
        assert!(curve_index < monitors_index);
    }

    #[test]
    fn compact_dashboard_scrolls_below_first_status_cards() {
        let source = production_source();
        let ui_main_source = source
            .rsplit("fn ui_main")
            .next()
            .expect("ui_main implementation should exist")
            .split("fn ui_calibration")
            .next()
            .expect("ui_main implementation should appear before calibration UI");
        assert!(
            ui_main_source.contains("egui::ScrollArea::vertical()"),
            "compact dashboard should keep lower cards reachable when the default window only shows NOW and SENSOR"
        );
    }

    #[test]
    fn compact_dashboard_emphasizes_primary_status_values() {
        let source = production_source();
        let dashboard_source = source
            .rsplit("fn ui_main_dashboard")
            .next()
            .expect("compact dashboard implementation should exist")
            .split("fn ui_main")
            .next()
            .expect("compact dashboard implementation should appear before ui_main");
        assert!(
            dashboard_source.contains("primary_status_text"),
            "current and target values should use a high-emphasis text style"
        );
        let light_controls_source = source
            .rsplit("fn ui_light_strip_controls")
            .next()
            .expect("light strip controls implementation should exist")
            .split("fn ui_curve_actions_card")
            .next()
            .expect("light strip controls should appear before curve actions");
        assert!(
            light_controls_source.contains("light_strip_status_text"),
            "light strip state should use a distinct status style"
        );
        let sensor_source = source
            .rsplit("fn ui_sensor_card")
            .next()
            .expect("sensor card implementation should exist")
            .split("fn ui_monitors_card")
            .next()
            .expect("sensor card should appear before monitor card");
        assert!(
            sensor_source.contains("sensor_value_text"),
            "sensor lux value should use a distinct status style"
        );
    }

    #[test]
    fn sensor_curve_history_keeps_three_revert_steps() {
        let mut history = Vec::new();
        for brightness in [40, 50, 60, 70] {
            super::remember_sensor_curve_history(
                &mut history,
                &[SensorCurvePoint {
                    lux: brightness as f64,
                    brightness,
                }],
            );
        }

        assert_eq!(history.len(), 3);
        assert_eq!(history[0][0].brightness, 50);
        assert_eq!(
            super::pop_sensor_curve_history(&mut history).unwrap()[0].brightness,
            70
        );
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn sensor_curve_plot_drag_maps_position_to_curve_point() {
        let rect = egui::Rect::from_min_max(egui::pos2(10.0, 10.0), egui::pos2(210.0, 110.0));
        let point = super::sensor_curve_point_from_plot_position(
            rect,
            egui::pos2(110.0, 35.0),
            10.0,
            1000.0,
        );

        assert!((point.lux - 100.0).abs() < 0.1);
        assert_eq!(point.brightness, 75);
    }

    #[test]
    fn settings_brightness_controls_allow_full_monitor_range() {
        assert_eq!(super::settings_brightness_range(), 0..=100);
    }

    #[test]
    fn theme_labels_include_system_mode() {
        assert_eq!(super::theme_label("system"), "System");
        assert_eq!(super::theme_label("light"), "Light");
        assert_eq!(super::theme_label("dark"), "Dark");
        assert_eq!(super::theme_label("unknown"), "Dark");
    }

    #[test]
    fn saved_settings_brightness_uses_full_monitor_range() {
        assert_eq!(super::normalize_settings_brightness(0), 0);
        assert_eq!(super::normalize_settings_brightness(100), 100);
        assert_eq!(super::normalize_settings_brightness(-1), 0);
        assert_eq!(super::normalize_settings_brightness(101), 100);
    }

    #[test]
    fn settings_save_and_cancel_close_the_window_even_when_native_open_flag_stays_true() {
        assert!(!super::window_open_after_action(true, true));
        assert!(super::window_open_after_action(true, false));
        assert!(!super::window_open_after_action(false, false));
    }

    #[test]
    fn pause_menu_label_matches_current_state() {
        assert_eq!(super::pause_menu_label(false), "Pause");
        assert_eq!(super::pause_menu_label(true), "Resume");
    }

    #[test]
    fn visible_error_keeps_action_errors_separate_from_worker_errors() {
        let action_error = Some("save failed".to_string());
        let worker_error = Some("sensor failed".to_string());
        assert_eq!(
            super::visible_error(&action_error, &worker_error),
            Some("save failed")
        );
        assert_eq!(
            super::visible_error(&None, &worker_error),
            Some("sensor failed")
        );
        assert_eq!(super::visible_error(&None, &None), None);
    }

    #[test]
    fn window_close_button_minimizes_to_tray_instead_of_exiting() {
        let source = production_source();
        let production_source = source
            .rsplit("impl LumiApp {")
            .next()
            .expect("egui_gui.rs should contain LumiApp production code")
            .split("impl eframe::App")
            .next()
            .expect("LumiApp implementation should appear before eframe::App implementation");
        assert!(
            production_source.contains("close_requested()"),
            "root viewport close requests should be intercepted"
        );
        assert!(
            production_source.contains("ViewportCommand::CancelClose"),
            "the window close button should cancel process exit"
        );
        assert!(
            production_source.contains("hide_main_window(ctx)"),
            "the window close button should hide the window to the tray"
        );
    }
}
