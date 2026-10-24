use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::view::colors::{BLACK, ORANGE, RED, WHITE, YELLOW};
use crate::view::components::{INFO_HEIGHT as HEIGHT, WIDTH};
use crate::view::font::{Anchor, Fonts, Size};
use crate::view::forecast::{Alert, AlertLevel};
use crate::view::image::Image;

fn colors(level: AlertLevel) -> ([u8; 4], [u8; 4]) {
    match level {
        AlertLevel::Info => (WHITE, BLACK),
        AlertLevel::Watch => (BLACK, YELLOW),
        AlertLevel::Warning => (BLACK, ORANGE),
        AlertLevel::Emergency => (WHITE, RED),
    }
}

/// The bar along the bottom of every frame: the local update time, plus a
/// summarised active alert when there is one.
pub fn render(fonts: &Fonts, alerts: &[Alert], now_utc: DateTime<Utc>, timezone: Tz) -> Image {
    let mut image = Image::new(WIDTH, HEIGHT, BLACK);
    let now_local = now_utc.with_timezone(&timezone);
    let width = image.width;
    let height = image.height;

    fonts.draw_text(
        &mut image,
        ((width - 18) as f32, height as f32 / 2.0 - 2.0),
        Size::Small,
        &format!("updated: {}", now_local.format("%-I:%M%p")),
        Anchor::RightMiddle,
        WHITE,
    );

    let mut active: Vec<&Alert> = alerts.iter().filter(|a| a.is_active(now_utc)).collect();
    // Most severe first; the highest `AlertLevel` wins the bar. The sort is
    // stable, so equal levels keep their input order.
    active.sort_by_key(|a| std::cmp::Reverse(a.level));

    let Some(first) = active.first() else {
        return image;
    };

    let (foreground, background) = colors(first.level);
    image.fill_rect(0, 0, (5 * width / 8) as i64, height as i64 - 1, background);

    let mut text = first.event.clone();
    if active.len() > 1 {
        text.push_str(&format!(" and {} others", active.len() - 1));
    }

    fonts.draw_text(
        &mut image,
        (18.0, height as f32 / 2.0 - 2.0),
        Size::Small,
        &text,
        Anchor::LeftMiddle,
        foreground,
    );

    image
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};
    use chrono_tz::America::Chicago;

    use super::*;

    fn alert(event: &str, level: AlertLevel, now: DateTime<Utc>) -> Alert {
        Alert {
            event: event.to_string(),
            level,
            status: "actual".to_string(),
            effective: (now - Duration::minutes(10)).fixed_offset(),
            expires: (now + Duration::minutes(60)).fixed_offset(),
        }
    }

    #[test]
    fn chooses_the_most_severe_active_alert() {
        let now = Utc.with_ymd_and_hms(2025, 6, 1, 12, 34, 56).unwrap();
        let alerts = [
            alert("flash flood watch", AlertLevel::Watch, now),
            alert("tornado emergency", AlertLevel::Emergency, now),
            alert("flood advisory", AlertLevel::Info, now),
            alert("tornado warning", AlertLevel::Warning, now),
        ];

        let image = render(&Fonts::new(), &alerts, now, Chicago);

        // Emergency is the highest level, so its red bar wins regardless of
        // the order the alerts arrive in.
        assert_eq!(image.pixel(0, 0), RED);
    }

    #[test]
    fn ignores_inactive_alerts() {
        let now = Utc.with_ymd_and_hms(2025, 6, 1, 12, 34, 56).unwrap();
        let mut expired = alert("tornado warning", AlertLevel::Warning, now);
        expired.expires = (now - Duration::minutes(1)).fixed_offset();
        let mut pending = alert("tornado watch", AlertLevel::Watch, now);
        pending.effective = (now + Duration::minutes(1)).fixed_offset();
        let mut draft = alert("flood advisory", AlertLevel::Info, now);
        draft.status = "test".to_string();

        let image = render(&Fonts::new(), &[expired, pending, draft], now, Chicago);

        assert_eq!(image.pixel(0, 0), BLACK);
    }
}
