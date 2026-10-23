/// Text shown under the current conditions icon. Matches the Open-Meteo WMO
/// weather-code mapping in the Python view.
pub fn code_to_string(code: i32) -> String {
    match code {
        0 | 1 => "clear".to_string(),
        2 => "cloudy".to_string(),
        3 => "overcast".to_string(),
        41..=49 => "fog".to_string(),
        51 | 53 | 55 => "drizzle".to_string(),
        56 | 57 => "sleet".to_string(),
        61 | 80 | 63 | 81 | 65 | 82 | 66 | 67 => "rain".to_string(),
        71 | 85 | 73 | 75 | 86 => "snow".to_string(),
        77 => "graupel".to_string(),
        95..=99 => "tstorm".to_string(),
        other => other.to_string(),
    }
}

/// Weather Icons name for the code, day or night.
pub fn code_to_icon(code: i32, night: bool) -> &'static str {
    match code {
        0 | 1 if night => "wi-stars",
        0 | 1 => "wi-day-sunny",
        2 if night => "wi-night-alt-cloudy",
        2 => "wi-day-cloudy",
        3 => "wi-cloudy",
        41..=49 => "wi-fog",
        51 | 53 | 55 if night => "wi-night-alt-showers",
        51 | 53 | 55 => "wi-day-showers",
        56 | 57 | 66 | 67 | 77 if night => "wi-night-alt-sleet",
        56 | 57 | 66 | 67 | 77 => "wi-day-sleet",
        61 | 80 | 63 | 81 | 65 | 82 if night => "wi-night-alt-rain",
        61 | 80 | 63 | 81 | 65 | 82 => "wi-day-rain",
        71 | 85 | 73 | 75 | 86 if night => "wi-night-snow",
        71 | 85 | 73 | 75 | 86 => "wi-day-snow",
        95..=99 if night => "wi-night-alt-thunderstorm",
        95..=99 => "wi-day-thunderstorm",
        _ => "wi-na",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_codes_like_the_python_view() {
        assert_eq!(code_to_string(0), "clear");
        assert_eq!(code_to_string(45), "fog");
        assert_eq!(code_to_string(53), "drizzle");
        assert_eq!(code_to_string(81), "rain");
        assert_eq!(code_to_string(85), "snow");
        assert_eq!(code_to_string(77), "graupel");
        assert_eq!(code_to_string(96), "tstorm");
        assert_eq!(code_to_string(50), "50");
        assert_eq!(code_to_string(100), "100");

        assert_eq!(code_to_icon(1, false), "wi-day-sunny");
        assert_eq!(code_to_icon(1, true), "wi-stars");
        assert_eq!(code_to_icon(2, true), "wi-night-alt-cloudy");
        assert_eq!(code_to_icon(3, true), "wi-cloudy");
        assert_eq!(code_to_icon(66, true), "wi-night-alt-sleet");
        assert_eq!(code_to_icon(80, false), "wi-day-rain");
        assert_eq!(code_to_icon(85, true), "wi-night-snow");
        assert_eq!(code_to_icon(99, false), "wi-day-thunderstorm");
        assert_eq!(code_to_icon(100, false), "wi-na");
        assert_eq!(code_to_icon(20, false), "wi-na");
    }
}
