//! Environment capability probes.
//!
//! Each probe is intentionally independent: a failing probe cannot hide the
//! result of another. The output is a fixed-column table sorted into four
//! logical groups (session, desktop services, launchers, kernel).

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use zbus::fdo::DBusProxy;
use zbus::Connection;

/// Status of a single probe.
#[derive(Debug)]
enum Status {
    /// Working and usable.
    Ok(String),
    /// Works but may warrant attention.
    Warn(String),
    /// Not present. Missing capabilities may be expected (e.g. Lutris not
    /// installed) or intentional (e.g. `input` group is optional).
    Missing(String),
}

#[derive(Debug)]
struct Check {
    name: &'static str,
    status: Status,
}

pub(crate) async fn run() -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    // Session group.
    checks.push(check_session());
    checks.push(check_desktop());

    // Desktop services over the session bus. Use a single connection.
    match Connection::session().await {
        Ok(session) => {
            checks.push(check_bus_name(&session, "kwin D-Bus", "org.kde.KWin").await);
            checks.push(check_bus_name(&session, "lutris D-Bus", "net.lutris.Lutris").await);
        }
        Err(e) => {
            let msg = format!("session bus unavailable: {e}");
            checks.push(Check {
                name: "kwin D-Bus",
                status: Status::Missing(msg.clone()),
            });
            checks.push(Check {
                name: "lutris D-Bus",
                status: Status::Missing(msg),
            });
        }
    }

    // logind lives on the system bus.
    checks.push(check_logind().await);

    // Launcher state on disk.
    checks.push(check_steam_dir());
    checks.push(check_heroic_dir());

    // Kernel capabilities.
    checks.push(check_drm());
    checks.push(check_input_group());
    checks.push(check_pidfd());

    print_table(&checks);
    Ok(())
}

fn check_session() -> Check {
    let xdg = env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let wayland = env::var("WAYLAND_DISPLAY").ok();
    let detail = match (xdg.as_str(), wayland) {
        ("wayland", Some(d)) => format!("wayland ({d})"),
        ("wayland", None) => "wayland".into(),
        ("x11", _) => "x11".into(),
        ("tty", _) => "tty".into(),
        (other, _) if !other.is_empty() => other.into(),
        _ => "unknown".into(),
    };
    let status = if detail.starts_with("wayland") || detail == "x11" {
        Status::Ok(detail)
    } else {
        Status::Warn(detail)
    };
    Check {
        name: "session type",
        status,
    }
}

fn check_desktop() -> Check {
    let de = env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".into());
    let status = if de.to_ascii_uppercase().contains("KDE") {
        Status::Ok(de)
    } else {
        Status::Warn(format!(
            "{de} (ludex targets KDE Plasma; other desktops are best-effort)"
        ))
    };
    Check {
        name: "desktop",
        status,
    }
}

async fn check_bus_name(conn: &Connection, label: &'static str, bus_name: &str) -> Check {
    let proxy = match DBusProxy::new(conn).await {
        Ok(p) => p,
        Err(e) => {
            return Check {
                name: label,
                status: Status::Missing(e.to_string()),
            };
        }
    };
    let names = match proxy.list_names().await {
        Ok(n) => n,
        Err(e) => {
            return Check {
                name: label,
                status: Status::Missing(e.to_string()),
            };
        }
    };
    let present = names.iter().any(|n| n.as_str() == bus_name);
    let status = if present {
        Status::Ok(format!("{bus_name} present"))
    } else {
        Status::Missing(format!("{bus_name} not owned on session bus"))
    };
    Check {
        name: label,
        status,
    }
}

async fn check_logind() -> Check {
    let system = match Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            return Check {
                name: "logind D-Bus",
                status: Status::Missing(format!("system bus: {e}")),
            };
        }
    };
    let proxy = match DBusProxy::new(&system).await {
        Ok(p) => p,
        Err(e) => {
            return Check {
                name: "logind D-Bus",
                status: Status::Missing(e.to_string()),
            };
        }
    };
    let names = proxy.list_names().await.unwrap_or_default();
    let present = names.iter().any(|n| n.as_str() == "org.freedesktop.login1");
    let status = if present {
        Status::Ok("org.freedesktop.login1 present".into())
    } else {
        Status::Missing("org.freedesktop.login1 not present".into())
    };
    Check {
        name: "logind D-Bus",
        status,
    }
}

fn check_steam_dir() -> Check {
    let Some(home) = env::var_os("HOME") else {
        return Check {
            name: "steam data dir",
            status: Status::Missing("HOME unset".into()),
        };
    };
    let steam = PathBuf::from(home).join(".local/share/Steam");
    if !steam.is_dir() {
        return Check {
            name: "steam data dir",
            status: Status::Missing(format!("{} not present", steam.display())),
        };
    }
    let mut detail = steam.display().to_string();
    let log = steam.join("logs/content_log.txt");
    if let Ok(meta) = fs::metadata(&log) {
        let size_mib = meta.len() / (1024 * 1024);
        if let Some(age) = meta
            .modified()
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
        {
            let _ = write!(
                detail,
                " (content_log.txt: {size_mib} MiB, last modified {} ago)",
                humantime_short(age)
            );
        } else {
            let _ = write!(detail, " (content_log.txt: {size_mib} MiB)");
        }
    } else {
        detail.push_str(" (no content_log.txt yet)");
    }
    Check {
        name: "steam data dir",
        status: Status::Ok(detail),
    }
}

fn check_heroic_dir() -> Check {
    let Some(home) = env::var_os("HOME") else {
        return Check {
            name: "heroic config dir",
            status: Status::Missing("HOME unset".into()),
        };
    };
    let heroic = PathBuf::from(home).join(".config/heroic");
    let status = if heroic.is_dir() {
        Status::Ok(heroic.display().to_string())
    } else {
        Status::Missing("not present (Heroic not installed?)".into())
    };
    Check {
        name: "heroic config dir",
        status,
    }
}

fn check_drm() -> Check {
    let path = Path::new("/sys/class/drm");
    if !path.is_dir() {
        return Check {
            name: "DRM subsystem",
            status: Status::Missing("/sys/class/drm not present".into()),
        };
    }
    let cards = match fs::read_dir(path) {
        Ok(rd) => rd
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with("card") && !n.contains('-'))
            .collect::<Vec<_>>(),
        Err(e) => {
            return Check {
                name: "DRM subsystem",
                status: Status::Warn(e.to_string()),
            };
        }
    };
    let status = if cards.is_empty() {
        Status::Warn("no cards found under /sys/class/drm".into())
    } else {
        Status::Ok(cards.join(", "))
    };
    Check {
        name: "DRM subsystem",
        status,
    }
}

fn check_input_group() -> Check {
    let status_text = match fs::read_to_string("/proc/self/status") {
        Ok(s) => s,
        Err(e) => {
            return Check {
                name: "input group",
                status: Status::Warn(e.to_string()),
            };
        }
    };
    let groups_line = status_text
        .lines()
        .find(|l| l.starts_with("Groups:"))
        .unwrap_or("");
    let gids: Vec<u32> = groups_line
        .trim_start_matches("Groups:")
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();

    let group_file = fs::read_to_string("/etc/group").unwrap_or_default();
    let input_gid = group_file.lines().find_map(|line| {
        let mut parts = line.splitn(4, ':');
        let name = parts.next()?;
        let _passwd = parts.next()?;
        let gid: u32 = parts.next()?.parse().ok()?;
        if name == "input" {
            Some(gid)
        } else {
            None
        }
    });

    let status = match input_gid {
        None => Status::Warn("no 'input' group on this system".into()),
        Some(gid) if gids.contains(&gid) => Status::Ok("member of 'input'".into()),
        Some(_) => {
            Status::Missing("not a member (optional; only required for the evdev feature)".into())
        }
    };
    Check {
        name: "input group",
        status,
    }
}

fn check_pidfd() -> Check {
    let pid = rustix::process::getpid();
    match rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()) {
        Ok(_fd) => Check {
            name: "pidfd syscall",
            status: Status::Ok("supported".into()),
        },
        Err(e) => Check {
            name: "pidfd syscall",
            status: Status::Missing(e.to_string()),
        },
    }
}

fn humantime_short(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn print_table(checks: &[Check]) {
    let width = checks
        .iter()
        .map(|c| c.name.len())
        .max()
        .unwrap_or(0)
        .max(18);
    println!(
        "{:<width$}  {:<8}  detail",
        "component",
        "status",
        width = width
    );
    println!("{}", "─".repeat(width + 60));
    for c in checks {
        let (tag, detail) = match &c.status {
            Status::Ok(d) => ("ok", d.as_str()),
            Status::Warn(d) => ("warn", d.as_str()),
            Status::Missing(d) => ("missing", d.as_str()),
        };
        println!("{:<width$}  {tag:<8}  {detail}", c.name, width = width);
    }
}
