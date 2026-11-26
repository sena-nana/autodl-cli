pub mod common;
pub mod deploy;
pub mod pro;
pub mod types;

use std::time::Duration;

use reqwest::{Client, Method, RequestBuilder, header};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::AppError;
use crate::output::mask_secret;

use self::types::ApiEnvelope;

pub const DEFAULT_API_BASE: &str = "https://api.autodl.com";

#[derive(Debug, Clone)]
pub struct AutoDlClient {
    http: Client,
    base_url: String,
    token: String,
    verbose: bool,
}

impl AutoDlClient {
    pub fn new(token: String, base_url_override: Option<&str>) -> Result<Self, AppError> {
        let base_url = resolve_api_base(base_url_override);
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("autodl-cli/", env!("CARGO_PKG_VERSION")))
            .build()?;

        Ok(Self {
            http,
            base_url,
            token,
            verbose: false,
        })
    }

    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    fn build(&self, method: Method, path: &str) -> RequestBuilder {
        let url = self.url(path);
        if self.verbose {
            eprintln!(
                "[debug] HTTP {} {} (Authorization: {})",
                method,
                url,
                mask_secret(&self.token)
            );
        }
        self.http
            .request(method, url)
            .header(header::AUTHORIZATION, &self.token)
    }

    pub async fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T, AppError> {
        let resp = self.build(Method::POST, path).body("").send().await?;
        self.parse_response(resp).await
    }

    pub async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, AppError> {
        let resp = self.build(Method::POST, path).json(body).send().await?;
        self.parse_response(resp).await
    }

    pub async fn post_json_unit<B: Serialize>(&self, path: &str, body: &B) -> Result<(), AppError> {
        let resp = self.build(Method::POST, path).json(body).send().await?;
        self.parse_unit_response(resp).await
    }

    /// AutoDL Pro endpoints `snapshot` and `status` use `GET` with a JSON request body.
    pub async fn get_with_json_body<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, AppError> {
        let resp = self.build(Method::GET, path).json(body).send().await?;
        self.parse_response(resp).await
    }

    pub async fn get_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T, AppError> {
        let resp = self.build(Method::GET, path).send().await?;
        self.parse_response(resp).await
    }

    pub async fn get_query<Q: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        query: &Q,
    ) -> Result<T, AppError> {
        let resp = self.build(Method::GET, path).query(query).send().await?;
        self.parse_response(resp).await
    }

    pub async fn put_json_unit<B: Serialize>(&self, path: &str, body: &B) -> Result<(), AppError> {
        let resp = self.build(Method::PUT, path).json(body).send().await?;
        self.parse_unit_response(resp).await
    }

    pub async fn delete_json_unit<B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<(), AppError> {
        let resp = self.build(Method::DELETE, path).json(body).send().await?;
        self.parse_unit_response(resp).await
    }

    async fn parse_response<T: DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T, AppError> {
        let status = resp.status();
        let text = resp.text().await?;
        if self.verbose {
            eprintln!(
                "[debug] HTTP status={} body_len={}",
                status.as_u16(),
                text.len()
            );
        }

        let env: ApiEnvelope<T> = serde_json::from_str(&text).map_err(|e| {
            if !status.is_success() {
                AppError::Api {
                    code: format!("HTTP_{}", status.as_u16()),
                    msg: text.clone(),
                    request_id: None,
                    http_status: Some(status.as_u16()),
                }
            } else {
                AppError::Other(format!("Failed to decode AutoDL API response: {e}; raw={text}"))
            }
        })?;

        if env.code != "Success" {
            if self.verbose {
                eprintln!(
                    "[debug] API error code={} msg={} request_id={:?}",
                    env.code, env.msg, env.request_id
                );
            }
            return Err(AppError::Api {
                code: env.code,
                msg: if env.msg.is_empty() {
                    format!("HTTP {}", status.as_u16())
                } else {
                    env.msg
                },
                request_id: env.request_id,
                http_status: Some(status.as_u16()),
            });
        }

        env.data.ok_or_else(|| {
            AppError::Other("AutoDL API returned Success with missing 'data' field".into())
        })
    }

    async fn parse_unit_response(&self, resp: reqwest::Response) -> Result<(), AppError> {
        let status = resp.status();
        let text = resp.text().await?;
        if self.verbose {
            eprintln!(
                "[debug] HTTP status={} body_len={}",
                status.as_u16(),
                text.len()
            );
        }

        let env: ApiEnvelope<serde_json::Value> = serde_json::from_str(&text).map_err(|e| {
            if !status.is_success() {
                AppError::Api {
                    code: format!("HTTP_{}", status.as_u16()),
                    msg: text.clone(),
                    request_id: None,
                    http_status: Some(status.as_u16()),
                }
            } else {
                AppError::Other(format!("Failed to decode AutoDL API response: {e}; raw={text}"))
            }
        })?;

        if env.code != "Success" {
            if self.verbose {
                eprintln!(
                    "[debug] API error code={} msg={} request_id={:?}",
                    env.code, env.msg, env.request_id
                );
            }
            return Err(AppError::Api {
                code: env.code,
                msg: if env.msg.is_empty() {
                    format!("HTTP {}", status.as_u16())
                } else {
                    env.msg
                },
                request_id: env.request_id,
                http_status: Some(status.as_u16()),
            });
        }

        Ok(())
    }
}

pub fn resolve_api_base(override_url: Option<&str>) -> String {
    if let Ok(env_url) = std::env::var("AUTODL_API_BASE") {
        let trimmed = env_url.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(u) = override_url.map(str::trim).filter(|s| !s.is_empty()) {
        return u.to_string();
    }
    DEFAULT_API_BASE.to_string()
}
