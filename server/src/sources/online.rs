//! The live provider: Open-Meteo panels with NWS alerts and radar status.

use async_trait::async_trait;

use super::nws;
use super::open_meteo;
use super::{Forecast, ForecastProvider, RadarStatus};
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
impl ForecastProvider for OnlineProvider {
    async fn forecast(&self) -> Result<Forecast> {
        open_meteo::fetch(&self.client, &self.config, chrono::Utc::now()).await
    }

    async fn alerts(&self) -> Result<Vec<Alert>> {
        nws::alerts(&self.client, &self.config).await
    }

    async fn radar_status(&self) -> Result<RadarStatus> {
        nws::radar_status(&self.client, &self.config).await
    }
}
