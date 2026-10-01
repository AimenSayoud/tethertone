//! The window: pairing QR, phones and their direction, the phone's audio, and
//! settings. On Windows it also lives in the tray, and closing the window
//! keeps it running there.

use std::time::{Duration, SystemTime};

use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke, Vec2};
use tethertone_core::server::ActivityKind;
use tethertone_core::{ClientSnapshot, StreamMode, UpSource};

use crate::engine::{Engine, View};
use crate::{autostart, icon, qr, VERSION};

const ACCENT: Color32 = Color32::from_rgb(56, 189, 248);
const OK: Color32 = Color32::from_rgb(74, 222, 128);
const WARN: Color32 = Color32::from_rgb(251, 191, 36);
const BAD: Color32 = Color32::from_rgb(248, 113, 113);
const MUTED: Color32 = Color32::from_rgb(148, 163, 184);

pub fn run(mut engine: Engine) {
    if engine.settings.was_running {
        let _ = engine.start();
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Tethertone")
            .with_inner_size([860.0, 640.0])
            .with_min_inner_size([620.0, 480.0])
            .with_icon(egui::IconData { rgba: icon::rgba(64), width: 64, height: 64 }),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Tethertone",
        options,
        Box::new(move |cc| {
            style(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            engine.on_change(move || ctx.request_repaint());
            Ok(Box::new(App::new(engine, &cc.egui_ctx)))
        }),
    );
    if let Err(e) = result {
        eprintln!("[gui] could not open the window: {e}");
    }
}

fn style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.visuals.selection.bg_fill = ACCENT.linear_multiply(0.5);
        s.visuals.hyperlink_color = ACCENT;
    });
}

struct App {
    engine: Engine,
    qr: Option<(String, egui::TextureHandle)>,
    level: f32,
    up_level: f32,
    port_text: String,
    autostart: bool,
    confirm_rotate: bool,
    #[cfg(windows)]
    tray: Option<tray::Tray>,
}

impl App {
    fn new(engine: Engine, ctx: &egui::Context) -> App {
        let _ = ctx;
        App {
            port_text: engine.settings.port.to_string(),
            engine,
            qr: None,
            level: 0.0,
            up_level: 0.0,
            autostart: autostart::is_enabled(),
            confirm_rotate: false,
            #[cfg(windows)]
            tray: tray::Tray::new(ctx),
        }
    }

    fn qr_texture(&mut self, ctx: &egui::Context, uri: &str) -> Option<egui::TextureHandle> {
        if self.qr.as_ref().map(|(u, _)| u.as_str()) != Some(uri) {
            let (width, modules) = qr::matrix(uri)?;
            let quiet = 3;
            let size = width + quiet * 2;
            let mut image = egui::ColorImage::new([size, size], vec![Color32::WHITE; size * size]);
            for y in 0..width {
                for x in 0..width {
                    if modules[y * width + x] {
                        image[(x + quiet, y + quiet)] = Color32::BLACK;
                    }
                }
            }
            // Nearest-neighbour: hard module edges, or it will not scan.
            let texture = ctx.load_texture("pairing-qr", image, egui::TextureOptions::NEAREST);
            self.qr = Some((uri.to_string(), texture));
        }
        self.qr.as_ref().map(|(_, t)| t.clone())
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(windows)]
        self.handle_tray(ctx);

        let view = self.engine.view();
        let (peak, up_peak) = self.engine.take_levels();
        // Decay, so a peak stays readable instead of flickering out.
        self.level = peak.max(self.level * 0.85);
        self.up_level = up_peak.max(self.up_level * 0.85);
        if view.running {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        egui::TopBottomPanel::top("header").show(ctx, |ui| self.header(ui, &view));
        egui::SidePanel::left("pairing").resizable(false).exact_width(280.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.pairing(ui, ctx, &view));
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.phones(ui, &view);
                ui.add_space(12.0);
                self.phone_audio(ui, &view);
                ui.add_space(12.0);
                self.settings(ui);
                ui.add_space(12.0);
                activity(ui, &view);
            });
        });
    }
}

impl App {
    fn header(&mut self, ui: &mut egui::Ui, view: &View) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let texture = ui.ctx().load_texture("logo", logo_image(), egui::TextureOptions::LINEAR);
            ui.add(egui::Image::new(&texture).fit_to_exact_size(Vec2::splat(28.0)));
            ui.label(RichText::new("Tethertone").size(20.0).strong());
            let (colour, status) = status(view);
            ui.add_space(8.0);
            dot(ui, colour);
            ui.label(RichText::new(status).color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if view.running { "■  Stop" } else { "▶  Start" };
                let button = egui::Button::new(RichText::new(label).strong().size(15.0))
                    .fill(if view.running { Color32::from_rgb(51, 65, 85) } else { ACCENT.linear_multiply(0.6) })
                    .min_size(Vec2::new(110.0, 32.0));
                if ui.add(button).clicked() {
                    if view.running {
                        self.engine.stop();
                    } else {
                        let _ = self.engine.start();
                    }
                }
            });
        });
        if let Some(error) = &view.error {
            ui.label(RichText::new(error).color(BAD));
        }
        ui.add_space(6.0);
    }

    fn pairing(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, view: &View) {
        ui.add_space(10.0);
        ui.heading("Pair a phone");
        ui.label(RichText::new("Scan with the Tethertone app on your Android phone.").color(MUTED));
        ui.add_space(6.0);
        match (&view.pairing, view.running) {
            (Some(pairing), true) => {
                let uri = pairing.uri();
                if let Some(texture) = self.qr_texture(ctx, &uri) {
                    ui.add(egui::Image::new(&texture).fit_to_exact_size(Vec2::splat(240.0)));
                }
                ui.add_space(4.0);
                for host in &pairing.hosts {
                    let label = if host == "127.0.0.1" { format!("{host}  (USB)") } else { host.clone() };
                    ui.monospace(label);
                }
                ui.monospace(format!("port {}", pairing.port));
                if ui.button("Copy pairing link").clicked() {
                    ui.ctx().copy_text(uri);
                }
            }
            _ => {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(240.0), Sense::hover());
                ui.painter().rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(30, 41, 59));
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Press Start", egui::FontId::proportional(16.0), MUTED);
            }
        }
        ui.add_space(10.0);
        ui.label(RichText::new("Capturing").strong());
        level_meter(ui, self.level);
        if let Some(format) = view.format {
            ui.label(RichText::new(format!("{} kHz · {} ch", format.sample_rate / 1000, format.channels)).color(MUTED));
        }
        ui.label(RichText::new(format!("{:.1} MB sent", view.bytes_sent as f64 / 1_048_576.0)).color(MUTED));
        ui.label(RichText::new(match &view.usb_serial {
            Some(serial) => format!("USB route live ({serial})"),
            None => "USB: plug in a phone with USB debugging on".into(),
        }).color(MUTED).small());
    }

    fn phones(&mut self, ui: &mut egui::Ui, view: &View) {
        section(ui, "Phones");
        if view.clients.is_empty() {
            ui.label(RichText::new(if view.running {
                "Nothing connected yet. Scan the QR code with the phone."
            } else {
                "The server is stopped."
            }).color(MUTED));
            return;
        }
        for client in &view.clients {
            card(ui, |ui| self.phone(ui, client));
        }
    }

    fn phone(&mut self, ui: &mut egui::Ui, client: &ClientSnapshot) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&client.name).strong().size(15.0));
            ui.label(RichText::new(&client.peer).color(MUTED).small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Disconnect").clicked() {
                    self.engine.disconnect(client.id);
                }
            });
        });
        let mut stats = vec![format!("rtt {:.0} ms", client.rtt_ms)];
        if client.buffer_ms > 0 {
            stats.push(format!("buffer {} ms", client.buffer_ms));
        }
        if client.underruns > 0 {
            stats.push(format!("underruns {}", client.underruns));
        }
        if client.lost > 0 {
            stats.push(format!("lost {}", client.lost));
        }
        if client.dropped > 0 {
            stats.push(format!("dropped {}", client.dropped));
        }
        if (client.speed - 1.0).abs() > 0.00005 {
            stats.push(format!("{:+.0} ppm", (client.speed - 1.0) * 1_000_000.0));
        }
        ui.monospace(RichText::new(stats.join("  ·  ")).color(MUTED));

        if !client.duplex {
            ui.label(RichText::new("This phone has an older Tethertone: update it for two-way audio.").color(MUTED).small());
            return;
        }
        // The phone owns the mode; every click here is a request it may decline.
        let mode = client.mode;
        ui.horizontal(|ui| {
            ui.label("Direction");
            let source = if mode.up == UpSource::Off { UpSource::Mic } else { mode.up };
            let choices = [
                ("To phone", StreamMode { down: true, up: UpSource::Off }),
                ("To computer", StreamMode { down: false, up: source }),
                ("Both", StreamMode { down: true, up: source }),
            ];
            for (label, choice) in choices {
                let selected = (mode.up == UpSource::Off) == (choice.up == UpSource::Off) && mode.down == choice.down;
                if ui.selectable_label(selected, label).clicked() && !selected {
                    self.engine.request_mode(client.id, choice);
                }
            }
        });
        if mode.up != UpSource::Off {
            ui.horizontal(|ui| {
                ui.label("From the phone");
                for (label, source) in [("Microphone", UpSource::Mic), ("Its audio", UpSource::Playback)] {
                    if ui.selectable_label(mode.up == source, label).clicked() && mode.up != source {
                        self.engine.request_mode(client.id, StreamMode { down: mode.down, up: source });
                    }
                }
            });
            if mode.up == UpSource::Mic && mode.down {
                ui.label(RichText::new("Headphones on the phone avoid echo: otherwise its speaker is heard by its own microphone.").color(MUTED).small());
            }
        }
    }

    fn phone_audio(&mut self, ui: &mut egui::Ui, view: &View) {
        section(ui, "Audio from the phone");
        match &view.up {
            Some(up) => {
                let shape = if up.channels == 1 { "mono" } else { "stereo" };
                ui.label(RichText::new(format!("Receiving from {} · {shape} {} kHz", up.phone, up.sample_rate / 1000)).color(OK));
                level_meter(ui, self.up_level);
                if let Some((buffer, underruns, speed)) = self.engine.up_stats() {
                    ui.monospace(RichText::new(format!(
                        "buffer {buffer} ms  ·  underruns {underruns}  ·  {:+.0} ppm",
                        (speed - 1.0) * 1_000_000.0
                    )).color(MUTED));
                }
                ui.label(RichText::new("Plays on the default output device.").color(MUTED).small());
            }
            None => {
                ui.label(RichText::new("Nothing is being sent from a phone. Choose “To computer” or “Both” above.").color(MUTED));
            }
        }
        if let Some(problem) = &view.up_problem {
            ui.label(RichText::new(problem).color(WARN));
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        section(ui, "Settings");
        ui.horizontal(|ui| {
            ui.label("Port");
            let edit = ui.add(egui::TextEdit::singleline(&mut self.port_text).desired_width(70.0));
            let entered = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Apply").clicked() || entered)
                && let Ok(port) = self.port_text.trim().parse::<u16>()
            {
                self.engine.set_port(port);
            }
        });
        let mut usb = self.engine.settings.usb;
        if ui.checkbox(&mut usb, "Offer the USB route when a phone is plugged in (adb reverse)").changed() {
            self.engine.set_usb(usb);
        }
        if autostart::supported() {
            let mut enabled = self.autostart;
            if ui.checkbox(&mut enabled, "Launch at login").changed() {
                match autostart::set_enabled(enabled) {
                    Ok(()) => self.autostart = enabled,
                    Err(e) => eprintln!("[gui] launch at login: {e}"),
                }
            }
        }
        ui.horizontal(|ui| {
            if self.confirm_rotate {
                ui.label(RichText::new("Every paired phone will have to scan again.").color(WARN));
                if ui.button("Generate new token").clicked() {
                    self.engine.rotate_token();
                    self.confirm_rotate = false;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_rotate = false;
                }
            } else if ui.button("New pairing token…").clicked() {
                self.confirm_rotate = true;
            }
        });
        ui.label(RichText::new(format!("Tethertone {VERSION} · MIT License · github.com/AimenSayoud/tethertone")).color(MUTED).small());
    }
}

fn activity(ui: &mut egui::Ui, view: &View) {
    section(ui, "Activity");
    if view.activity.is_empty() {
        ui.label(RichText::new("Nothing yet.").color(MUTED));
    }
    for entry in view.activity.iter().take(10) {
        let colour = match entry.kind {
            ActivityKind::Connected => OK,
            ActivityKind::Rejected => BAD,
            ActivityKind::Silent => WARN,
            ActivityKind::Arrived => ACCENT,
            ActivityKind::Disconnected | ActivityKind::Info => MUTED,
        };
        let age = SystemTime::now().duration_since(entry.at).unwrap_or_default().as_secs();
        let when = if age < 60 { format!("{age}s") } else { format!("{}m", age / 60) };
        ui.horizontal_wrapped(|ui| {
            dot(ui, colour);
            ui.monospace(RichText::new(format!("{when:>4}")).color(MUTED));
            ui.label(&entry.text);
        });
    }
}

fn status(view: &View) -> (Color32, String) {
    if !view.running {
        return (MUTED, "Stopped".into());
    }
    match view.clients.len() {
        0 => (WARN, "Waiting for a phone".into()),
        1 => (OK, format!("Streaming to {}", view.clients[0].name)),
        n => (OK, format!("Streaming to {n} phones")),
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title.to_uppercase()).small().strong().color(MUTED));
}

fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Color32::from_rgb(17, 28, 51))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(30, 41, 59)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
}

fn dot(ui: &mut egui::Ui, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, colour);
}

/// Peak meter in decibels: a linear one looks dead for ordinary music.
fn level_meter(ui: &mut egui::Ui, level: f32) {
    let width = ui.available_width().min(260.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 8.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), Color32::from_rgb(30, 41, 59));
    let fraction = if level > 0.0001 { ((20.0 * level.log10() + 60.0) / 60.0).clamp(0.0, 1.0) } else { 0.0 };
    let mut filled = rect;
    filled.set_width(rect.width() * fraction);
    let colour = if fraction > 0.95 { BAD } else if fraction > 0.8 { WARN } else { OK };
    painter.rect_filled(filled, CornerRadius::same(3), colour);
}

fn logo_image() -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([56, 56], &icon::rgba(56))
}

/// Windows tray: Start/Stop, Open and Quit. Closing the window hides it here.
#[cfg(windows)]
mod tray {
    use std::sync::mpsc::{channel, Receiver};

    use eframe::egui::{self, ViewportCommand};
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    use super::{icon, App};

    pub struct Tray {
        _icon: TrayIcon,
        toggle: MenuId,
        open: MenuId,
        quit: MenuId,
        events: Receiver<MenuEvent>,
    }

    impl Tray {
        pub fn new(ctx: &egui::Context) -> Option<Tray> {
            let toggle = MenuItem::new("Start / Stop", true, None);
            let open = MenuItem::new("Open Tethertone", true, None);
            let quit = MenuItem::new("Quit", true, None);
            let menu = Menu::new();
            menu.append_items(&[&toggle, &open, &quit]).ok()?;
            let image = Icon::from_rgba(icon::rgba(32), 32, 32).ok()?;
            let tray = TrayIconBuilder::new()
                .with_tooltip("Tethertone")
                .with_icon(image)
                .with_menu(Box::new(menu))
                .build()
                .ok()?;
            // The window may be hidden, so wake the UI for every menu event.
            let (tx, rx) = channel();
            let ctx = ctx.clone();
            let open_id = open.id().clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if event.id == open_id {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                let _ = tx.send(event);
                ctx.request_repaint();
            }));
            Some(Tray { _icon: tray, toggle: toggle.id().clone(), open: open.id().clone(), quit: quit.id().clone(), events: rx })
        }
    }

    impl App {
        pub(super) fn handle_tray(&mut self, ctx: &egui::Context) {
            let Some(tray) = &self.tray else { return };
            // Closing hides to the tray; the server keeps streaming.
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            }
            let events: Vec<MenuEvent> = tray.events.try_iter().collect();
            let (toggle, open, quit) = (tray.toggle.clone(), tray.open.clone(), tray.quit.clone());
            for event in events {
                if event.id == toggle {
                    if self.engine.running() {
                        self.engine.stop();
                    } else {
                        let _ = self.engine.start();
                    }
                } else if event.id == quit {
                    self.tray = None;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                } else if event.id == open {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                }
            }
        }
    }
}
