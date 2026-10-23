//! GDAL color table (`gdaldem color-relief`) and nearest-entry lookup.

use std::path::Path;

use crate::render::{Error, Result};

/// The shipped table, embedded so the binary needs no asset file at runtime.
pub const BUILTIN: &str = include_str!("../../assets/gdal-colors.txt");

#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    value: f64,
    rgba: [u8; 4],
}

/// A parsed text color table, equivalent to GDAL's `GDALLoadTextColorMap`.
#[derive(Debug, Clone)]
pub struct ColorTable {
    entries: Vec<Entry>,
}

impl ColorTable {
    /// The embedded `assets/gdal-colors.txt`.
    pub fn builtin() -> Result<Self> {
        Self::parse(BUILTIN)
    }

    /// Load the embedded table or the configured override.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        match path {
            None => Self::builtin(),
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| Error(format!("{}: {e}", path.display())))?;
                Self::parse(&text).map_err(|e| Error(format!("{}: {e}", path.display())))
            }
        }
    }

    /// Parse `gdal-colors.txt`: `value r g b [a]`, `#`/`/` comments.
    ///
    /// Values may be in any order; the table is sorted ascending, as GDAL does.
    pub fn parse(text: &str) -> Result<Self> {
        let mut entries = Vec::new();

        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('/') {
                continue;
            }

            let tokens: Vec<&str> = line
                .split([' ', ',', '\t', ':', '/'])
                .filter(|t| !t.is_empty())
                .collect();
            if tokens.len() < 4 {
                return Err(Error(format!(
                    "color table line {}: expected 'value r g b [a]', got {line:?}",
                    number + 1
                )));
            }

            let parse_num = |token: &str, what: &str| -> Result<f64> {
                token.parse::<f64>().map_err(|_| {
                    Error(format!(
                        "color table line {}: invalid {what} {token:?}",
                        number + 1
                    ))
                })
            };

            let value = parse_num(tokens[0], "value")?;
            // GDAL parses r/g/b with atoi and casts to GByte, i.e. truncates
            // toward zero and keeps the low byte.
            let mut rgba = [0u8; 4];
            for (slot, token) in rgba.iter_mut().zip(&tokens[1..4]) {
                let channel = token.parse::<i32>().map_err(|_| {
                    Error(format!(
                        "color table line {}: invalid channel {token:?}",
                        number + 1
                    ))
                })?;
                *slot = channel as u8;
            }
            rgba[3] = match tokens.get(4) {
                Some(token) => {
                    let alpha = token.parse::<i32>().map_err(|_| {
                        Error(format!(
                            "color table line {}: invalid alpha {token:?}",
                            number + 1
                        ))
                    })?;
                    alpha as u8
                }
                None => 255,
            };

            entries.push(Entry { value, rgba });
        }

        if entries.is_empty() {
            return Err(Error("color table has no entries".to_string()));
        }

        entries.sort_by(|a, b| a.value.total_cmp(&b.value));
        Ok(ColorTable { entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `GDALColorReliefGetRGBA` in `COLOR_SELECTION_NEAREST_ENTRY` mode.
    pub fn lookup(&self, value: u8) -> [u8; 4] {
        let value = f64::from(value);
        let i = self.entries.partition_point(|e| e.value < value);

        if i == 0 {
            return self.entries[0].rgba;
        }
        if i == self.entries.len() {
            return self.entries[self.entries.len() - 1].rgba;
        }

        let next = self.entries[i];
        if next.value == value {
            return next.rgba;
        }
        let prev = self.entries[i - 1];
        if value - prev.value < next.value - value {
            prev.rgba
        } else {
            next.rgba
        }
    }

    /// All 256 lookups, so the renderer resolves the palette once.
    pub fn lut(&self) -> [[u8; 4]; 256] {
        let mut lut = [[0u8; 4]; 256];
        for (value, slot) in lut.iter_mut().enumerate() {
            *slot = self.lookup(value as u8);
        }
        lut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_table() -> ColorTable {
        ColorTable::builtin().expect("parse embedded gdal-colors.txt")
    }

    #[test]
    fn parses_the_embedded_table() {
        let table = repo_table();
        assert_eq!(table.len(), 71);

        assert_eq!(table.lookup(0), [0, 0, 0, 0]);
        assert_eq!(table.lookup(10), [23, 33, 58, 255]);
        assert_eq!(table.lookup(11), [29, 45, 71, 255]);
        assert_eq!(table.lookup(41), [234, 121, 36, 255]);
        assert_eq!(table.lookup(79), [46, 11, 119, 255]);
        assert_eq!(table.lookup(80), [46, 11, 119, 255]);
        assert_eq!(table.lookup(255), [46, 11, 119, 255]);
    }

    #[test]
    fn nearest_entry_ties_go_to_the_higher_value() {
        let table = ColorTable::parse("0 0 0 0 0\n10 1 2 3 255\n20 4 5 6 255\n").unwrap();

        assert_eq!(table.lookup(4), [0, 0, 0, 0]);
        assert_eq!(table.lookup(5), [1, 2, 3, 255]);
        assert_eq!(table.lookup(14), [1, 2, 3, 255]);
        assert_eq!(table.lookup(15), [4, 5, 6, 255]);
        assert_eq!(table.lookup(21), [4, 5, 6, 255]);
    }

    #[test]
    fn alpha_defaults_to_opaque_and_explicit_alpha_is_used() {
        let table = ColorTable::parse("10 1 2 3\n20 4 5 6 7\n").unwrap();
        assert_eq!(table.lookup(10), [1, 2, 3, 255]);
        assert_eq!(table.lookup(20), [4, 5, 6, 7]);
    }

    #[test]
    fn comments_blank_lines_and_separators_are_ignored() {
        let table =
            ColorTable::parse("# comment\n/ another\n\n10,1,2,3,4\n20\t5\t6\t7\t8\n").unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(table.lookup(10), [1, 2, 3, 4]);
        assert_eq!(table.lookup(20), [5, 6, 7, 8]);
    }

    #[test]
    fn empty_table_is_an_error() {
        assert!(ColorTable::parse("").is_err());
        assert!(ColorTable::parse("# nothing\n").is_err());
    }

    #[test]
    fn lut_matches_lookup() {
        let table = repo_table();
        let lut = table.lut();
        for value in 0..=255u8 {
            assert_eq!(
                lut[usize::from(value)],
                table.lookup(value),
                "value {value}"
            );
        }
    }
}
