//! Twitch GQL **POST JSON** seam。
//!
//! 背景：Twitch 全部读接口都走 `https://gql.twitch.tv/gql` 的 **POST JSON**
//! （`twitch_site.dart:49` `buildPersistedRequest` + `HttpClient.instance.postJson`），
//! 而共享的 [`crate::http::HttpTransport`] 只暴露 `get`（B站全 GET）。为避免在多平台
//! 并行期间改动共享 `http.rs`，这里在 Twitch 模块内自建一条**向下依赖**的 POST 通道：
//! - 生产：[`ReqwestTwitchPost`]（复用 `crate::http::build_client` 的共享 rustls Client 口径）
//! - 测试：[`MockTwitchPost`]（按 URL + 请求体标记返回夹具 + 记录请求，供离线用例断言）
//!
//! 纪律：**不自动重试**；业务错误由上层判定并抛错。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::error::SliveError;
use crate::http::{build_client, HttpReply};

/// 错误体截断上限（防止把整页 HTML 塞进错误消息）。
const ERROR_BODY_CAP: usize = 512;

/// 一次 POST JSON 请求描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostRequest {
    /// 目标 URL（无 query）。
    pub url: String,
    /// JSON 请求体（`postJson(data:)` 的等价物 —— `data` 本身是已序列化的 JSON 串）。
    pub body: String,
    /// 请求头（顺序保留）。
    pub headers: Vec<(String, String)>,
}

/// Twitch POST 通道抽象（seam）。
#[async_trait]
pub trait TwitchPost: Send + Sync {
    /// 发送一次 JSON POST（`content-type: application/json`）。
    async fn post_json(&self, request: &PostRequest) -> Result<HttpReply, SliveError>;
}

/// 生产实现：`reqwest` + rustls（复用共享 Client 构造，保持与 GET 通道同 TLS/超时口径）。
pub struct ReqwestTwitchPost {
    client: reqwest::Client,
}

impl ReqwestTwitchPost {
    /// 生产构造。
    pub fn new() -> Self {
        Self {
            client: build_client(),
        }
    }
}

impl Default for ReqwestTwitchPost {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TwitchPost for ReqwestTwitchPost {
    async fn post_json(&self, request: &PostRequest) -> Result<HttpReply, SliveError> {
        let mut builder = self
            .client
            .post(&request.url)
            .header("content-type", "application/json");
        for (key, value) in &request.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }
        let response = builder
            .body(request.body.clone())
            .send()
            .await
            .map_err(|e| SliveError::network(e.to_string()))?;

        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| SliveError::network(format!("读取响应体失败: {e}")))?;

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

fn truncate(body: &str) -> String {
    if body.chars().count() <= ERROR_BODY_CAP {
        body.to_string()
    } else {
        body.chars().take(ERROR_BODY_CAP).collect()
    }
}

/// 可脚本化的 mock POST 通道，**记录全部请求**（含 body，供 golden 断言）。
///
/// 所有 GQL 请求共用一个 URL，所以除了 URL 子串，还可以用 `body_marker`
/// （通常是 `operationName`）把不同 operation 路由到不同夹具。带标记的夹具
/// 优先于通用夹具，避免注册顺序影响命中结果。
///
/// 放在平台模块内而非共享 `testing.rs`，是为了多平台并行期间不改共享文件。
pub struct MockTwitchPost {
    stubs: Mutex<Vec<(String, String, StubOutcome)>>,
    requests: Mutex<Vec<PostRequest>>,
}

/// 夹具响应（**可克隆**，因此同一条夹具可被多次请求命中 —— 与
/// `crate::testing::MockTransport` 的「夹具常驻」语义一致）。
///
/// 不能直接存 `Result<HttpReply, SliveError>`：`SliveError` 未实现 `Clone`，
/// 取出时只能 move（会消费夹具）或拿到引用（类型不匹配）。
#[derive(Debug, Clone)]
enum StubOutcome {
    /// 200 + 文本。
    Text(String),
    /// 同一 operation 的多次请求依次返回不同夹具（cursor 分页测试用），
    /// 用尽后重复最后一条。
    Sequence {
        bodies: Vec<String>,
        cursor: Arc<Mutex<usize>>,
    },
    /// HTTP 错误状态。
    Http(u16),
    /// 传输层失败。
    Network(String),
}

impl StubOutcome {
    fn into_result(self) -> Result<HttpReply, SliveError> {
        match self {
            StubOutcome::Text(text) => Ok(reply(&text)),
            StubOutcome::Sequence { bodies, cursor } => {
                if bodies.is_empty() {
                    return Err(SliveError::network("空的顺序夹具"));
                }
                let mut index = cursor.lock().expect("sequence poisoned");
                let body = &bodies[(*index).min(bodies.len() - 1)];
                *index += 1;
                Ok(reply(body))
            }
            StubOutcome::Http(status) => Err(SliveError::Http {
                status,
                body: String::new(),
            }),
            StubOutcome::Network(message) => Err(SliveError::network(message)),
        }
    }
}

impl Default for MockTwitchPost {
    fn default() -> Self {
        Self::new()
    }
}

impl MockTwitchPost {
    /// 新建。
    pub fn new() -> Self {
        Self {
            stubs: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// 注册一条 200 + 文本响应夹具（`key` 为 URL 子串，匹配任意请求体）。
    pub fn stub(&self, key: &str, body: &str) {
        self.push(key, "", StubOutcome::Text(body.to_string()));
    }

    /// 注册一条只在请求体含 `body_marker` 时命中的响应夹具。
    pub fn stub_body(&self, key: &str, body_marker: &str, body: &str) {
        self.push(key, body_marker, StubOutcome::Text(body.to_string()));
    }

    /// 注册同一 operation 的多条响应，按请求顺序依次返回（用尽后重复最后一条）。
    pub fn stub_sequence(&self, key: &str, body_marker: &str, bodies: &[&str]) {
        self.push(
            key,
            body_marker,
            StubOutcome::Sequence {
                bodies: bodies.iter().map(|body| body.to_string()).collect(),
                cursor: Arc::new(Mutex::new(0)),
            },
        );
    }

    /// 注册一条 HTTP 错误夹具。
    pub fn stub_http_error(&self, key: &str, status: u16) {
        self.push(key, "", StubOutcome::Http(status));
    }

    /// 注册一条传输层失败夹具。
    pub fn stub_network_error(&self, key: &str, message: &str) {
        self.push(key, "", StubOutcome::Network(message.to_string()));
    }

    fn push(&self, key: &str, body_marker: &str, outcome: StubOutcome) {
        self.stubs.lock().expect("stubs poisoned").push((
            key.to_string(),
            body_marker.to_string(),
            outcome,
        ));
    }

    /// 已记录的全部请求（克隆）。
    pub fn requests(&self) -> Vec<PostRequest> {
        self.requests.lock().expect("requests poisoned").clone()
    }

    /// 命中 `key` 的请求次数（无重试断言）。
    pub fn request_count(&self, key: &str) -> usize {
        self.requests
            .lock()
            .expect("requests poisoned")
            .iter()
            .filter(|r| r.url.contains(key))
            .count()
    }
}

fn reply(body: &str) -> HttpReply {
    HttpReply {
        status: 200,
        text: body.to_string(),
        set_cookie: Vec::new(),
    }
}

#[async_trait]
impl TwitchPost for MockTwitchPost {
    async fn post_json(&self, request: &PostRequest) -> Result<HttpReply, SliveError> {
        self.requests
            .lock()
            .expect("requests poisoned")
            .push(request.clone());
        let matched = {
            let stubs = self.stubs.lock().expect("stubs poisoned");
            let url_matches = || {
                stubs
                    .iter()
                    .filter(|(url_key, _, _)| request.url.contains(url_key.as_str()))
            };
            url_matches()
                .find(|(_, marker, _)| {
                    !marker.is_empty() && request.body.contains(marker.as_str())
                })
                .or_else(|| url_matches().find(|(_, marker, _)| marker.is_empty()))
                .map(|(_, _, outcome)| outcome.clone())
        };
        match matched {
            None => Err(SliveError::network(format!(
                "MockTwitchPost 未注册 URL: {}",
                request.url
            ))),
            Some(outcome) => outcome.into_result(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_records_body_and_returns_stub() {
        let mock = MockTwitchPost::new();
        mock.stub("gql.twitch.tv", r#"[{"data":{}}]"#);
        let got = mock
            .post_json(&PostRequest {
                url: "https://gql.twitch.tv/gql".to_string(),
                body: r#"[{"operationName":"ChannelShell"}]"#.to_string(),
                headers: vec![("client-id".to_string(), "cid".to_string())],
            })
            .await
            .expect("命中夹具");
        assert_eq!(got.status, 200);
        assert_eq!(got.text, r#"[{"data":{}}]"#);
        assert_eq!(mock.request_count("gql.twitch.tv"), 1);
        assert!(mock.requests()[0].body.contains("ChannelShell"));
    }

    #[tokio::test]
    async fn body_marker_routes_operations_regardless_of_registration_order() {
        let mock = MockTwitchPost::new();
        mock.stub("gql.twitch.tv", "generic");
        mock.stub_body("gql.twitch.tv", "DirectoryPage_Game", "game");
        mock.stub_body("gql.twitch.tv", "SearchCategoryTags", "tags");
        async fn send(mock: &MockTwitchPost, body: &str) -> String {
            mock.post_json(&PostRequest {
                url: "https://gql.twitch.tv/gql".to_string(),
                body: body.to_string(),
                headers: vec![],
            })
            .await
            .expect("命中夹具")
            .text
        }
        assert_eq!(send(&mock, "SearchCategoryTags").await, "tags");
        assert_eq!(send(&mock, "DirectoryPage_Game").await, "game");
        assert_eq!(send(&mock, "something else").await, "generic");
        assert_eq!(mock.request_count("gql.twitch.tv"), 3);
    }

    #[tokio::test]
    async fn sequence_returns_bodies_in_order_and_repeats_the_last() {
        let mock = MockTwitchPost::new();
        mock.stub_sequence("gql.twitch.tv", "DirectoryPage_Game", &["p1", "p2"]);
        let send = || async {
            mock.post_json(&PostRequest {
                url: "https://gql.twitch.tv/gql".to_string(),
                body: "DirectoryPage_Game".to_string(),
                headers: vec![],
            })
            .await
            .expect("命中夹具")
            .text
        };
        assert_eq!(send().await, "p1");
        assert_eq!(send().await, "p2");
        assert_eq!(send().await, "p2");
    }

    #[tokio::test]
    async fn mock_unregistered_url_is_network_error() {
        let mock = MockTwitchPost::new();
        let err = mock
            .post_json(&PostRequest {
                url: "https://gql.twitch.tv/gql".to_string(),
                body: "[]".to_string(),
                headers: vec![],
            })
            .await
            .expect_err("未注册必须报错");
        assert!(matches!(err, SliveError::Network(_)), "实际 {err:?}");
    }

    #[test]
    fn truncate_caps_error_body() {
        assert_eq!(truncate("abc"), "abc");
        assert_eq!(truncate(&"x".repeat(600)).chars().count(), ERROR_BODY_CAP);
    }
}
