//! Launch at login, without extra dependencies: the per-user Run key on
//! Windows, an XDG autostart entry on Linux.

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

pub fn supported() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

pub fn is_enabled() -> bool {
    #[cfg(windows)]
    {
        quiet(std::process::Command::new("reg").args(["query", RUN_KEY, "/v", "Tethertone"]))
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        desktop_file().is_file()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        let mut command = std::process::Command::new("reg");
        if enabled {
            command.args(["add", RUN_KEY, "/v", "Tethertone", "/t", "REG_SZ", "/f", "/d"]);
            command.arg(format!("\"{}\"", exe.display()));
        } else {
            command.args(["delete", RUN_KEY, "/v", "Tethertone", "/f"]);
        }
        let output = quiet(&mut command).map_err(|e| e.to_string())?;
        return if output.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&output.stderr).into_owned()) };
    }
    #[cfg(target_os = "linux")]
    {
        let path = desktop_file();
        if enabled {
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            let entry = format!(
                "[Desktop Entry]\nType=Application\nName=Tethertone\nComment=Your computer's audio on your Android phone\nExec=\"{}\"\nX-GNOME-Autostart-enabled=true\n",
                exe.display()
            );
            return std::fs::write(&path, entry).map_err(|e| e.to_string());
        }
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    #[allow(unreachable_code)]
    {
        let _ = (enabled, exe);
        Err("Launch at login is not supported on this platform.".into())
    }
}

#[cfg(target_os = "linux")]
fn desktop_file() -> std::path::PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::home_dir().unwrap_or_default().join(".config"));
    config.join("autostart/tethertone.desktop")
}

#[cfg(windows)]
fn quiet(command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000).output() // CREATE_NO_WINDOW
}
