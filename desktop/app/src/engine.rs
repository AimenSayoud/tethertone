//! Everything the app does, shared by the window and headless mode: capture,
//! the server, playback of the phone's audio, and the USB route.
//!
//! The UI reads a [`View`] snapshot and calls methods; server callbacks update
//! the view from their own threads.

use std::collections::VecDeque;
use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tethertone_core::jitter::UpstreamBuffer;
use tethertone_core::pairing::{self, PairingInfo};
use tethertone_core::protocol::peak_s16le;
use tethertone_core::server::{ActivityKind, ServerEvents};
use tethertone_core::{ClientSnapshot, Server, ServerConfig, StreamFormat, StreamMode};

use crate::audio::{self, CaptureKind, StreamHandle};
use crate::{adb, net};

pub const DEFAULT_PORT: u16 = 45678;

/// Kept in ~/.tethertone/desktop.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub port: u16,
    pub usb: bool,
    pub was_running: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { port: DEFAULT_PORT, usb: true, was_running: false }
    }
}

impl Settings {
    fn path() -> std::path::PathBuf {
        pairing::config_dir().join("desktop.json")
    }

    pub fn load() -> Settings {
        fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = fs::create_dir_all(pairing::config_dir());
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(Self::path(), json);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActivityEntry {
    pub at: SystemTime,
    pub kind: ActivityKind,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct UpInfo {
    pub phone: String,
    pub sample_rate: u32,
    pub channels: u16,
}

/// What the UI shows. Cloned once per frame.
#[derive(Debug, Clone, Default)]
pub struct View {
    pub running: bool,
    pub clients: Vec<ClientSnapshot>,
    pub activity: VecDeque<ActivityEntry>,
    pub pairing: Option<PairingInfo>,
    pub error: Option<String>,
    pub format: Option<StreamFormat>,
    pub usb_serial: Option<String>,
    pub up: Option<UpInfo>,
    /// The last problem with the phone's audio, e.g. a declined request.
    pub up_problem: Option<String>,
    pub bytes_sent: u64,
}

pub struct Shared {
    view: Mutex<View>,
    level: AtomicU32,
    up_level: AtomicU32,
    upstream: Mutex<Option<(Arc<Mutex<UpstreamBuffer>>, StreamHandle)>>,
    /// Headless mode prints activity as it happens.
    echo: bool,
    /// Test mode: never touch a real audio device.
    null_audio: bool,
    repaint: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Shared {
    fn update(&self, change: impl FnOnce(&mut View)) {
        change(&mut self.view.lock().unwrap());
        if let Some(repaint) = self.repaint.lock().unwrap().as_ref() {
            repaint();
        }
    }

    fn log(&self, kind: ActivityKind, text: String) {
        if self.echo {
            println!("[{kind:?}] {text}");
        }
        self.update(|v| {
            v.activity.push_front(ActivityEntry { at: SystemTime::now(), kind, text });
            v.activity.truncate(60);
        });
    }

    fn take_level(slot: &AtomicU32) -> f32 {
        f32::from_bits(slot.swap(0, Ordering::Relaxed))
    }
}

/// Peaks are non-negative, so comparing their bit patterns compares the values.
fn raise_level(slot: &AtomicU32, peak: f32) {
    slot.fetch_max(peak.to_bits(), Ordering::Relaxed);
}

impl ServerEvents for Shared {
    fn clients_changed(&self, clients: Vec<ClientSnapshot>) {
        self.update(|v| v.clients = clients);
    }

    fn activity(&self, kind: ActivityKind, text: String) {
        self.log(kind, text);
    }

    fn up_started(&self, sample_rate: u32, channels: u16, phone: String) {
        let buffer = Arc::new(Mutex::new(UpstreamBuffer::new(sample_rate, channels, 60)));
        *self.upstream.lock().unwrap() = None; // stop any previous player first
        let started = if self.null_audio {
            audio::start_null_playback(buffer.clone())
        } else {
            audio::start_playback(buffer.clone())
        };
        match started {
            Ok(handle) => {
                *self.upstream.lock().unwrap() = Some((buffer, handle));
                let shape = if channels == 1 { "mono" } else { "stereo" };
                self.log(ActivityKind::Connected, format!("{phone} is sending audio ({shape}, {} kHz)", sample_rate / 1000));
                self.update(|v| {
                    v.up = Some(UpInfo { phone, sample_rate, channels });
                    v.up_problem = None;
                });
            }
            Err(e) => self.update(|v| v.up_problem = Some(e)),
        }
    }

    fn up_stopped(&self) {
        let was = self.upstream.lock().unwrap().take().is_some();
        let phone = self.view.lock().unwrap().up.as_ref().map(|u| u.phone.clone());
        if let (true, Some(phone)) = (was, phone) {
            self.log(ActivityKind::Info, format!("{phone} stopped sending audio"));
        }
        self.update(|v| v.up = None);
    }

    fn up_audio(&self, pcm: &[u8]) {
        if let Some((buffer, _)) = self.upstream.lock().unwrap().as_ref() {
            let peak = buffer.lock().unwrap().push_s16le(pcm);
            raise_level(&self.up_level, peak);
        }
    }

    fn refused(&self, phone: String, reason: String) {
        self.log(ActivityKind::Info, format!("{phone} declined: {reason}"));
        self.update(|v| v.up_problem = Some(format!("{phone}: {reason}")));
    }

    fn listener_failed(&self, error: String) {
        self.update(|v| v.error = Some(format!("The server stopped accepting connections: {error}")));
    }
}

pub struct Engine {
    pub shared: Arc<Shared>,
    pub settings: Settings,
    pub capture_kind: CaptureKind,
    server: Option<Arc<Server>>,
    capture: Option<StreamHandle>,
    usb_watch: Option<Sender<()>>,
}

impl Engine {
    pub fn new(capture_kind: CaptureKind, echo: bool) -> Engine {
        Engine {
            shared: Arc::new(Shared {
                view: Mutex::new(View::default()),
                level: AtomicU32::new(0),
                up_level: AtomicU32::new(0),
                upstream: Mutex::new(None),
                echo,
                null_audio: capture_kind == CaptureKind::TestTone,
                repaint: Mutex::new(None),
            }),
            settings: Settings::load(),
            capture_kind,
            server: None,
            capture: None,
            usb_watch: None,
        }
    }

    /// Called whenever the view changes, e.g. to wake the window.
    pub fn on_change(&self, repaint: impl Fn() + Send + Sync + 'static) {
        *self.shared.repaint.lock().unwrap() = Some(Box::new(repaint));
    }

    pub fn view(&self) -> View {
        let mut view = self.shared.view.lock().unwrap().clone();
        view.bytes_sent = self.server.as_ref().map_or(0, |s| s.bytes_sent());
        view
    }

    /// Peak levels since the last call, 0..=1: (captured, from the phone).
    pub fn take_levels(&self) -> (f32, f32) {
        (Shared::take_level(&self.shared.level), Shared::take_level(&self.shared.up_level))
    }

    pub fn running(&self) -> bool {
        self.server.is_some()
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.running() {
            return Ok(());
        }
        let result = self.try_start();
        if let Err(e) = &result {
            self.stop_internal(false);
            self.shared.update(|v| v.error = Some(e.clone()));
        }
        result
    }

    fn try_start(&mut self) -> Result<(), String> {
        let format = audio::capture_format(self.capture_kind)?;
        let token = pairing::load_token(false).map_err(|e| format!("Could not read the pairing token: {e}"))?;
        let config = ServerConfig {
            port: self.settings.port,
            token,
            name: net::machine_name(),
            format,
            max_queued: 24,
        };
        let server = Arc::new(
            Server::start(config, self.shared.clone())
                .map_err(|e| format!("Port {} is unavailable: {e}", self.settings.port))?,
        );

        let to_server = server.clone();
        let shared = self.shared.clone();
        let capture = audio::start_capture(
            self.capture_kind,
            format,
            Box::new(move |block| {
                to_server.broadcast(block);
                raise_level(&shared.level, peak_s16le(block));
            }),
        )?;

        self.server = Some(server);
        self.capture = Some(capture);
        self.settings.was_running = true;
        self.settings.save();
        self.shared.update(|v| {
            v.running = true;
            v.error = None;
            v.format = Some(format);
        });
        self.rebuild_pairing();
        if self.settings.usb {
            self.start_usb_watch();
        }
        let source = match self.capture_kind {
            CaptureKind::System => "system audio",
            CaptureKind::TestTone => "test tone",
        };
        self.shared.log(
            ActivityKind::Info,
            format!("Listening on port {} · {source} · {} Hz {}ch", self.settings.port, format.sample_rate, format.channels),
        );
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stop_internal(true);
    }

    fn stop_internal(&mut self, user_initiated: bool) {
        let was_running = self.running();
        drop(self.usb_watch.take()); // the watcher removes its reverse on the way out
        self.capture = None;
        *self.shared.upstream.lock().unwrap() = None;
        if let Some(server) = self.server.take() {
            server.stop();
        }
        if user_initiated {
            self.settings.was_running = false;
            self.settings.save();
            if was_running {
                self.shared.log(ActivityKind::Info, "Server stopped".into());
            }
        }
        self.shared.update(|v| {
            v.running = false;
            v.clients.clear();
            v.up = None;
            v.usb_serial = None;
        });
    }

    pub fn restart(&mut self) {
        if self.running() {
            self.stop_internal(false);
            let _ = self.start();
        }
    }

    pub fn set_port(&mut self, port: u16) {
        if port == 0 || port == self.settings.port {
            return;
        }
        self.settings.port = port;
        self.settings.save();
        self.restart();
    }

    pub fn set_usb(&mut self, enabled: bool) {
        self.settings.usb = enabled;
        self.settings.save();
        self.restart();
    }

    /// Invalidates every paired phone.
    pub fn rotate_token(&mut self) {
        if let Err(e) = pairing::load_token(true) {
            self.shared.update(|v| v.error = Some(format!("Could not write a new token: {e}")));
            return;
        }
        self.shared.log(ActivityKind::Info, "New pairing token: phones must scan the QR again".into());
        if self.running() {
            self.restart();
        } else {
            self.rebuild_pairing();
        }
    }

    pub fn request_mode(&self, client: u64, mode: StreamMode) {
        if !mode.is_valid() {
            return;
        }
        self.shared.update(|v| v.up_problem = None);
        if let Some(server) = &self.server {
            server.request_mode(client, mode);
        }
    }

    pub fn disconnect(&self, client: u64) {
        if let Some(server) = &self.server {
            server.disconnect(client);
        }
    }

    /// Phone audio buffer depth (ms), underruns and playback speed, if playing.
    pub fn up_stats(&self) -> Option<(i64, u64, f64)> {
        let upstream = self.shared.upstream.lock().unwrap();
        let (buffer, _) = upstream.as_ref()?;
        let b = buffer.lock().unwrap();
        Some((b.buffered_ms(), b.underruns, b.speed()))
    }

    fn rebuild_pairing(&self) {
        rebuild_pairing(&self.shared, self.settings.port);
    }

    /// Polls for phones on USB and keeps `adb reverse` in place for the first one.
    fn start_usb_watch(&mut self) {
        if adb::executable().is_none() {
            self.shared.log(ActivityKind::Info, "adb not found: pairing is Wi-Fi only (install Android platform-tools for USB)".into());
            return;
        }
        let (stop, stopped) = channel::<()>();
        let shared = self.shared.clone();
        let port = self.settings.port;
        let spawned = thread::Builder::new().name("tethertone-usb".into()).spawn(move || {
            let mut current: Option<String> = None;
            loop {
                let devices = adb::attached_devices();
                let still_there = current.as_ref().is_some_and(|s| devices.contains(s));
                if !still_there {
                    if let Some(gone) = current.take() {
                        shared.log(ActivityKind::Info, format!("{gone} unplugged: Wi-Fi only"));
                    }
                    if let Some(serial) = devices.first()
                        && adb::reverse(port, serial)
                    {
                        shared.log(ActivityKind::Info, format!("{serial}: USB route ready (adb reverse tcp:{port})"));
                        current = Some(serial.clone());
                    }
                    let serial = current.clone();
                    shared.update(|v| v.usb_serial = serial);
                    rebuild_pairing(&shared, port);
                }
                match stopped.recv_timeout(Duration::from_secs(3)) {
                    Err(RecvTimeoutError::Timeout) => continue,
                    _ => break,
                }
            }
            if let Some(serial) = current {
                adb::remove_reverse(port, &serial);
            }
        });
        if spawned.is_ok() {
            self.usb_watch = Some(stop);
        }
    }
}

/// Loopback first when the cable is live: the phone races the hosts in order,
/// and a small head start is enough for USB to win.
fn rebuild_pairing(shared: &Arc<Shared>, port: u16) {
    let token = pairing::load_token(false).unwrap_or_default();
    let (usb, format) = {
        let view = shared.view.lock().unwrap();
        (view.usb_serial.is_some(), view.format.unwrap_or_default())
    };
    let mut hosts = Vec::new();
    if usb {
        hosts.push("127.0.0.1".to_string());
    }
    hosts.extend(net::local_addresses());
    let info = PairingInfo {
        hosts,
        port,
        token,
        rate: format.sample_rate,
        channels: format.channels,
        name: net::machine_name(),
    };
    shared.update(|v| v.pairing = Some(info));
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop_internal(false);
    }
}
