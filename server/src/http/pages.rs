//! Askama pages and the fields they share with the status document.

use askama::Template;
use axum::response::{Html, IntoResponse, Response};
use chrono_tz::Tz;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::state::State;

pub struct Fields {
    pub mode: &'static str,
    pub mode_select: &'static str,
    pub last_updated: String,
    pub manual_til: String,
    pub has_manual_til: bool,
}

pub fn fields(state: &State, timezone: Tz) -> Fields {
    let manual_til = state.manual_til_str(timezone);
    Fields {
        mode: state.mode.name(),
        mode_select: state.mode_select.name(),
        // Jinja rendered `None` for the initial timestamp; keep the text.
        last_updated: state
            .last_updated_str(timezone)
            .unwrap_or_else(|| "None".to_string()),
        has_manual_til: manual_til.is_some(),
        manual_til: manual_til.unwrap_or_default(),
    }
}

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexPage {
    pub mode: &'static str,
    pub mode_select: &'static str,
    pub last_updated: String,
    pub manual_til: String,
    pub has_manual_til: bool,
}

#[derive(Template)]
#[template(path = "partials/settings.html")]
pub struct SettingsPage {
    pub mode: &'static str,
    pub mode_select: &'static str,
    pub manual_til: String,
    pub has_manual_til: bool,
}

pub fn html<T: Template>(page: T) -> Result<Response> {
    match page.render() {
        Ok(body) => Ok(Html(body).into_response()),
        Err(error) => {
            tracing::error!("template: {error}");
            Err(Error::from(error))
        }
    }
}

#[derive(Deserialize)]
pub struct SettingsForm {
    #[serde(rename = "mode-select")]
    pub mode_select: String,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Deserialize)]
pub struct FrameQuery {
    pub debug: Option<String>,
}
