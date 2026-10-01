//! Tethertone for Windows and Linux.
//!
//!     tethertone                         the window
//!     tethertone --headless              server only; prints the pairing link and QR
//!     tethertone --headless --test-tone  a synthetic tone instead of system audio (tests, CI)
//!     tethertone --port 45679            another port (saved)
//!     tethertone --no-usb                don't set up adb reverse (saved)

// No console window behind the GUI on Windows. Headless runs from a console anyway.
#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

mod adb;
mod audio;
#[cfg(feature = "gui")]
mod autostart;
mod engine;
#[cfg(feature = "gui")]
mod icon;
mod net;
mod qr;
#[cfg(feature = "gui")]
mod gui;

use audio::CaptureKind;
use engine::Engine;

pub const VERSION: &str = env!("TETHERTONE_VERSION");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    if has("--version") {
        println!("tethertone {VERSION}");
        return;
    }
    if has("--help") || has("-h") {
        println!(
            "Tethertone {VERSION} — your computer's audio on your Android phone, and back.\n\n\
             tethertone                         open the window\n\
             tethertone --headless              server only: prints the pairing link and QR code\n\
             tethertone --headless --test-tone  stream a test tone instead of system audio\n\
             --port <n>                         listen on another port (saved)\n\
             --no-usb                           don't set up the USB route (saved)"
        );
        return;
    }

    let kind = if has("--test-tone") { CaptureKind::TestTone } else { CaptureKind::System };
    let headless = has("--headless") || !cfg!(feature = "gui");
    let mut engine = Engine::new(kind, headless);

    if let Some(port) = args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)?.parse().ok()) {
        engine.settings.port = port;
        engine.settings.save();
    }
    if has("--no-usb") {
        engine.settings.usb = false;
        engine.settings.save();
    }

    if headless {
        run_headless(engine);
    } else {
        #[cfg(feature = "gui")]
        gui::run(engine);
    }
}

fn run_headless(mut engine: Engine) {
    // The GUI build has no console of its own on Windows; borrow the one we
    // were started from so headless output is visible.
    #[cfg(all(windows, feature = "gui"))]
    {
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        unsafe { AttachConsole(u32::MAX) }; // ATTACH_PARENT_PROCESS
    }
    if let Err(e) = engine.start() {
        eprintln!("[server] failed to start: {e}");
        std::process::exit(1);
    }
    let view = engine.view();
    if let Some(pairing) = &view.pairing {
        println!("[net]    reachable at: {}", pairing.hosts.join(", "));
        println!("[pair]   {}", pairing.uri());
        println!("\n{}", qr::terminal(&pairing.uri()));
    }
    println!("[server] ready — Ctrl+C to stop.");
    loop {
        std::thread::park();
    }
}
