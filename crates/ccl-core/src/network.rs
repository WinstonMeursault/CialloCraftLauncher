use serde::de::DeserializeOwned;
use std::time::Duration;

use crate::{Error, Result, config::DownloadConfig};

/// Shared HTTP transport, independent of any user interface or download source.
#[derive(Clone)]
pub struct HttpClient(reqwest::Client, bool);

impl HttpClient {
    pub fn new(settings: &DownloadConfig) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .user_agent(concat!("CialloCraftLauncher/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(30));
        if let Some(proxy) = &settings.http_proxy {
            let proxy = reqwest::Proxy::all(proxy).map_err(|_| {
                Error::InvalidConfig(
                    "downloads.http_proxy is invalid; check its scheme and address".into(),
                )
            })?;
            builder = builder.proxy(proxy);
        }
        Ok(Self(
            builder
                .build()
                .map_err(|error| Error::Network(error.without_url()))?,
            settings.offline,
        ))
    }

    /// Fetch and deserialize a JSON resource. Callers own source selection and retry policy.
    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        if self.1 {
            return Err(Error::InvalidConfig(format!(
                "offline mode: metadata is not cached ({})",
                safe_url(url)
            )));
        }
        self.0
            .get(url)
            .send()
            .await
            .map_err(|error| Error::Network(error.without_url()))?
            .error_for_status()
            .map_err(|error| Error::Network(error.without_url()))?
            .json()
            .await
            .map_err(|error| Error::Network(error.without_url()))
    }
}

fn safe_url(raw: &str) -> String {
    reqwest::Url::parse(raw)
        .map(|mut u| {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        })
        .unwrap_or_else(|_| "<invalid URL>".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn offline_metadata_request_is_rejected_before_http() {
        let client = HttpClient::new(&DownloadConfig {
            offline: true,
            ..DownloadConfig::default()
        })
        .unwrap();
        let result = client
            .get_json::<serde_json::Value>("http://127.0.0.1:1/private?token=secret")
            .await;
        let error = result.unwrap_err().to_string();
        assert!(error.contains("offline mode"));
        assert!(!error.contains("secret"));
    }

    #[test]
    fn diagnostic_url_redacts_credentials_and_query() {
        let url = safe_url("https://user:password@example.org/resource?token=secret");
        assert_eq!(url, "https://example.org/resource");
    }
}
