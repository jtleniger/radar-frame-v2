//! The live provider: Open-Meteo panels with NWS alerts and radar status.

use async_trait::async_trait;

use super::nws;
use super::open_meteo;
use super::{Forecast, ForecastProvider, RadarStatus};
use crate::config::Config;
use crate::error::Result;
use frame_view::forecast::Alert;

pub struct OnlineProvider {
    client: reqwest::Client,
    config: Config,
    forecast: tokio::sync::OnceCell<Forecast>,
}

impl OnlineProvider {
    pub fn new(client: reqwest::Client, config: Config) -> Self {
        OnlineProvider {
            client,
            config,
            // Filled once per provider instance, which is once per update cycle
            // in `api::frame`; this is why the three panels share one request.
            forecast: tokio::sync::OnceCell::new(),
        }
    }
}

#[async_trait]
impl ForecastProvider for OnlineProvider {
    async fn forecast(&self) -> Result<&Forecast> {
        self.forecast
            .get_or_try_init(|| async {
                open_meteo::fetch(&self.client, &self.config, chrono::Utc::now()).await
            })
            .await
    }

    async fn alerts(&self) -> Result<Vec<Alert>> {
        nws::alerts(&self.client, &self.config).await
    }

    async fn radar_status(&self) -> Result<RadarStatus> {
        nws::radar_status(&self.client, &self.config).await
    }
}
