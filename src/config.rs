//! `settings.json` persistence, server-URL normalization, and HKCU autostart.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::constants::{APP_DIR_NAME, DEFAULT_IDLE_POLL_SECONDS};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Empty means "not configured yet".
    pub server_url: String,
    /// `0` disables idle polling (refresh on flyout open only).
    pub idle_poll_seconds: u64,
    pub pin_flyout: bool,
    pub launch_at_startup: bool,
    /// `Some` when the user dragged the flyout to a custom height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flyout_height: Option<f32>,
    /// Subtle synthesized UI ticks for actions and expansion.
    #[serde(default = "default_true")]
    pub ui_sounds: bool,
    /// Saved card order by schema group path. Missing paths are retained so a
    /// device keeps its slot if it disappears and comes back.
    #[serde(default)]
    pub device_order: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            idle_poll_seconds: DEFAULT_IDLE_POLL_SECONDS,
            pin_flyout: false,
            launch_at_startup: false,
            flyout_height: None,
            ui_sounds: true,
            device_order: Vec::new(),
        }
    }
}

/// `%APPDATA%\PiDA-Tray` on Windows.
pub fn app_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR_NAME)
}

pub fn settings_path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

impl Settings {
    /// Missing or corrupt files fall back to defaults.
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(settings_path(dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let path = settings_path(dir);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)
    }
}

/// Prepend `http://` if no scheme, require http/https + a host, strip trailing `/`.
pub fn normalize_server_url(input: &str) -> Result<String, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("Enter a server URL".into());
    }
    if s.chars().any(char::is_whitespace) {
        return Err("URL must not contain spaces".into());
    }

    let with_scheme = match s.find("://") {
        Some(idx) => {
            let scheme = s[..idx].to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return Err("Only http and https are supported".into());
            }
            format!("{scheme}{}", &s[idx..])
        }
        None => format!("http://{s}"),
    };

    if with_scheme.contains('?') || with_scheme.contains('#') {
        return Err("URL must not contain a query or fragment".into());
    }

    let rest = with_scheme.split_once("://").map(|(_, r)| r).unwrap_or("");
    let authority = rest.split('/').next().unwrap_or("");
    if authority.contains('@') {
        return Err("Credentials in the URL are not supported".into());
    }

    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        let (h, after) = stripped
            .split_once(']')
            .ok_or_else(|| "Unterminated IPv6 address".to_string())?;
        (h, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err("URL is missing a host".into());
    }
    if let Some(p) = port {
        match p.parse::<u16>() {
            Ok(n) if n > 0 => {}
            _ => return Err("Port must be a number from 1 to 65535".into()),
        }
    }

    Ok(with_scheme.trim_end_matches('/').to_string())
}

// ---------------------------------------------------------------------------
// Autostart (HKCU\Software\Microsoft\Windows\CurrentVersion\Run\PiDATray)
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteValueW, RegSetValueExW,
    };
    use windows::core::{HSTRING, PCWSTR, w};

    let name = HSTRING::from(crate::constants::AUTOSTART_VALUE_NAME);
    let subkey = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

    // SAFETY: wide pointers remain valid for the duration of each registry call.
    unsafe {
        let mut hkey = HKEY::default();
        let err = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        );
        if err != ERROR_SUCCESS {
            return Err(format!("registry open failed ({})", err.0));
        }

        let res = if enabled {
            match std::env::current_exe() {
                Ok(exe) => {
                    let quoted = format!("\"{}\"", exe.display());
                    let wide: Vec<u16> = quoted.encode_utf16().chain(std::iter::once(0)).collect();
                    let bytes =
                        std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2);
                    let err =
                        RegSetValueExW(hkey, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes));
                    if err == ERROR_SUCCESS {
                        Ok(())
                    } else {
                        Err(format!("registry write failed ({})", err.0))
                    }
                }
                Err(e) => Err(e.to_string()),
            }
        } else {
            let err = RegDeleteValueW(hkey, PCWSTR(name.as_ptr()));
            if err == ERROR_SUCCESS || err == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("registry delete failed ({})", err.0))
            }
        };
        let _ = RegCloseKey(hkey);
        res
    }
}

#[cfg(not(windows))]
pub fn set_autostart(_enabled: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_scheme_and_strips_slash() {
        assert_eq!(
            normalize_server_url("192.0.2.10:9999").unwrap(),
            "http://192.0.2.10:9999"
        );
        assert_eq!(
            normalize_server_url("  http://192.0.2.10:9999//  ").unwrap(),
            "http://192.0.2.10:9999"
        );
        assert_eq!(
            normalize_server_url("HTTPS://pida.example.test/").unwrap(),
            "https://pida.example.test"
        );
    }

    #[test]
    fn keeps_path_prefix() {
        assert_eq!(
            normalize_server_url("pida.example.test/base/").unwrap(),
            "http://pida.example.test/base"
        );
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "   ",
            "ftp://x.test",
            "http://",
            "http://:80",
            "http://h:abc",
            "http://h:0",
            "http://h:70000",
            "http://a b",
            "http://u:p@h",
            "http://h/?q=1",
            "http://[::1",
        ] {
            assert!(
                normalize_server_url(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_ipv6() {
        assert_eq!(
            normalize_server_url("[2001:db8::1]:9999").unwrap(),
            "http://[2001:db8::1]:9999"
        );
    }

    #[test]
    fn settings_defaults_and_partial_json() {
        let s: Settings = serde_json::from_str(r#"{"pin_flyout": true}"#).unwrap();
        assert!(s.pin_flyout);
        assert_eq!(s.idle_poll_seconds, 60);
        assert!(s.server_url.is_empty());
        assert!(s.ui_sounds);
        assert!(s.device_order.is_empty());
    }

    #[test]
    fn settings_round_trip_and_corrupt_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings {
            server_url: "http://192.0.2.10:9999".into(),
            idle_poll_seconds: 30,
            pin_flyout: true,
            launch_at_startup: false,
            flyout_height: Some(700.0),
            ui_sounds: true,
            device_order: Vec::new(),
        };
        s.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), s);
        std::fs::write(settings_path(dir.path()), "{ not json").unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
    }
}
