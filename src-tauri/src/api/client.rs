use anyhow::{Context, Result};
use reqwest::Client;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) fn chatgpt_client() -> Result<Client> {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("Failed to build ChatGPT HTTP client")
}
