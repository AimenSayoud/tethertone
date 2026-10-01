//! The USB route: `adb reverse` makes the phone's own 127.0.0.1 reach this
//! computer over the cable. Looked up in the usual SDK locations as well as
//! PATH, because a GUI app does not always inherit the shell's PATH.

use std::path::PathBuf;
use std::process::Command;

pub fn executable() -> Option<PathBuf> {
    let name = if cfg!(windows) { "adb.exe" } else { "adb" };
    let from_path = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).map(|p| p.join(name)).collect::<Vec<_>>())
        .unwrap_or_default();
    let home = std::env::home_dir().unwrap_or_default();
    let mut known = vec![home.join("Android/Sdk/platform-tools").join(name)];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        known.push(PathBuf::from(local).join("Android\\Sdk\\platform-tools").join(name));
    }
    known.push(PathBuf::from("/usr/bin").join(name));
    known.push(PathBuf::from("/opt/homebrew/bin").join(name));
    known.push(PathBuf::from("/usr/local/bin").join(name));
    from_path.into_iter().chain(known).find(|p| p.is_file())
}

fn run(args: &[&str]) -> Option<String> {
    let adb = executable()?;
    let mut command = Command::new(adb);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashing up
    }
    let output = command.output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Serials of phones that are attached and authorised for debugging.
pub fn attached_devices() -> Vec<String> {
    run(&["devices"])
        .map(|out| {
            out.lines()
                .skip(1)
                .filter_map(|line| {
                    let mut parts = line.split_whitespace();
                    let serial = parts.next()?;
                    (parts.next() == Some("device")).then(|| serial.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn reverse(port: u16, serial: &str) -> bool {
    let spec = format!("tcp:{port}");
    run(&["-s", serial, "reverse", &spec, &spec]).is_some()
}

pub fn remove_reverse(port: u16, serial: &str) {
    let spec = format!("tcp:{port}");
    let _ = run(&["-s", serial, "reverse", "--remove", &spec]);
}
