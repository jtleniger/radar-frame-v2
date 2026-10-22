use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Png(png::DecodingError),
    PngEncode(png::EncodingError),
    Json(serde_json::Error),
    Image(String),
    Timezone(String),
    Time(String),
    Svg(String),
    Arguments(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Png(e) => write!(f, "png: {e}"),
            Error::PngEncode(e) => write!(f, "png: {e}"),
            Error::Json(e) => write!(f, "json: {e}"),
            Error::Image(e) => write!(f, "image: {e}"),
            Error::Timezone(e) => write!(f, "timezone: {e}"),
            Error::Time(e) => write!(f, "time: {e}"),
            Error::Svg(e) => write!(f, "svg: {e}"),
            Error::Arguments(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error::Io(value)
    }
}

impl From<png::DecodingError> for Error {
    fn from(value: png::DecodingError) -> Self {
        Error::Png(value)
    }
}

impl From<png::EncodingError> for Error {
    fn from(value: png::EncodingError) -> Self {
        Error::PngEncode(value)
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Error::Json(value)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
