//! HTTP 传输层：`HttpTransport` 抽象（seam）+ `ReqwestTransport` 生产实现。
//!
//! 设计要点：
//! - 复用单个 `reqwest::Client`（连接池 / keep-alive），差异化请求头按请求携带；
//! - 统一 rustls（纯 Rust），`.http1_only()` + 连接/请求超时；默认 `.no_proxy()`，
//!   仅在显式设置 `SLIVE_PROXY_URL` 时启用该代理（Clash/TUN 的 fake-IP 环境需要）；
//! - **不对业务错误码自动重试**（AC-25）；传输层亦不重试（保持与 Dart core 同路径的请求数）。

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::SliveError;

/// 默认 User-Agent（对齐 Dart `BiliBiliSite.kDefaultUserAgent`）。
pub const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 Edg/126.0.0.0";
/// 默认 Referer（对齐 Dart `BiliBiliSite.kDefaultReferer`）。
pub const DEFAULT_REFERER: &str = "https://live.bilibili.com/";
/// 取流 headers 的 Referer（对齐 Dart `getPlayUrls`）。
pub const PLAY_REFERER: &str = "https://live.bilibili.com";
/// 取流 headers 的 User-Agent（对齐 Dart `getPlayUrls`）。
pub const PLAY_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36 Edg/115.0.1901.188";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const ERROR_BODY_CAP: usize = 512;

/// 一次 GET 请求描述（URL + query + 差异头）。MockTransport 依此记录与匹配。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// 不含 query 的基准 URL。
    pub url: String,
    /// query 参数（顺序保留）。
    pub query: Vec<(String, String)>,
    /// 请求头（顺序保留）。
    pub headers: Vec<(String, String)>,
}

/// 一次 GET 响应。
#[derive(Debug, Clone)]
pub struct HttpReply {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应体文本。
    pub text: String,
    /// `set-cookie` 响应头（QR 轮询汇总 credential 需要）。
    pub set_cookie: Vec<String>,
}

impl HttpReply {
    /// 解析响应体为 JSON（非法 JSON -> `SliveError::Decode`）。
    pub fn json(&self) -> Result<Value, SliveError> {
        serde_json::from_str(&self.text)
            .map_err(|e| SliveError::decode(format!("解析 JSON 失败: {e}")))
    }
}

/// 校验服务端业务错误码：存在且 `code != 0` -> `SliveError::Api`（AC-25，不静默返回空）。
pub fn check_api_code(value: &Value) -> Result<(), SliveError> {
    let Some(code) = value.get("code").and_then(Value::as_i64) else {
        return Ok(());
    };
    if code == 0 {
        return Ok(());
    }
    let message = value
        .get("message")
        .map(crate::util::value_to_string)
        .unwrap_or_default();
    Err(SliveError::Api {
        code: code as i32,
        message,
    })
}

/// HTTP 传输抽象（seam）。
#[async_trait]
pub trait HttpTransport: Send + Sync {
    /// 发送一次 GET 请求。
    async fn get(&self, request: &HttpRequest) -> Result<HttpReply, SliveError>;

    /// 发送一次 POST 请求。
    ///
    /// 默认实现用独立 `reqwest::Client`；测试的 mock 传输覆盖它，
    /// 这样平台里的 POST（GQL / startPlay / InnerTube）也能被夹具接管。
    async fn post(
        &self,
        request: &HttpRequest,
        body: &str,
        content_type: &str,
    ) -> Result<HttpReply, SliveError> {
        let client = build_client();
        let mut builder = client.post(&request.url).header("content-type", content_type);
        for (key, value) in &request.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }
        let response = builder
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| SliveError::network(e.to_string()))?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| SliveError::network(format!("读取响应体失败: {e}")))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if !(200..400).contains(&status) {
            return Err(SliveError::Http {
                status,
                body: truncate(&text),
            });
        }
        Ok(HttpReply {
            status,
            text,
            set_cookie: Vec::new(),
        })
    }
}

/// 生产实现：`reqwest` + rustls，复用单个 Client。
pub struct ReqwestTransport {
    client: reqwest::Client,
}

/// 构造共享 `reqwest::Client`。
pub fn build_client() -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .http1_only()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT);
    match std::env::var("SLIVE_PROXY_URL") {
        Ok(raw) if !raw.trim().is_empty() => match reqwest::Proxy::all(raw.trim()) {
            Ok(proxy) => builder = builder.proxy(proxy),
            Err(e) => log::warn!("忽略非法 SLIVE_PROXY_URL: {e}"),
        },
        _ => builder = builder.no_proxy(),
    }
    builder.build().unwrap_or_default()
}

impl ReqwestTransport {
    /// 生产构造。
    pub fn new() -> Self {
        Self {
            client: build_client(),
        }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn get(&self, request: &HttpRequest) -> Result<HttpReply, SliveError> {
        let mut url = reqwest::Url::parse(&request.url)
            .map_err(|e| SliveError::invalid_input(format!("非法 URL({}): {e}", request.url)))?;
        if !request.query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in &request.query {
                pairs.append_pair(key, value);
            }
        }

        let mut builder = self.client.get(url);
        for (key, value) in &request.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }

        let response = builder
            .send()
            .await
            .map_err(|e| SliveError::network(e.to_string()))?;

        let status = response.status().as_u16();
        let set_cookie = response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(str::to_string)
            .collect();
        // 页面与接口均为 UTF-8；`text()` 在响应未声明 charset 时会按 Latin-1
        // 解码（快手房间页即如此），故显式按字节转 UTF-8。
        let bytes = response
            .bytes()
            .await
            .map_err(|e| SliveError::network(format!("读取响应体失败: {e}")))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();

        if !(200..400).contains(&status) {
            // 允许 301/302 重定向（某些平台会返回 redirect）
            if status == 301 || status == 302 {
                return Err(SliveError::Api { 
                    code: status as i32, 
                    message: format!("HTTP 重定向 ({})", status) 
                });
            }
            return Err(SliveError::Http {
                status,
                body: truncate(&text),
            });
        }
        Ok(HttpReply {
            status,
            text,
            set_cookie,
        })
    }
}

// ============================================================================
// HTTP Transport Extension (供各平台复用)
// ============================================================================

#[async_trait::async_trait]
pub trait HttpTransportExt: HttpTransport + Send + Sync {
    /// GET URL 并返回文本（带 cookie）。
    async fn get_with_cookie(&self, url: &str, cookie: &str) -> Result<String, SliveError> {
        self.get_with_headers(
            url,
            &[
                ("cookie".to_string(), cookie.to_string()),
                ("referer".to_string(), "https://www.acfun.cn/".to_string()),
            ],
        )
        .await
    }

    /// GET URL 并返回文本（调用方头覆盖默认头，同名以调用方为准）。
    async fn get_with_headers(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<String, SliveError> {
        let mut all = vec![("user-agent".to_string(), DEFAULT_USER_AGENT.to_string())];
        for (key, value) in headers {
            if let Some(existing) = all
                .iter_mut()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
            {
                existing.1 = value.clone();
            } else {
                all.push((key.clone(), value.clone()));
            }
        }
        let request = HttpRequest {
            url: url.to_string(),
            query: Vec::new(),
            headers: all,
        };
        let reply = self.get(&request).await?;
        Ok(reply.text)
    }

    /// GET URL 并解析 JSON。
    async fn get_json(&self, url: &str) -> Result<Value, SliveError> {
        let request = HttpRequest {
            url: url.to_string(),
            query: vec![],
            headers: vec![
                ("user-agent".to_string(), DEFAULT_USER_AGENT.to_string()),
                ("accept-language".to_string(), "zh-CN,zh;q=0.9,en;q=0.8".to_string()),
                ("referer".to_string(), "https://www.acfun.cn/".to_string()),
            ],
        };
        let reply = self.get(&request).await?;
        reply.json()
    }

    /// GET URL 并返回文本。
    async fn get_text(&self, url: &str) -> Result<String, SliveError> {
        let request = HttpRequest {
            url: url.to_string(),
            query: vec![],
            headers: vec![
                ("user-agent".to_string(), DEFAULT_USER_AGENT.to_string()),
            ],
        };
        let reply = self.get(&request).await?;
        Ok(reply.text)
    }

    /// POST form data（form-urlencoded）。
    async fn post_form(&self, url: &str, body: &str) -> Result<String, SliveError> {
        self.post_form_with_headers(url, body, &[]).await
    }

    /// POST form data with cookie header。
    async fn post_form_with_cookie(
        &self,
        url: &str,
        body: &str,
        cookie: &str,
    ) -> Result<String, SliveError> {
        self.post_form_with_headers(url, body, &[("cookie", cookie.to_string())])
            .await
    }

    /// POST form data with custom headers。
    async fn post_form_with_headers(
        &self,
        url: &str,
        body: &str,
        headers: &[(&str, String)],
    ) -> Result<String, SliveError> {
        let request = HttpRequest {
            url: url.to_string(),
            query: Vec::new(),
            headers: post_headers(headers),
        };
        let reply = self
            .post(&request, body, "application/x-www-form-urlencoded")
            .await?;
        Ok(reply.text)
    }

    /// POST JSON 并返回响应文本。
    async fn post_json(&self, url: &str, json_body: &str) -> Result<String, SliveError> {
        let request = HttpRequest {
            url: url.to_string(),
            query: Vec::new(),
            headers: post_headers(&[]),
        };
        let reply = self.post(&request, json_body, "application/json").await?;
        Ok(reply.text)
    }

    /// POST JSON（按给定请求描述，含自定义头）并返回响应文本。
    async fn post_json_with(
        &self,
        request: &HttpRequest,
        json_body: &str,
    ) -> Result<String, SliveError> {
        let reply = self.post(request, json_body, "application/json").await?;
        Ok(reply.text)
    }
}

/// POST 的默认请求头（UA + 调用方附加头；同名以调用方为准）。
fn post_headers(extra: &[(&str, String)]) -> Vec<(String, String)> {
    let mut headers = vec![("user-agent".to_string(), DEFAULT_USER_AGENT.to_string())];
    for (key, value) in extra {
        if let Some(existing) = headers
            .iter_mut()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
        {
            existing.1 = value.clone();
        } else {
            headers.push(((*key).to_string(), value.clone()));
        }
    }
    headers
}

impl<T: HttpTransport + ?Sized> HttpTransportExt for T {}

fn truncate(body: &str) -> String {
    if body.chars().count() <= ERROR_BODY_CAP {
        body.to_string()
    } else {
        body.chars().take(ERROR_BODY_CAP).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn check_api_code_passes_zero_and_missing() {
        assert!(check_api_code(&json!({"code": 0})).is_ok());
        assert!(check_api_code(&json!({"data": []})).is_ok());
    }

    #[test]
    fn check_api_code_maps_business_error() {
        let err = check_api_code(&json!({"code": -352, "message": "风控"}))
            .expect_err("code!=0 必须报错");
        match err {
            SliveError::Api { code, message } => {
                assert_eq!(code, -352, "错误码应原样带出");
                assert_eq!(message, "风控");
            }
            other => panic!("期望 SliveError::Api，实际 {other:?}"),
        }
    }

    #[test]
    fn reply_json_reports_decode_error() {
        let reply = HttpReply {
            status: 200,
            text: "not json".to_string(),
            set_cookie: vec![],
        };
        assert!(matches!(reply.json(), Err(SliveError::Decode(_))));
    }
}
