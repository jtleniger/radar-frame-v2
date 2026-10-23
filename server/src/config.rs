//! The INI settings file, read once at startup.
//!
//! This is the subset of `configparser` that `config/config.ini` uses:
//! `[section]` headers, `key = value` (or `key: value`) options, blank lines,
//! and `#` / `;` comment lines. Option names are case-insensitive, as in
//! Python's `ConfigParser`.

use std::collections::HashMap;
use std::path::Path;

use chrono_tz::Tz;

use crate::error::{Error, Result};

/// `User-Agent` sent when the settings file has no `[nws] user_agent`. The
/// example settings file provides a contact placeholder; the NWS API asks for
/// one.
pub const DEFAULT_USER_AGENT: &str = "radar-frame";

#[derive(Clone, Debug, Default)]
pub struct Config {
    sections: HashMap<String, HashMap<String, String>>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
        Self::parse(&text).map_err(|e| Error(format!("{}: {e}", path.display())))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut current: Option<String> = None;

        for (number, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            if let Some(name) = line.strip_prefix('[') {
                let name = name
                    .strip_suffix(']')
                    .ok_or_else(|| Error(format!("line {}: unterminated section", number + 1)))?;
                let name = name.trim().to_string();
                sections.entry(name.clone()).or_default();
                current = Some(name);
                continue;
            }

            let separator = line
                .find('=')
                .or_else(|| line.find(':'))
                .ok_or_else(|| Error(format!("line {}: expected key = value", number + 1)))?;
            let key = line[..separator].trim().to_lowercase();
            let value = line[separator + 1..].trim().to_string();

            if key.is_empty() {
                return Err(Error(format!("line {}: empty option name", number + 1)));
            }

            let section = current
                .as_ref()
                .ok_or_else(|| Error(format!("line {}: option outside a section", number + 1)))?;
            sections
                .entry(section.clone())
                .or_default()
                .insert(key, value);
        }

        Ok(Config { sections })
    }

    pub fn get(&self, section: &str, key: &str) -> Result<&str> {
        self.get_opt(section, key)
            .ok_or_else(|| Error(format!("config: missing [{section}] {key}")))
    }

    pub fn get_opt(&self, section: &str, key: &str) -> Option<&str> {
        self.sections
            .get(section)
            .and_then(|values| values.get(&key.to_lowercase()))
            .map(String::as_str)
    }

    pub fn f64(&self, section: &str, key: &str) -> Result<f64> {
        let value = self.get(section, key)?;
        value.parse().map_err(|_| {
            Error(format!(
                "config: [{section}] {key} is not a number: {value}"
            ))
        })
    }

    pub fn timezone(&self, section: &str, key: &str) -> Result<Tz> {
        let value = self.get(section, key)?;
        value.parse().map_err(|_| {
            Error(format!(
                "config: [{section}] {key} is not a time zone: {value}"
            ))
        })
    }

    pub fn user_agent(&self) -> &str {
        self.get_opt("nws", "user_agent")
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_USER_AGENT)
    }

    /// Optional `[render] colors` override; the embedded table is the default.
    pub fn colors(&self) -> Option<std::path::PathBuf> {
        self.get_opt("render", "colors")
            .map(std::path::PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
[logging]
level = INFO

[radar]
nexrad_id = KTLX

[bbox]
min_lat = 35.35
max_lat = 35.39
min_lon = -97.26
max_lon = -97.21

[forecast]
timezone = America/Chicago

[nws]
user_agent = (someone@example.com)
";

    #[test]
    fn reads_sections_and_values() {
        let config = Config::parse(SAMPLE).unwrap();
        assert_eq!(config.get("radar", "nexrad_id").unwrap(), "KTLX");
        assert_eq!(config.f64("bbox", "min_lat").unwrap(), 35.35);
        assert_eq!(
            config.get("nws", "USER_AGENT").unwrap(),
            "(someone@example.com)"
        );
        assert_eq!(
            config.timezone("forecast", "timezone").unwrap(),
            chrono_tz::America::Chicago
        );
    }

    #[test]
    fn keeps_parentheses_and_hashes_inside_values() {
        let config =
            Config::parse("[nws]\nuser_agent = radar-frame (jt@example.com) # contact\n").unwrap();
        assert_eq!(
            config.get("nws", "user_agent").unwrap(),
            "radar-frame (jt@example.com) # contact"
        );
    }

    #[test]
    fn missing_setting_names_the_section() {
        let config = Config::parse(SAMPLE).unwrap();
        let error = config.get("bbox", "missing").unwrap_err().to_string();
        assert!(error.contains("[bbox] missing"), "{error}");
    }

    #[test]
    fn user_agent_defaults_and_can_be_overridden() {
        let default = Config::parse("[radar]\nnexrad_id = KTLX\n").unwrap();
        assert_eq!(default.user_agent(), "radar-frame");

        let configured = Config::parse("[nws]\nuser_agent = (contact@example.com)\n").unwrap();
        assert_eq!(configured.user_agent(), "(contact@example.com)");
    }

    #[test]
    fn render_colors_is_optional() {
        let configured = Config::parse("[render]\ncolors = /tmp/table.txt\n").unwrap();
        assert_eq!(
            configured.colors(),
            Some(std::path::PathBuf::from("/tmp/table.txt"))
        );

        let default = Config::parse(SAMPLE).unwrap();
        assert_eq!(default.colors(), None);
    }
}
