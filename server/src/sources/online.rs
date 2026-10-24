//! The live provider: Open-Meteo panels with NWS alerts and radar status.

use async_trait::async_trait;

use super::nws;
use super::open_meteo;
use super::{AlertSource, Forecast, ForecastSource, RadarStatus, RadarStatusSource};
use crate::config::Config;
use crate::error::Result;
use crate::view::forecast::Alert;

pub struct OnlineProvider {
    client: reqwest::Client,
    config: Config,
}

impl OnlineProvider {
    pub fn new(client: reqwest::Client, config: Config) -> Self {
        OnlineProvider { client, config }
    }
}

#[async_trait]
impl ForecastSource for OnlineProvider {
    async fn forecast(&self) -> Result<Forecast> {
        open_meteo::fetch(&self.client, &self.config, chrono::Utc::now()).await
    }
}

#[async_trait]
impl AlertSource for OnlineProvider {
    async fn alerts(&self) -> Result<Vec<Alert>> {
        nws::alerts(&self.client, &self.config).await
    }
}

#[async_trait]
impl RadarStatusSource for OnlineProvider {
    async fn radar_status(&self) -> Result<RadarStatus> {
        nws::radar_status(&self.client, &self.config).await
    }
}
