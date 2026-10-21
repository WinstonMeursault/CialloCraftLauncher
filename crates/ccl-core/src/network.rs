use serde::de::DeserializeOwned;
use std::time::Duration;

use crate::{Result, config::DownloadConfig};

/// Shared HTTP transport, independent of any user interface or download source.
#[derive(Clone)]
pub struct HttpClient(reqwest::Client);

impl HttpClient {
    pub fn new(settings: &DownloadConfig) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .user_agent(concat!("CialloCraftLauncher/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(30));
        if let Some(proxy) = &settings.http_proxy {
            builder = builder.proxy(reqwest::Proxy::all(proxy)?);
        }
        Ok(Self(builder.build()?))
    }

    /// Fetch and deserialize a JSON resource. Callers own source selection and retry policy.
    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        Ok(self
            .0
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}
