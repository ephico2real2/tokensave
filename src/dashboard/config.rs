//! The dashboard's settings: `[dashboard]` in `dashboard.toml` in tokensave's
//! data folder, overridden by `TOKENSAVE_DASHBOARD_*` environment variables,
//! then by command-line flags.
//!
//! Nothing breaks when the file is missing: every setting has a default. A
//! value that is present but wrong is an error that names the setting and
//! where it came from, so `dashboard start` refuses it instead of guessing.
//! Keys this binary does not know (`at_login`, `workspaces`, `[sources]`,
//! `[colors]`, added by later stories) are ignored.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// The file name, in tokensave's data folder.
pub const FILE_NAME: &str = "dashboard.toml";

/// The port when none is configured: below Linux's ephemeral range.
pub const DEFAULT_PORT: u16 = 7311;

/// The page's theme when none is chosen in the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
    Auto,
}

impl Theme {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

/// The resolved settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardConfig {
    pub enabled: bool,
    pub port: u16,
    /// Always a loopback address: [`DashboardConfig::resolve`] refuses any other.
    pub bind: IpAddr,
    /// The preset range, in days, used when a request names none.
    pub default_range_days: u32,
    pub theme: Theme,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            bind: IpAddr::from([127, 0, 0, 1]),
            default_range_days: 30,
            theme: Theme::Dark,
        }
    }
}

/// A setting that could not be used, named with where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// The `[dashboard]` table as written, before validation.
#[derive(Debug, Default, serde::Deserialize)]
struct Section {
    enabled: Option<bool>,
    port: Option<i64>,
    bind: Option<String>,
    default_range: Option<String>,
    theme: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct File {
    #[serde(default)]
    dashboard: Section,
}

/// The preset ranges, in days.
pub const PRESET_DAYS: [u32; 5] = [7, 30, 45, 60, 90];

/// A preset range written as `30` or `30d`, in days.
pub fn parse_preset(value: &str) -> Option<u32> {
    let days: u32 = value.strip_suffix('d').unwrap_or(value).parse().ok()?;
    PRESET_DAYS.contains(&days).then_some(days)
}

/// The path of `dashboard.toml` in tokensave's data folder `home`.
pub fn path_in(home: &Path) -> PathBuf {
    home.join(FILE_NAME)
}

impl DashboardConfig {
    /// Reads `dashboard.toml` in `home` and the environment, and validates
    /// the result. `port` (the `--port` flag) wins over both.
    pub fn load(home: &Path, port: Option<u16>) -> Result<Self, ConfigError> {
        let path = path_in(home);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(ConfigError(format!(
                    "cannot read {}: {error}",
                    path.display()
                )))
            }
        };
        Self::resolve(
            text.as_deref(),
            &path,
            |name| std::env::var(name).ok(),
            port,
        )
    }

    /// [`Self::load`] with its inputs passed in, so it is tested without
    /// touching the process environment.
    pub fn resolve(
        text: Option<&str>,
        path: &Path,
        env: impl Fn(&str) -> Option<String>,
        port: Option<u16>,
    ) -> Result<Self, ConfigError> {
        let file: File = match text {
            Some(text) => toml::from_str(text)
                .map_err(|error| ConfigError(format!("{}: {error}", path.display())))?,
            None => File::default(),
        };
        let section = file.dashboard;
        let in_file = |key: &str| format!("`{key}` in {}", path.display());
        let mut config = Self::default();

        if let Some(enabled) = section.enabled {
            config.enabled = enabled;
        }
        if let Some(value) = section.port {
            config.port = u16::try_from(value)
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| invalid(&in_file("port"), &value.to_string(), "a port, 1-65535"))?;
        }
        if let Some(value) = section.bind {
            config.bind = parse_bind(&value, &in_file("bind"))?;
        }
        if let Some(value) = section.default_range {
            config.default_range_days = parse_range(&value, &in_file("default_range"))?;
        }
        if let Some(value) = section.theme {
            config.theme = parse_theme(&value, &in_file("theme"))?;
        }

        let from_env = |name: &str| env(name).filter(|value| !value.is_empty());
        if let Some(value) = from_env("TOKENSAVE_DASHBOARD_ENABLED") {
            config.enabled = match value.as_str() {
                "1" | "true" => true,
                "0" | "false" => false,
                _ => {
                    return Err(invalid(
                        "TOKENSAVE_DASHBOARD_ENABLED",
                        &value,
                        "true or false",
                    ))
                }
            };
        }
        if let Some(value) = from_env("TOKENSAVE_DASHBOARD_PORT") {
            config.port = value
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| invalid("TOKENSAVE_DASHBOARD_PORT", &value, "a port, 1-65535"))?;
        }
        if let Some(value) = from_env("TOKENSAVE_DASHBOARD_BIND") {
            config.bind = parse_bind(&value, "TOKENSAVE_DASHBOARD_BIND")?;
        }
        if let Some(value) = from_env("TOKENSAVE_DASHBOARD_DEFAULT_RANGE") {
            config.default_range_days = parse_range(&value, "TOKENSAVE_DASHBOARD_DEFAULT_RANGE")?;
        }
        if let Some(value) = from_env("TOKENSAVE_DASHBOARD_THEME") {
            config.theme = parse_theme(&value, "TOKENSAVE_DASHBOARD_THEME")?;
        }

        if let Some(port) = port {
            if port == 0 {
                return Err(invalid("--port", "0", "a port, 1-65535"));
            }
            config.port = port;
        }
        Ok(config)
    }
}

fn invalid(setting: &str, value: &str, expected: &str) -> ConfigError {
    ConfigError(format!("{setting} is {value:?}; it must be {expected}"))
}

/// A bind address: an IP literal on the loopback interface. Anything else
/// would serve the token data to the network, so it is refused.
fn parse_bind(value: &str, setting: &str) -> Result<IpAddr, ConfigError> {
    let ip: IpAddr = value.parse().map_err(|_| {
        invalid(
            setting,
            value,
            "a loopback IP address, such as 127.0.0.1 or ::1",
        )
    })?;
    if !ip.is_loopback() {
        return Err(ConfigError(format!(
            "{setting} is {value:?}, which is not a loopback address: the dashboard \
             serves only this machine (127.0.0.1 or ::1)"
        )));
    }
    Ok(ip)
}

fn parse_range(value: &str, setting: &str) -> Result<u32, ConfigError> {
    parse_preset(value).ok_or_else(|| invalid(setting, value, "one of 7d, 30d, 45d, 60d, 90d"))
}

fn parse_theme(value: &str, setting: &str) -> Result<Theme, ConfigError> {
    Theme::parse(value).ok_or_else(|| invalid(setting, value, "dark, light or auto"))
}

/// Sets `[dashboard] enabled` (and `port`, when given) in `dashboard.toml`
/// in `home`, keeping the file's other keys. The file is rewritten as TOML
/// from its parsed form, so comments in it are not kept.
pub fn write_enabled(
    home: &Path,
    enabled: bool,
    port: Option<u16>,
) -> Result<PathBuf, ConfigError> {
    let path = path_in(home);
    let mut table: toml::Table = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|error| ConfigError(format!("{}: {error}", path.display())))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(error) => {
            return Err(ConfigError(format!(
                "cannot read {}: {error}",
                path.display()
            )))
        }
    };
    let section = table
        .entry("dashboard")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let Some(section) = section.as_table_mut() else {
        return Err(ConfigError(format!(
            "{}: `dashboard` is not a table",
            path.display()
        )));
    };
    section.insert("enabled".to_string(), toml::Value::Boolean(enabled));
    if let Some(port) = port {
        section.insert("port".to_string(), toml::Value::Integer(i64::from(port)));
    }
    let text = toml::to_string(&table)
        .map_err(|error| ConfigError(format!("{}: {error}", path.display())))?;
    std::fs::create_dir_all(home)
        .and_then(|()| {
            let tmp = home.join(format!("{FILE_NAME}.tmp"));
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, &path)
        })
        .map_err(|error| ConfigError(format!("cannot write {}: {error}", path.display())))?;
    Ok(path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn resolve(
        text: Option<&str>,
        env: &[(&str, &str)],
        port: Option<u16>,
    ) -> Result<DashboardConfig, ConfigError> {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        DashboardConfig::resolve(
            text,
            Path::new("/h/dashboard.toml"),
            |name| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()),
            port,
        )
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let config = resolve(None, &[], None).unwrap();
        assert_eq!(config, DashboardConfig::default());
        assert_eq!(config.port, 7311);
        assert_eq!(config.bind.to_string(), "127.0.0.1");
        assert_eq!(config.default_range_days, 30);
        assert_eq!(config.theme, Theme::Dark);
        assert!(!config.enabled);
    }

    #[test]
    fn the_file_then_the_environment_then_the_flag_decide() {
        let text = "[dashboard]\nenabled = true\nport = 8000\nbind = \"::1\"\ndefault_range = \"7d\"\ntheme = \"light\"\nat_login = true\n";
        let config = resolve(Some(text), &[], None).unwrap();
        assert_eq!(
            (
                config.enabled,
                config.port,
                config.bind.to_string(),
                config.default_range_days,
                config.theme
            ),
            (true, 8000, "::1".to_string(), 7, Theme::Light)
        );
        let config = resolve(
            Some(text),
            &[
                ("TOKENSAVE_DASHBOARD_PORT", "9000"),
                ("TOKENSAVE_DASHBOARD_DEFAULT_RANGE", "90"),
            ],
            None,
        )
        .unwrap();
        assert_eq!((config.port, config.default_range_days), (9000, 90));
        let config = resolve(
            Some(text),
            &[("TOKENSAVE_DASHBOARD_PORT", "9000")],
            Some(9100),
        )
        .unwrap();
        assert_eq!(config.port, 9100);
    }

    #[test]
    fn a_non_loopback_bind_is_refused_wherever_it_comes_from() {
        let error = resolve(Some("[dashboard]\nbind = \"0.0.0.0\"\n"), &[], None).unwrap_err();
        assert!(error.0.contains("`bind` in /h/dashboard.toml"), "{error}");
        assert!(error.0.contains("not a loopback address"), "{error}");
        let error =
            resolve(None, &[("TOKENSAVE_DASHBOARD_BIND", "192.168.1.5")], None).unwrap_err();
        assert!(error.0.contains("TOKENSAVE_DASHBOARD_BIND"), "{error}");
        let error = resolve(None, &[("TOKENSAVE_DASHBOARD_BIND", "localhost")], None).unwrap_err();
        assert!(error.0.contains("loopback IP address"), "{error}");
        assert!(resolve(None, &[("TOKENSAVE_DASHBOARD_BIND", "127.0.0.2")], None).is_ok());
    }

    #[test]
    fn a_wrong_value_names_its_setting() {
        for (text, setting) in [
            ("[dashboard]\nport = 70000\n", "`port`"),
            ("[dashboard]\ndefault_range = \"14d\"\n", "`default_range`"),
            ("[dashboard]\ntheme = \"blue\"\n", "`theme`"),
        ] {
            let error = resolve(Some(text), &[], None).unwrap_err();
            assert!(error.0.contains(setting), "{error}");
        }
        let error = resolve(None, &[("TOKENSAVE_DASHBOARD_THEME", "blue")], None).unwrap_err();
        assert!(error.0.contains("TOKENSAVE_DASHBOARD_THEME"), "{error}");
        assert!(resolve(Some("not toml ["), &[], None).is_err());
    }

    #[test]
    fn presets_are_written_with_or_without_the_day_suffix() {
        assert_eq!(parse_preset("30"), Some(30));
        assert_eq!(parse_preset("45d"), Some(45));
        assert_eq!(parse_preset("14"), None);
        assert_eq!(parse_preset("d"), None);
    }

    #[test]
    fn enabling_keeps_the_other_keys() {
        let home = tempfile::TempDir::new().unwrap();
        std::fs::write(
            path_in(home.path()),
            "[dashboard]\ntheme = \"light\"\n[sources]\ncodex = false\n",
        )
        .unwrap();
        write_enabled(home.path(), true, Some(7400)).unwrap();
        let text = std::fs::read_to_string(path_in(home.path())).unwrap();
        let config = resolve(Some(&text), &[], None).unwrap();
        assert_eq!(
            (config.enabled, config.port, config.theme),
            (true, 7400, Theme::Light)
        );
        assert!(text.contains("codex = false"), "{text}");
        write_enabled(home.path(), false, None).unwrap();
        let text = std::fs::read_to_string(path_in(home.path())).unwrap();
        let config = resolve(Some(&text), &[], None).unwrap();
        assert_eq!((config.enabled, config.port), (false, 7400));
    }
}
