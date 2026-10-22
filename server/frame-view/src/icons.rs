use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};

use crate::error::{Error, Result};
use crate::image::Image;

/// The Weather Icons SVGs used by the weather-code mapping, embedded so the
/// binary needs no asset directory at runtime.
fn svg(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "wi-na" => include_bytes!("../assets/icons/wi-na.svg"),
        "wi-stars" => include_bytes!("../assets/icons/wi-stars.svg"),
        "wi-day-sunny" => include_bytes!("../assets/icons/wi-day-sunny.svg"),
        "wi-night-alt-cloudy" => include_bytes!("../assets/icons/wi-night-alt-cloudy.svg"),
        "wi-day-cloudy" => include_bytes!("../assets/icons/wi-day-cloudy.svg"),
        "wi-cloudy" => include_bytes!("../assets/icons/wi-cloudy.svg"),
        "wi-fog" => include_bytes!("../assets/icons/wi-fog.svg"),
        "wi-night-alt-showers" => include_bytes!("../assets/icons/wi-night-alt-showers.svg"),
        "wi-day-showers" => include_bytes!("../assets/icons/wi-day-showers.svg"),
        "wi-night-alt-sleet" => include_bytes!("../assets/icons/wi-night-alt-sleet.svg"),
        "wi-day-sleet" => include_bytes!("../assets/icons/wi-day-sleet.svg"),
        "wi-night-alt-rain" => include_bytes!("../assets/icons/wi-night-alt-rain.svg"),
        "wi-day-rain" => include_bytes!("../assets/icons/wi-day-rain.svg"),
        "wi-night-snow" => include_bytes!("../assets/icons/wi-night-snow.svg"),
        "wi-day-snow" => include_bytes!("../assets/icons/wi-day-snow.svg"),
        "wi-night-alt-thunderstorm" => {
            include_bytes!("../assets/icons/wi-night-alt-thunderstorm.svg")
        }
        "wi-day-thunderstorm" => include_bytes!("../assets/icons/wi-day-thunderstorm.svg"),
        _ => return None,
    })
}

/// Rasterize an icon at `size`x`size`: black ink with a hard alpha edge, which
/// is what the Python view got from CairoSVG plus `quantize(2)`.
pub fn render_icon(name: &str, size: u32) -> Result<Image> {
    let data = svg(name).ok_or_else(|| Error::Svg(format!("unknown icon {name}")))?;
    let tree = Tree::from_data(data, &Options::default())
        .map_err(|e| Error::Svg(format!("{name}: {e}")))?;

    let source = tree.size();
    let mut pixmap = Pixmap::new(size, size)
        .ok_or_else(|| Error::Svg(format!("{name}: cannot allocate {size}x{size}")))?;

    let scale = Transform::from_scale(size as f32 / source.width(), size as f32 / source.height());
    resvg::render(&tree, scale, &mut pixmap.as_mut());

    let mut image = Image::new(size, size, [0, 0, 0, 0]);
    for y in 0..size {
        for x in 0..size {
            let offset = ((y * size + x) * 4) as usize;
            if pixmap.data()[offset + 3] > 0 {
                image.put_pixel(x as i64, y as i64, [0, 0, 0, 255]);
            }
        }
    }

    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_icon_the_codes_use() {
        for code in [0, 2, 3, 45, 51, 57, 61, 66, 71, 77, 96, 123] {
            for night in [false, true] {
                let name = crate::codes::code_to_icon(code, night);
                let image = render_icon(name, 86).unwrap();
                assert_eq!(image.width, 86);
                assert_eq!(image.height, 86);
                let ink = (0..86).any(|y| (0..86).any(|x| image.pixel(x, y)[3] != 0));
                assert!(ink, "{name} rendered empty");
            }
        }
    }
}
