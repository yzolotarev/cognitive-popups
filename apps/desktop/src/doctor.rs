//! Read-only diagnostics; no key values or database contents are printed.
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

fn executable(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(dir.join(name))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

fn report(name: &str, ok: bool, detail: &str) {
    println!("{} {name}: {detail}", if ok { "[есть]" } else { "[нет]" });
}

fn dbus_owner(name: &str) -> bool {
    Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--timeout",
            "3",
            "--dest",
            "org.freedesktop.DBus",
            "--object-path",
            "/org/freedesktop/DBus",
            "--method",
            "org.freedesktop.DBus.NameHasOwner",
            name,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("true"))
}

fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn database_path() -> PathBuf {
    std::env::var_os("COGNITIVE_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir().join("cognitive-popups/journal.db"))
}

fn database_status(path: &Path) -> (bool, String) {
    if !path.exists() {
        return (
            false,
            format!("{} — будет создана при первом запуске", path.display()),
        );
    }
    use std::io::Read;
    let valid = std::fs::File::open(path)
        .and_then(|mut f| {
            let mut header = [0; 16];
            f.read_exact(&mut header)?;
            Ok(&header == b"SQLite format 3\0")
        })
        .unwrap_or(false);
    (
        valid,
        format!(
            "{} — {}",
            path.display(),
            if valid {
                "файл SQLite доступен"
            } else {
                "не удалось прочитать заголовок SQLite; проверьте права и файл"
            }
        ),
    )
}

pub fn run() -> ExitCode {
    let gtk = gtk::init().is_ok();
    report(
        "GTK 4",
        gtk,
        if gtk {
            "инициализирован"
        } else {
            "нет доступного дисплея; установите gtk4 и запустите в графической сессии"
        },
    );
    let layer = gtk && gtk4_layer_shell::is_supported();
    report(
        "gtk4-layer-shell",
        layer,
        if layer {
            "протокол доступен"
        } else {
            "установите gtk4-layer-shell и используйте поддерживающий layer-shell композитор"
        },
    );
    let hypr = Command::new("hyprctl")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    report(
        "Hyprland",
        hypr,
        if hypr {
            "hyprctl доступен"
        } else {
            "установите hyprland и войдите в сессию Hyprland"
        },
    );
    for (name, package) in [
        ("wl-copy", "wl-clipboard"),
        ("wl-paste", "wl-clipboard"),
        ("wtype", "wtype"),
        ("playerctl", "playerctl"),
        ("secret-tool", "libsecret"),
    ] {
        let present = executable(name);
        report(
            name,
            present,
            &if present {
                "команда доступна".into()
            } else {
                format!("установите: sudo pacman -S {package}")
            },
        );
    }
    let watcher = dbus_owner("org.kde.StatusNotifierWatcher");
    report(
        "трей",
        watcher,
        if watcher {
            "StatusNotifierWatcher доступен"
        } else {
            "запустите панель с поддержкой StatusNotifierItem, например waybar"
        },
    );
    let secret = dbus_owner("org.freedesktop.secrets");
    report(
        "связка ключей",
        secret,
        if secret {
            "Secret Service доступен"
        } else {
            "установите gnome-keyring и запустите Secret Service; доступен запасной файл с правами 600"
        },
    );
    let key = cognitive_exoskeleton_ai::config_from_env()
        .ok()
        .is_some_and(|c| c.key().is_some_and(|k| !k.trim().is_empty()));
    report(
        "ключ ИИ",
        key,
        if key {
            "настроен (значение не показывается)"
        } else {
            "откройте хаб на вкладке «ИИ» и добавьте ключ"
        },
    );
    let prompts = crate::ctx::prompts_dir();
    report(
        "промпты",
        prompts.is_dir(),
        &format!(
            "{}{}",
            prompts.display(),
            if prompts.is_dir() {
                ""
            } else {
                " — установите файлы промптов"
            }
        ),
    );
    let (ok, message) = database_status(&database_path());
    report("база", ok, &message);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_probe_does_not_create_or_modify_files() {
        let dir = std::env::temp_dir().join(format!("cp-doctor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("journal.db");
        assert!(!database_status(&db).0);
        assert!(!db.exists());
        std::fs::write(&db, b"not a database").unwrap();
        assert!(!database_status(&db).0);
        std::fs::write(&db, b"SQLite format 3\0").unwrap();
        assert!(database_status(&db).0);
        assert_eq!(std::fs::read(&db).unwrap(), b"SQLite format 3\0");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
