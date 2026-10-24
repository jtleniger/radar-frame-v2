//! Configuration loaded from the same INI file the Python client used.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use ini::Ini;
use thiserror::Error;
use url::Url;

const DEFAULT_LOG_FILE: &str = "radar-frame.log";
const DEFAULT_LOG_LEVEL: &str = "INFO";
const DEFAULT_POLL_SECONDS: u64 = 120;
const DEFAULT_CONNECT_TIMEOUT_SECONDS: u64 = 5;
const DEFAULT_REQUEST_TIMEOUT_SECONDS: u64 = 30;

#[derive(Clone, Debug)]
pub struct Config {
    pub logging: LoggingConfig,
    pub server: ServerConfig,
    pub epaper: EpaperConfig,
}

#[derive(Clone, Debug)]
pub struct LoggingConfig {
    pub file: PathBuf,
    pub level: String,
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub base_url: Url,
    pub poll_interval: Duration,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct EpaperConfig {
    pub use_noop: bool,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {message}")]
    Read { path: PathBuf, message: String },
    #[error("{path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let ini = Ini::load_from_file(path).map_err(|error| ConfigError::Read {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        Self::from_ini(&ini, path)
    }

    fn from_ini(ini: &Ini, path: &Path) -> Result<Self, ConfigError> {
        let invalid = |message: String| ConfigError::Invalid {
            path: path.to_path_buf(),
            message,
        };

        let base_url = required(ini, "server", "base_url", path)?;
        let mut base_url =
            Url::parse(base_url).map_err(|error| invalid(format!("[server] base_url: {error}")))?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(invalid(format!(
                "[server] base_url must use http or https, not {}",
                base_url.scheme()
            )));
        }
        if base_url.host_str().is_none() {
            return Err(invalid("[server] base_url has no host".to_string()));
        }
        if base_url.query().is_some() || base_url.fragment().is_some() {
            return Err(invalid(
                "[server] base_url must not have a query or fragment".to_string(),
            ));
        }
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }

        let poll_seconds =
            number(ini, "server", "poll_seconds", path)?.unwrap_or(DEFAULT_POLL_SECONDS);
        if poll_seconds == 0 {
            return Err(invalid(
                "[server] poll_seconds must be positive".to_string(),
            ));
        }

        let connect_seconds = number(ini, "server", "connect_timeout_seconds", path)?
            .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECONDS);
        let request_seconds = number(ini, "server", "request_timeout_seconds", path)?
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECONDS);
        if connect_seconds == 0 || request_seconds == 0 {
            return Err(invalid(
                "[server] timeouts must be positive seconds".to_string(),
            ));
        }

        let log_file = optional(ini, "logging", "file")
            .unwrap_or(DEFAULT_LOG_FILE)
            .to_owned();
        let log_level = optional(ini, "logging", "level")
            .unwrap_or(DEFAULT_LOG_LEVEL)
            .to_owned();
        tracing::Level::from_str(&log_level)
            .map_err(|_| invalid(format!("[logging] level {log_level:?} is not valid")))?;

        let use_noop = optional(ini, "epaper", "use_noop")
            .map(parse_bool)
            .transpose()
            .map_err(|value| {
                invalid(format!(
                    "[epaper] use_noop {value:?} is not a boolean (true/false, yes/no, on/off, 1/0)"
                ))
            })?
            .unwrap_or(false);

        Ok(Config {
            logging: LoggingConfig {
                file: PathBuf::from(log_file),
                level: log_level,
            },
            server: ServerConfig {
                base_url,
                poll_interval: Duration::from_secs(poll_seconds),
                connect_timeout: Duration::from_secs(connect_seconds),
                request_timeout: Duration::from_secs(request_seconds),
            },
            epaper: EpaperConfig { use_noop },
        })
    }
}

fn optional<'a>(ini: &'a Ini, section: &str, key: &str) -> Option<&'a str> {
    ini.section(Some(section))
        .and_then(|properties| properties.get(key))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn required<'a>(
    ini: &'a Ini,
    section: &str,
    key: &str,
    path: &Path,
) -> Result<&'a str, ConfigError> {
    optional(ini, section, key).ok_or_else(|| ConfigError::Invalid {
        path: path.to_path_buf(),
        message: format!("missing [{section}] {key}"),
    })
}

fn number(ini: &Ini, section: &str, key: &str, path: &Path) -> Result<Option<u64>, ConfigError> {
    let Some(value) = optional(ini, section, key) else {
        return Ok(None);
    };
    value
        .parse::<u64>()
        .map(Some)
        .map_err(|_| ConfigError::Invalid {
            path: path.to_path_buf(),
            message: format!("[{section}] {key} must be a non-negative integer, got {value:?}"),
        })
}

fn parse_bool(value: &str) -> Result<bool, &str> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Result<Config, ConfigError> {
        let ini = Ini::read_from(&mut source.as_bytes()).expect("valid INI");
        Config::from_ini(&ini, Path::new("config.ini"))
    }

    #[test]
    fn keeps_the_python_ini_shape_and_defaults() {
        let config = parse(
            "\
[logging]
file = radar-frame.log
level = INFO

[server]
base_url = http://radar.local:8000

[epaper]
use_noop = yes
",
        )
        .unwrap();

        assert_eq!(config.server.base_url.as_str(), "http://radar.local:8000/");
        assert_eq!(config.server.poll_interval, Duration::from_secs(120));
        assert_eq!(config.server.connect_timeout, Duration::from_secs(5));
        assert_eq!(config.server.request_timeout, Duration::from_secs(30));
        assert!(config.epaper.use_noop);
    }

    #[test]
    fn rejects_a_missing_base_url() {
        let error = parse("[server]\n").unwrap_err();
        assert!(error.to_string().contains("missing [server] base_url"));
    }

    #[test]
    fn accepts_python_style_booleans() {
        for value in ["true", "YES", "on", "1"] {
            let config = parse(&format!(
                "[server]\nbase_url = http://example.com/\n[epaper]\nuse_noop = {value}\n"
            ))
            .unwrap();
            assert!(config.epaper.use_noop, "{value}");
        }
        assert!(
            parse("[server]\nbase_url = http://example.com/\n[epaper]\nuse_noop = maybe\n")
                .is_err()
        );
    }
}
