//! 快手集成测试共享支撑。

use std::sync::{Arc, OnceLock};

use serde_json::Value;
use slive_core::platforms::kuaishou::KuaishouSite;
use slive_core::testing::{FakeTime, MockTransport};

/// 固定时间戳（会话 / lssid 派生用）。
pub const FIXED_WTS: u64 = 1_700_000_000;

fn responses() -> &'static Value {
    static RESPONSES: OnceLock<Value> = OnceLock::new();
    RESPONSES.get_or_init(|| {
        serde_json::from_str(include_str!("../fixtures/kuaishou/responses.json"))
            .expect("解析快手列表夹具")
    })
}

/// 取夹具中的一个响应体（`home_list` / `search_author` / `category_areas` / `area_rooms`）。
///
/// 键名打错时直接 panic：`Value` 索引缺键返回 `Null`，若照常注册成 stub，
/// 下游只会报「缺少 data.list」之类看不出根因的错误。
pub fn sample(key: &str) -> String {
    let value = &responses()[key];
    assert!(
        !value.is_null(),
        "夹具 fixtures/kuaishou/responses.json 缺少键「{key}」"
    );
    serde_json::to_string(value).expect("序列化夹具")
}

/// 快手测试站点 + mock HTTP。
pub struct Harness {
    /// 被测站点。
    pub site: KuaishouSite,
    /// HTTP mock（记录所有请求）。
    pub http: Arc<MockTransport>,
}

/// 构造注入式站点。
pub fn harness() -> Harness {
    let http = Arc::new(MockTransport::new());
    let time = Arc::new(FakeTime::new(FIXED_WTS));
    let site = KuaishouSite::with_deps(http.clone(), time);
    Harness { site, http }
}
