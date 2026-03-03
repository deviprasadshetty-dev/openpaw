use anyhow::Result;
use reqwest::blocking::Client;
use std::time::Duration;

pub struct HttpResponse {
    pub status_code: u16,
    pub body: Vec<u8>,
}

pub fn curl_post_with_proxy(
    url: &str,
    body: &str,
    headers: &[String],
    proxy: Option<&str>,
    max_time_secs: Option<u64>,
) -> Result<HttpResponse> {
    let mut builder = Client::builder();

    if let Some(proxy_url) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy_url)?);
    }

    if let Some(secs) = max_time_secs {
        builder = builder.timeout(Duration::from_secs(secs));
    }

    let client = builder.build()?;

    let mut req = client.post(url).body(body.to_string());
    req = req.header("Content-Type", "application/json");

    for header in headers {
        if let Some((k, v)) = header.split_once(':') {
            req = req.header(k.trim(), v.trim());
        }
    }

    let resp = req.send()?;
    let status = resp.status().as_u16();
    let bytes = resp.bytes()?.to_vec();

    Ok(HttpResponse {
        status_code: status,
        body: bytes,
    })
}
