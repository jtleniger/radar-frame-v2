//! Elevation argument parsing for `radar-frame render archive`.

/// Parse `N` or `N-M` (inclusive) into the elevation numbers it selects.
///
/// A range must be ascending; a single elevation is returned as a one-element
/// vector. `-e` is parsed once, by `render archive`.
pub fn parse_elevations(value: &str) -> Option<Vec<u8>> {
    fn valid(part: &str) -> bool {
        !part.is_empty() && part.len() <= 2 && part.bytes().all(|byte| byte.is_ascii_digit())
    }

    let parts: Vec<&str> = value.split('-').collect();
    match parts.as_slice() {
        [single] if valid(single) => Some(vec![single.parse().ok()?]),
        [start, stop] if valid(start) && valid(stop) => {
            let start: u8 = start.parse().ok()?;
            let stop: u8 = stop.parse().ok()?;
            if start >= stop {
                return None;
            }
            Some((start..=stop).collect())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_elevations() {
        assert_eq!(parse_elevations("1"), Some(vec![1]));
        assert_eq!(parse_elevations("1-3"), Some(vec![1, 2, 3]));
        assert_eq!(parse_elevations("9-10"), Some(vec![9, 10]));
        assert_eq!(parse_elevations("3-3"), None);
        assert_eq!(parse_elevations("3-1"), None);
        assert_eq!(parse_elevations("1-"), None);
        assert_eq!(parse_elevations(""), None);
        assert_eq!(parse_elevations("100"), None);
        assert_eq!(parse_elevations("1-2-3"), None);
        assert_eq!(parse_elevations("a"), None);
    }
}
