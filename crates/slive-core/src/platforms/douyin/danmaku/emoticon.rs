//! 抖音标准表情包表。
//!
//! 普通聊天只下发文本码（如 `[微笑]`），图片不在消息里，因此只能靠一张
//! 「文本码 -> 图片」表把占位符替换成图片；未命中表的方括号文本保持原样。
//!
//! 表的来源有两层：
//! - **运行时**：公开接口 `aweme/v1/web/emoji/list`（见 [`fetch`]），无需登录
//!   cookie，签名地址会随接口自动刷新，是会话首选；
//! - **内置静态表**：接口失败 / 超时时的兜底（[`DOUYIN_EMOTICONS`]），由
//!   `tools/generate_douyin_emoticons.mjs` 从同一接口生成。
//!
//! 口径对齐虎牙：只输出正文里真实出现的码；重复出现保留多个；large = false
//! （行内表情，高度跟随聊天字号）。会员 / 守护表情走
//! `WebcastEmojiChatMessage`，在 `decode` 里单独标记 `large = true`。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use serde_json::Value;

use crate::http::{HttpRequest, HttpTransport};
use crate::models::LiveMessageEmoticon;

use super::args::DEFAULT_USER_AGENT;

/// 表情列表接口（网页端表情面板同源数据）。
pub const EMOJI_LIST_URL: &str = "https://www.douyin.com/aweme/v1/web/emoji/list";
const REFERER: &str = "https://www.douyin.com/";
/// 表情表是增强信息，不能把弹幕首连卡到传输层的 45s 超时。
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
/// 成功拉到的线上表情表缓存时长。
///
/// 这张表是全局数据（与房间无关），一天之内基本不变；每个会话都重发一次
/// 118KB 请求纯属浪费。取 30 分钟：既能省掉切直播间时的重复请求，又能在
/// 平台增删表情后自然跟上（且签名 URL 本身有效期到 2036 年，缓存不会失效）。
const CACHE_TTL: Duration = Duration::from_secs(30 * 60);

/// `[占位符]` -> 图片地址。
pub type DouyinEmoticonTable = HashMap<String, String>;

/// 进程内缓存：`(拉取时刻的 unix 秒, 表)`。
///
/// 只在**接口成功**时写入；失败/超时一律返回静态表且不污染缓存，
/// 这样下一次会话还会重试（网络抖动不会把静态表钉死一整个 TTL）。
static CACHE: Lazy<std::sync::Mutex<Option<(u64, DouyinEmoticonTable)>>> =
    Lazy::new(|| std::sync::Mutex::new(None));

/// 文本码 -> 图片地址（顺序与网页端表情面板一致）。
const DOUYIN_EMOTICONS: &[(&str, &str)] = &[
    ("[微笑]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/54a0f69dc150401a85ba8c20c1a05db1?lk3s=343af0a2&x-expires=2108847600&x-signature=p0zOJ12Gekuw99uoRdMU59eujPw%3D&from=876277922"),
    ("[色]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/27a1d35dea9748ac8c14f6c2c9829965?lk3s=343af0a2&x-expires=2108847600&x-signature=ldQIxtWWTBj6V1oAxjqgEX0Ya4c%3D&from=876277922"),
    ("[发呆]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/88ebe7b3f2e24729aa4de3ff33ae5731?lk3s=343af0a2&x-expires=2108847600&x-signature=qKCnE9AIQcXoABGHa%2FHJI81inOM%3D&from=876277922"),
    ("[酷拽]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/0fac43466165409a805b3b6e3547ccef?lk3s=343af0a2&x-expires=2108847600&x-signature=0%2BdGrHZy%2FhMynLWsig8NMSmquC4%3D&from=876277922"),
    ("[抠鼻]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/accde8fa9fe04c5e8fa3f25d1ddb5584?lk3s=343af0a2&x-expires=2108847600&x-signature=DnWrl93ReT2phQMn1LfXyHDqfhs%3D&from=876277922"),
    ("[流泪]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/2d8dc1e2bb8a417cab4130df2d10e478?lk3s=343af0a2&x-expires=2108847600&x-signature=cuq29NW0CyPQh3VWmR%2BoCwN4EuY%3D&from=876277922"),
    ("[捂脸]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/357ac72b9b6449f7bd3005ff66678822?lk3s=343af0a2&x-expires=2108847600&x-signature=CSxBxNgbf4Jb8wpwfPDBczfcoIA%3D&from=876277922"),
    ("[发怒]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/156352b6879b4b5f96fd80434ff72c8e?lk3s=343af0a2&x-expires=2108847600&x-signature=PGCb3roARryx3cTTzG5i3lOo7UU%3D&from=876277922"),
    ("[呲牙]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/17bc9c4501784279b8e7d886832b0f28?lk3s=343af0a2&x-expires=2108847600&x-signature=fMUYwExRCzbMTdUewEDFqUPIJKw%3D&from=876277922"),
    ("[尬笑]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/96744c9a302345d1b14d7e0a9951c7b9?lk3s=343af0a2&x-expires=2108847600&x-signature=Mqpx79VlnR%2Fg5ntGqjpNHAE3wps%3D&from=876277922"),
    ("[害羞]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/d05cee2608fe4f998fef3f90de9466df?lk3s=343af0a2&x-expires=2108847600&x-signature=YUJhAjvL9EF7MNUGDTBUfv8yAlw%3D&from=876277922"),
    ("[调皮]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/a921fa28653043c1bbf162abe017c99a?lk3s=343af0a2&x-expires=2108847600&x-signature=%2BsV%2BzzOgVmM7%2BAiFO4hMt4zhGGg%3D&from=876277922"),
    ("[舔屏]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/d99bd102d422484188b98e8889c0c330?lk3s=343af0a2&x-expires=2108847600&x-signature=o%2Fj6jk3qi%2BexJ8rV9xePeLg4sM0%3D&from=876277922"),
    ("[看]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/87c2ae45679c4cc4a35bd7182fd76935?lk3s=343af0a2&x-expires=2108847600&x-signature=jVkWt3P7HP%2BtgSXJSzG9JwYUUkI%3D&from=876277922"),
    ("[爱心]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/427b632213784532a4076c3b0ab269fb?lk3s=343af0a2&x-expires=2108847600&x-signature=wPTHFq8rLLXtizQ0pbPacrvX9fU%3D&from=876277922"),
    ("[比心]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/75b4e7e2405447959578f54d87811e35?lk3s=343af0a2&x-expires=2108847600&x-signature=nALEAQm1rJ9bkKUIvqjaUPhfjwY%3D&from=876277922"),
    ("[赞]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/1a09f79a0e7e433eb2bc143c9f026d65?lk3s=343af0a2&x-expires=2108847600&x-signature=mXi8J3aDkSy%2FffkN%2BICRB1kvrGQ%3D&from=876277922"),
    ("[鼓掌]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/15d1258bec8e4c9ea9687f1134bd5f65?lk3s=343af0a2&x-expires=2108847600&x-signature=RCZ0BSDmFf13HfZuPcXMi4MLDog%3D&from=876277922"),
    ("[感谢]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/fabab8b14ec64f669283287a4b7041cb?lk3s=343af0a2&x-expires=2108847600&x-signature=4w4VkB68M3saUetzlQtPLJ7yzqw%3D&from=876277922"),
    ("[抱抱你]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/299f50b8f96640d5b349cc6cb4a76030?lk3s=343af0a2&x-expires=2108847600&x-signature=Jb2A%2Fmw%2F67QOhTSZdXntfkRdrVI%3D&from=876277922"),
    ("[玫瑰]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/683fcd46faf148af99c90e36ccc4c013?lk3s=343af0a2&x-expires=2108847600&x-signature=sRSbsWA1XbgPAYr%2BVcg5VbsUMBY%3D&from=876277922"),
    ("[尴尬流汗]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/160eca1320584e2e87122e7d7ba72a53?lk3s=343af0a2&x-expires=2108847600&x-signature=%2B8zKHEs1Pt10EXfeUH8xdsY08n4%3D&from=876277922"),
    ("[戳手手]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/4268c0b885ec4eae8e95dab8635ca215?lk3s=343af0a2&x-expires=2108847600&x-signature=XasGNt%2F1ptjrhEG5YsjkCxinoDs%3D&from=876277922"),
    ("[星星眼]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/007064f99b694041a1890e52ff8c5768?lk3s=343af0a2&x-expires=2108847600&x-signature=Js1ce2XFjx2ktIGwAsJN8v4NmvE%3D&from=876277922"),
    ("[杀马特]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/ed5fb68598cf4741b3e7f2affd825650?lk3s=343af0a2&x-expires=2108847600&x-signature=gEnZ8wRVXGZRSl75VjMWwvQ4fvI%3D&from=876277922"),
    ("[黄脸干杯]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/a0a1e35991334532b142572d93f38ac5?lk3s=343af0a2&x-expires=2108847600&x-signature=uGL2GD7671ve5Z3oR5c%2F5Shcl2Q%3D&from=876277922"),
    ("[抱紧自己]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/c0cf6b7d6a144b68b6045fd92bfe9fe2?lk3s=343af0a2&x-expires=2108847600&x-signature=pSwARurRF0g238HTQGckN%2Bez%2B5U%3D&from=876277922"),
    ("[拜拜]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/36cf73734e9c49d48b3e9a5bbe2affaf?lk3s=343af0a2&x-expires=2108847600&x-signature=kzvw6eO0PolxinrqoWXpzfYpyZM%3D&from=876277922"),
    ("[热化了]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/2cffc18234074d20acb0c6334c76f04a?lk3s=343af0a2&x-expires=2108847600&x-signature=BFhaj9Kmf128cjnp30HJOeJKVX8%3D&from=876277922"),
    ("[黄脸祈祷]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/3f67722d0c694372b7023deda01bd91c?lk3s=343af0a2&x-expires=2108847600&x-signature=CNkakRmOeSeGu1PsCN3zOqBZD78%3D&from=876277922"),
    ("[懵]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/dbd68a1183b344648759f5971cbc6238?lk3s=343af0a2&x-expires=2108847600&x-signature=19R1qnmm2sQ04mRTc06Xr3T%2FYeo%3D&from=876277922"),
    ("[举手]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/3c34da0d65db47c6933e6b66cc17e5aa?lk3s=343af0a2&x-expires=2108847600&x-signature=bZcVOxrxEYxoWx04tBgraHgiaUc%3D&from=876277922"),
    ("[加功德]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/49b00fcf022d4c36ba86bb19c7358ec6?lk3s=343af0a2&x-expires=2108847600&x-signature=gPYpTJutH%2BFf4v1qJpgfJVl1w5Y%3D&from=876277922"),
    ("[摊手]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/6ffc70d8850b4e5eb6c7a5aa9e7b1782?lk3s=343af0a2&x-expires=2108847600&x-signature=MLrxFQwvZei4nJyz3nFXHqxQE0M%3D&from=876277922"),
    ("[无语流汗]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/67f5a31e2895499dbfff97ff81efb0f8?lk3s=343af0a2&x-expires=2108847600&x-signature=C45Mp%2F6sPtHAFrFOPUEwLjOnqlg%3D&from=876277922"),
    ("[续火花吧]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/1da1cd2598ba48a78cf07dc7eea09338?lk3s=343af0a2&x-expires=2108847600&x-signature=R8u4ZwFwuainZ5wYMLnoKbnKaNE%3D&from=876277922"),
    ("[点火]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/3bb9834bd95d4a5ba305ca2d788bd62e?lk3s=343af0a2&x-expires=2108847600&x-signature=ZH%2F0QD5AfbixtN7LseAUEf2nJhs%3D&from=876277922"),
    ("[哭哭]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/c380b32e707b43a2967ee7b492771c0f?lk3s=343af0a2&x-expires=2108847600&x-signature=hsGQk0wo7CJ%2FWot%2BOqc%2BC6RnpRY%3D&from=876277922"),
    ("[吐舌小狗]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/e4fb11ce2e2b432483ef58e058be6e0d?lk3s=343af0a2&x-expires=2108847600&x-signature=p5KSmBNIQM13zFgb3ZMJ9zBGE5A%3D&from=876277922"),
    ("[送花]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/ff736698b03446d19ae05cbd6cc45256?lk3s=343af0a2&x-expires=2108847600&x-signature=QO%2BiSDoEqVs7hH8UMGpfzfxHfiE%3D&from=876277922"),
    ("[爱心手]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/7f50f1d4d0c74571b1a60eba9b41a4d7?lk3s=343af0a2&x-expires=2108847600&x-signature=RocBCEv5oWDEDJkUBdgtVCj5zxM%3D&from=876277922"),
    ("[贴贴]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/c3dee11223784da3a2ed524d94bb2ce8?lk3s=343af0a2&x-expires=2108847600&x-signature=flz1%2FdHjdOV8qAXwnmzJHToekHo%3D&from=876277922"),
    ("[灵机一动]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/801127f060844405b0f5e00bd01cb8ba?lk3s=343af0a2&x-expires=2108847600&x-signature=6XKt2fCAWxfDg8okfAxsPcB0CYw%3D&from=876277922"),
    ("[耶]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/f00c1c8054ff41768994256236d305b1?lk3s=343af0a2&x-expires=2108847600&x-signature=ok6fWmL%2BjAU0Xpyt3FP6Fyxz%2B0g%3D&from=876277922"),
    ("[打脸]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/d00f39c3d5d54144a395ad4d7f8d4489?lk3s=343af0a2&x-expires=2108847600&x-signature=dWP7rJyLZSIybdbvrW0YSqylkHw%3D&from=876277922"),
    ("[大笑]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/ce993ad237174993a2ed8de550a63772?lk3s=343af0a2&x-expires=2108847600&x-signature=tW5ohpw%2FcRYtbb3h2skPtlxTJ6c%3D&from=876277922"),
    ("[机智]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/ca113e21412d4609ae38032b3befddb9?lk3s=343af0a2&x-expires=2108847600&x-signature=XrjHqD0w0VziuCMJveaI%2Bidl3c0%3D&from=876277922"),
    ("[送心]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/72211d3eadb64791a967f43ab340a55a?lk3s=343af0a2&x-expires=2108847600&x-signature=e4YVt4qtA51GnHTOc99Vk62pXQk%3D&from=876277922"),
    ("[666]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/11a083f1a213461f859370ccbce074df?lk3s=343af0a2&x-expires=2108847600&x-signature=z8Sf5R%2BzoGeWJllQC6ADpU%2FVTb0%3D&from=876277922"),
    ("[闭嘴]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/d32f14b5edfd4489a4ea736a8ad39741?lk3s=343af0a2&x-expires=2108847600&x-signature=O0tQBpQ7i1to%2FvXUdu6691f32SY%3D&from=876277922"),
    ("[来看我]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/27059cc0891649a8b2b0860b5b08f540?lk3s=343af0a2&x-expires=2108847600&x-signature=JVD7yQodKZ8ixXBC6VcT3omMdjk%3D&from=876277922"),
    ("[一起加油]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/632214f7625a41ec8b558961929153f8?lk3s=343af0a2&x-expires=2108847600&x-signature=%2B0UepmsvA5MfNwnuPjaA%2BOnjNsc%3D&from=876277922"),
    ("[哈欠]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/8ef4e9f6ef36449dab4c5ecde36f7d6b?lk3s=343af0a2&x-expires=2108847600&x-signature=Nbm3wcUv8aD6K0b2QxgxdHaNbA8%3D&from=876277922"),
    ("[震惊]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/7181d20e807f4c5787f484531d1d8468?lk3s=343af0a2&x-expires=2108847600&x-signature=cyZm4QhXgNFuy4Z63g4SnIz6XsA%3D&from=876277922"),
    ("[晕]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/2af8cb49d2854197b7dd9b87f9dd106b?lk3s=343af0a2&x-expires=2108847600&x-signature=qr46sg9MtblB%2FTtYA%2FzeOv0c4ls%3D&from=876277922"),
    ("[衰]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/aa5c7f4a1d0c430a94934287cacd3c17?lk3s=343af0a2&x-expires=2108847600&x-signature=TIWCyW9FB5pnDf8I7s4LY%2FjUg54%3D&from=876277922"),
    ("[困]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/23cdce75a6d64bd8bb7ff87037db1be0?lk3s=343af0a2&x-expires=2108847600&x-signature=d6%2FrCI%2BhTCbkjVOe36nTf%2FWiuQ8%3D&from=876277922"),
    ("[疑问]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/9ea78ef420fa4bf1b7e544a01e75b830?lk3s=343af0a2&x-expires=2108847600&x-signature=8RHxGpjm7Hc2oiPqwwsP%2FSTjKAs%3D&from=876277922"),
    ("[泣不成声]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/7fb67c323a0d410ea5573d4825e347eb?lk3s=343af0a2&x-expires=2108847600&x-signature=HiF3KI2Nyf6IqJYKsdB3725Kma0%3D&from=876277922"),
    ("[小鼓掌]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/8f19f0db21324c7f9ce7b2dca984a309?lk3s=343af0a2&x-expires=2108847600&x-signature=PQ%2BwoxFXNeEyYoMXwb9OdSC6QgU%3D&from=876277922"),
    ("[大金牙]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/b94090121f3e4549a3bee84e00515ed0?lk3s=343af0a2&x-expires=2108847600&x-signature=ATeybKWW74oHdnPlnWNWgDWUDNQ%3D&from=876277922"),
    ("[偷笑]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/817c931baafa4d06a120a8d67973b6f0?lk3s=343af0a2&x-expires=2108847600&x-signature=b8%2FRaNC7tEA7wraSg0FVdcxOZEA%3D&from=876277922"),
    ("[石化]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/88746458dcbd47b1aa801e3823e6c9a2?lk3s=343af0a2&x-expires=2108847600&x-signature=94igjlvJFlTiy8XzmoLYZg2wHAY%3D&from=876277922"),
    ("[思考]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/c3e345069adf42f58036c349ecc20a00?lk3s=343af0a2&x-expires=2108847600&x-signature=s6vVLo4j18R2BIvtzbFsWsf%2BrMY%3D&from=876277922"),
    ("[吐血]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/331299e25af844a38800ec75d8dd0a16?lk3s=343af0a2&x-expires=2108847600&x-signature=7ahW9ebSNx%2BE4IONPHZWDUdSuew%3D&from=876277922"),
    ("[可怜]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/1369a30daa7d45d6b3fa5ab241938fd2?lk3s=343af0a2&x-expires=2108847600&x-signature=W7NvSmF0YV%2FwgIG6RzngJE1m5Ms%3D&from=876277922"),
    ("[嘘]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/7c02d77ba7b44cfda223941529ccb2e6?lk3s=343af0a2&x-expires=2108847600&x-signature=K96mmm6jh2OMP2bwTurmMartwfw%3D&from=876277922"),
    ("[撇嘴]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/784ef77ac9de4e4990a607166856cd38?lk3s=343af0a2&x-expires=2108847600&x-signature=c6m0tmPMR2Wp7q0XJ9Hkpccqt7w%3D&from=876277922"),
    ("[笑哭]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/e4f0c86449754bab8ae2cefe6317ceee?lk3s=343af0a2&x-expires=2108847600&x-signature=dPgx5OogbI7WaIHWD8K5aQFg79w%3D&from=876277922"),
    ("[奸笑]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/6ac6247beb3a41d9b9e0e982e0225506?lk3s=343af0a2&x-expires=2108847600&x-signature=03GqcznYWP5J7%2Fmkg6JSKRDBg1A%3D&from=876277922"),
    ("[得意]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/b1b52b6e940a4ef8907819eb8e871c21?lk3s=343af0a2&x-expires=2108847600&x-signature=3cUTudNyiEpCmW1WBK3lys2zRuk%3D&from=876277922"),
    ("[憨笑]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/fceeae50c04b4343b44055ba9f9bb1a3?lk3s=343af0a2&x-expires=2108847600&x-signature=TjdCsW8ZUnlOYQVmtlgtLzCQbno%3D&from=876277922"),
    ("[坏笑]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/eab59a94674f40358b412586f148aac6?lk3s=343af0a2&x-expires=2108847600&x-signature=QRRZkyRLCfy0BmLxwjeFu3NedQg%3D&from=876277922"),
    ("[抓狂]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/e7ffd9db28e3455195872ebf666d2c66?lk3s=343af0a2&x-expires=2108847600&x-signature=BaLGSaxzvZJDq6F7XSti8v8GUxY%3D&from=876277922"),
    ("[泪奔]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/20dbe8334dd64d028f943abc2ccde4d9?lk3s=343af0a2&x-expires=2108847600&x-signature=4wDf352pqY98%2FPMBFS4CSWo20JA%3D&from=876277922"),
    ("[钱]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/9a45bb9c0b8a4963b858846b079cfe2f?lk3s=343af0a2&x-expires=2108847600&x-signature=wF3L%2Fplez8T%2FjWeN98yMP5yf6bs%3D&from=876277922"),
    ("[恐惧]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/c12d025f50a047c2b064ce5f1a196f94?lk3s=343af0a2&x-expires=2108847600&x-signature=YIFvT34em%2Baspl7grAsCg8j3yms%3D&from=876277922"),
    ("[愉快]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/62266cefe5fe4422aceaeaeed9072b3b?lk3s=343af0a2&x-expires=2108847600&x-signature=qh0rDFOLgwb4%2FdepoRdruMQ2NsQ%3D&from=876277922"),
    ("[快哭了]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/79df79563eaf424e9eb7a26558236ff7?lk3s=343af0a2&x-expires=2108847600&x-signature=GqSCl5vm6tRaLnU2EfEbl6cnvAE%3D&from=876277922"),
    ("[翻白眼]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/0e40da06368145848f7585edc937f34e?lk3s=343af0a2&x-expires=2108847600&x-signature=921zL0KQw%2BQnlCe1%2FA9oRZC%2Bwlo%3D&from=876277922"),
    ("[互粉]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/cb46843009fe47808953af2c9ab2fb9a?lk3s=343af0a2&x-expires=2108847600&x-signature=0GZ6eMCZOfmo6NZKSBaSSc1aJLI%3D&from=876277922"),
    ("[我想静静]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/95d5d919b99c43f29d2fbc660ec873f0?lk3s=343af0a2&x-expires=2108847600&x-signature=OG5EdmnCFoFksVMUNtVxp%2BTI1zA%3D&from=876277922"),
    ("[委屈]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/2f7509f2cbe248889f279d19ea3fd649?lk3s=343af0a2&x-expires=2108847600&x-signature=RIWQQjixbHYmmV3MrSPGXcdLzYs%3D&from=876277922"),
    ("[鄙视]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/5c13cbb44f334122a9cd7cf4bcc6598e?lk3s=343af0a2&x-expires=2108847600&x-signature=Tor09zYdGjF80WFwgGqyXBdTHdE%3D&from=876277922"),
    ("[飞吻]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/6dcb7560c1c84d45b4abe2cb37ae7afb?lk3s=343af0a2&x-expires=2108847600&x-signature=9hUbB51q7MEy7RJBpMk4V0EtVTc%3D&from=876277922"),
    ("[再见]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/cae63f89c3eb478f8d1cca893b1b1a0c?lk3s=343af0a2&x-expires=2108847600&x-signature=Q6PgnW0Vo1HEJ8Lw5XSp%2FZG5m58%3D&from=876277922"),
    ("[紫薇别走]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/1d921b6eff48473f872fa7d278d26926?lk3s=343af0a2&x-expires=2108847600&x-signature=VAzpEtz3u5dUad%2FYLKywxOegnrw%3D&from=876277922"),
    ("[听歌]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/82bed7a3182b4c6db21ca39417359fa1?lk3s=343af0a2&x-expires=2108847600&x-signature=ZRXTyGWddTctoomf6jFil2mk6hQ%3D&from=876277922"),
    ("[求抱抱]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/de99b54c6be24f73a5aaef8366a55e99?lk3s=343af0a2&x-expires=2108847600&x-signature=bqvD7KOpWAG2lQ38ZcSCNvMyGCY%3D&from=876277922"),
    ("[绝望的凝视]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/19f6a9449e714d85bb393794daaf6d58?lk3s=343af0a2&x-expires=2108847600&x-signature=Uquj27IrmEvx44iMi8grNfYtcdA%3D&from=876277922"),
    ("[不失礼貌的微笑]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/b7b842ba92e14fb8a8f328f25db91276?lk3s=343af0a2&x-expires=2108847600&x-signature=TLDzsIBsv%2BlEFgW4NgH0rJu%2BHKA%3D&from=876277922"),
    ("[不看]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/626f19e473d94e95b21b67f098599281?lk3s=343af0a2&x-expires=2108847600&x-signature=CrNrRjlU6Fbp%2FPB2cEZkBPkYF2o%3D&from=876277922"),
    ("[裂开]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/e8c40bd4201e4afda065c55688971a2f?lk3s=343af0a2&x-expires=2108847600&x-signature=9rw%2FIhxFphY4WOgIJlYWCDWojSI%3D&from=876277922"),
    ("[干饭人]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/0b0adfdffaea4c62a73302ae44d67a66?lk3s=343af0a2&x-expires=2108847600&x-signature=wuzF8s5PubGRFLHJfCa9m3P3K4k%3D&from=876277922"),
    ("[庆祝]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/9c1325e3650a4a4d96d8e299406d9d68?lk3s=343af0a2&x-expires=2108847600&x-signature=LHNH9xzqZlpIrlaTtbyVHDPz5wI%3D&from=876277922"),
    ("[吐舌]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/e577535321324e68a0cc8fcff1a2f421?lk3s=343af0a2&x-expires=2108847600&x-signature=1cOAU4Hs0mBaWNvzfiF9vNR0mB8%3D&from=876277922"),
    ("[呆无辜]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/98c368cc39b74b94bf7923b6d9c479b9?lk3s=343af0a2&x-expires=2108847600&x-signature=ZPXSM9n0AOCslIspP%2BUvuiTG1j8%3D&from=876277922"),
    ("[白眼]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/a9d1ba597042412f9275369807147bee?lk3s=343af0a2&x-expires=2108847600&x-signature=4t2c22DNLd3F8MJWDIVft8MOEdQ%3D&from=876277922"),
    ("[猪头]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/4296ea156ee6479e9ad5c1381ec73cca?lk3s=343af0a2&x-expires=2108847600&x-signature=%2FDLt%2F3QbVEzGPKAqiqycw7SY%2BHs%3D&from=876277922"),
    ("[冷漠]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/ba1d020f28a749e388fb1d15e52ac4ae?lk3s=343af0a2&x-expires=2108847600&x-signature=M4mX57SUmLAyc%2BiLwCe4jC2yQdM%3D&from=876277922"),
    ("[暗中观察]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/3cf77301823a4e969cc4c0a04d245f3d?lk3s=343af0a2&x-expires=2108847600&x-signature=6fEVAOjF0oPg9%2BIpRmfyKpYjujE%3D&from=876277922"),
    ("[二哈]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/2ddc8ae48fae4a4fb3767f666bb79d95?lk3s=343af0a2&x-expires=2108847600&x-signature=V%2FLykHXabzJdjuaYoShICNEUQ7Y%3D&from=876277922"),
    ("[菜狗]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/62baecf5a0084194ac383d3d199ab597?lk3s=343af0a2&x-expires=2108847600&x-signature=hSyniVxSBGeLyYTJc1WPCWCY73Q%3D&from=876277922"),
    ("[黑脸]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/b96c76ac689f44d09537583ca2357672?lk3s=343af0a2&x-expires=2108847600&x-signature=JxGq%2FnAKFlFw0hXnLtWJc8gF63U%3D&from=876277922"),
    ("[展开说说]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/30f0c01ef9d24144a8ae05351485fe18?lk3s=343af0a2&x-expires=2108847600&x-signature=IUPFJDDlW%2BayJHuIJhkhpXALYok%3D&from=876277922"),
    ("[蜜蜂狗]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/d336f83b827b407d872bc48861a44c70?lk3s=343af0a2&x-expires=2108847600&x-signature=Loww0NmKMJ%2BQbfP%2F%2FgngpVLxZZo%3D&from=876277922"),
    ("[柴犬]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/764a5db2a67242c189f5fdabb1c75648?lk3s=343af0a2&x-expires=2108847600&x-signature=HtHYVXF%2BB16z9OjJHEvFryeUhwo%3D&from=876277922"),
    ("[摸头]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/10790e118cbc4d13b48cdc19c1eecb96?lk3s=343af0a2&x-expires=2108847600&x-signature=WDG1kfFYP8xnlMXLQU7ui4swutU%3D&from=876277922"),
    ("[皱眉]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/d143738426054c9f88657ee97b779643?lk3s=343af0a2&x-expires=2108847600&x-signature=j5BY5JihuMOlANgVtJKJHnLdcQw%3D&from=876277922"),
    ("[擦汗]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/0f7be11419c44d1ab158bbafa666101c?lk3s=343af0a2&x-expires=2108847600&x-signature=Xyn%2Bi3DZY5tVeZIWblnVhEax5Xs%3D&from=876277922"),
    ("[红脸]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/15efff0277f74e5b9e2b12c984eac159?lk3s=343af0a2&x-expires=2108847600&x-signature=uvyK%2FbYGSuIj8ZyNiLiczQTrHWY%3D&from=876277922"),
    ("[做鬼脸]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/e8d099b1dc5b41b0b8535b47747a94f4?lk3s=343af0a2&x-expires=2108847600&x-signature=DGjvxoLlgFV5jwDI%2Fw3DwfxkO%2BE%3D&from=876277922"),
    ("[强]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/0b815bcdb647471b9281e3105d8d6c8e?lk3s=343af0a2&x-expires=2108847600&x-signature=ij4KAiDVEQzXmE%2FS2HBT%2FX8OqUM%3D&from=876277922"),
    ("[如花]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/dd44ccdacbe6444a9515bdca3fb56d5f?lk3s=343af0a2&x-expires=2108847600&x-signature=L8UM6J2MsRG4EONBtjuKTB1L788%3D&from=876277922"),
    ("[吐]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/c0157bfb1fc548d097933c039d6952ae?lk3s=343af0a2&x-expires=2108847600&x-signature=Y0v1s6sG4WdSJaXpvIeu1lJ2ZfE%3D&from=876277922"),
    ("[惊喜]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/e718e4d7913c4bcf9c49560ff96386e9?lk3s=343af0a2&x-expires=2108847600&x-signature=PHOgm3%2B7BDOy9tD8x8vNCMGgb2I%3D&from=876277922"),
    ("[敲打]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/8067eb30b0d74ad29c15a8c5c7b11230?lk3s=343af0a2&x-expires=2108847600&x-signature=s5EwppKkRR69qfBDpSR63vkJvUo%3D&from=876277922"),
    ("[奋斗]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/fb3fb08ab5204f2a83086d782944882d?lk3s=343af0a2&x-expires=2108847600&x-signature=NLrZUHdNnlAqAf%2FX1mzzwrbXEhM%3D&from=876277922"),
    ("[吐彩虹]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/0878b09fe3fa4d4a8f47c63748cf7762?lk3s=343af0a2&x-expires=2108847600&x-signature=UJJ2DzpbV5ed7zC1rtGNxCfVgDs%3D&from=876277922"),
    ("[大哭]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/53246f7d51fa42e5b48b99c8074ca607?lk3s=343af0a2&x-expires=2108847600&x-signature=bNxnbZNh5V7KfbMiGvQTML%2BZOsE%3D&from=876277922"),
    ("[嘿哈]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/af4e2e63b51f4d66a060ff0a42d0098b?lk3s=343af0a2&x-expires=2108847600&x-signature=sDViHs3PfdJE4eiudQWW7bUNuo8%3D&from=876277922"),
    ("[惊恐]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/fd2f5745edbd4dadb6014096965d1c6c?lk3s=343af0a2&x-expires=2108847600&x-signature=RqotP3X7O2E71QvjTaknkWNJsRY%3D&from=876277922"),
    ("[囧]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/4cd5a1e2c43341a9b4ac95ace6ed913e?lk3s=343af0a2&x-expires=2108847600&x-signature=OjDYGlhng64UXxZEnATjXASHzQk%3D&from=876277922"),
    ("[难过]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/697dc91cd453495d988628058582fed3?lk3s=343af0a2&x-expires=2108847600&x-signature=uhga%2B%2FhacZcGAmfwDILPVlvxj14%3D&from=876277922"),
    ("[斜眼]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/63a00ad586eb4f9e81369a7e693b778b?lk3s=343af0a2&x-expires=2108847600&x-signature=mMtUs4BAnl0WTGATKb5s1YUfTjQ%3D&from=876277922"),
    ("[阴险]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/628bc8956092489399730d0c8178de5b?lk3s=343af0a2&x-expires=2108847600&x-signature=Nvjef06hh%2B02LyUPROc5YGiFaqk%3D&from=876277922"),
    ("[悠闲]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/989818a338bd4d718f0afc47f7c76a4c?lk3s=343af0a2&x-expires=2108847600&x-signature=rlbhQHHvboL%2B6HKzcP0%2F8iG%2FWvE%3D&from=876277922"),
    ("[咒骂]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/44802ec1a8bd4ff2aac38cfcf97201cb?lk3s=343af0a2&x-expires=2108847600&x-signature=4cJnl259V6RhvHGvTWBifXpib5k%3D&from=876277922"),
    ("[吃瓜群众]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/2e2059714acb489483735cbe63ff7dbc?lk3s=343af0a2&x-expires=2108847600&x-signature=mYTB9cj8iHeqWYGaNeeZRrPr%2Fwc%3D&from=876277922"),
    ("[绿帽子]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/d23e058bab744349ac636c45e9a3687f?lk3s=343af0a2&x-expires=2108847600&x-signature=O%2F3nKDx%2BQpwuQXoEilxlVhgthAo%3D&from=876277922"),
    ("[敢怒不敢言]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/38b34096e69c41119b8d50cfd3a66882?lk3s=343af0a2&x-expires=2108847600&x-signature=yRED3i6D%2BeB0HPTck%2B33OZQiueE%3D&from=876277922"),
    ("[求求了]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/3b99deef770044d79182c18a854f4f27?lk3s=343af0a2&x-expires=2108847600&x-signature=rqQlfuao4DWfN%2BsFUkGCu6SNKTY%3D&from=876277922"),
    ("[眼含热泪]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/daf0e01ed3404b8e88898f690533bb76?lk3s=343af0a2&x-expires=2108847600&x-signature=S0PE3ko3QoAfGWdk4hHN8EGGCQY%3D&from=876277922"),
    ("[叹气]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/d7c541c323de4272acf862f77bb8b8a2?lk3s=343af0a2&x-expires=2108847600&x-signature=QZw8An2PUrksxFru2bYMR%2FwzqEo%3D&from=876277922"),
    ("[好开心]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/9b931359a63a42f693fdabe5934e95a9?lk3s=343af0a2&x-expires=2108847600&x-signature=N79LAQKQL3PwJ%2BrKJ7WlXTpDl%2Fo%3D&from=876277922"),
    ("[不是吧]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/a854fc87af894fddb1e36f97867c8169?lk3s=343af0a2&x-expires=2108847600&x-signature=7P9EPnj7BVICAc%2FlaMfy%2BQnB8pk%3D&from=876277922"),
    ("[鞠躬]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/93ffc02b07174d15a0d602b7a4cbb87f?lk3s=343af0a2&x-expires=2108847600&x-signature=X1lctk7SdgvRzoZzPxwq7t2mKi8%3D&from=876277922"),
    ("[躺平]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/25625ce0678448fc9c556bd1ebcf638c?lk3s=343af0a2&x-expires=2108847600&x-signature=EHR8fcSySgMoMSBjm0YkZteB4Y8%3D&from=876277922"),
    ("[九转大肠]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/ea89e25c77dd423d8403d20740f23bb7?lk3s=343af0a2&x-expires=2108847600&x-signature=8CpK9rMLBl81ezTyVkZwUmJY38A%3D&from=876277922"),
    ("[不你不想]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/b4845baf417a404488e955464208cc3c?lk3s=343af0a2&x-expires=2108847600&x-signature=XBriLgt8995vyv%2FD7yl81EbCy4w%3D&from=876277922"),
    ("[一头乱麻]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/db393c6b9bba40f3b37d4e079d1cbf1e?lk3s=343af0a2&x-expires=2108847600&x-signature=YbVh5635cl25UZBc%2FkJ82fBTiwI%3D&from=876277922"),
    ("[kisskiss]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/f971f6f859b24829a15196c5a0738df1?lk3s=343af0a2&x-expires=2108847600&x-signature=8TF7cRtfM7sb12d6mof4kzcml5E%3D&from=876277922"),
    ("[你不大行]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/0f2d07ea48a94bf0a57818056392bb42?lk3s=343af0a2&x-expires=2108847600&x-signature=5AGs43Ur681mfCBefECULjxXTOA%3D&from=876277922"),
    ("[噢买尬]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/af75834bf58e44c88b1825f85d50b7da?lk3s=343af0a2&x-expires=2108847600&x-signature=or0YMCy2jsgX4KgJYCuJt6ocaaY%3D&from=876277922"),
    ("[宕机]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/561341b97e7844148b6f040241357122?lk3s=343af0a2&x-expires=2108847600&x-signature=DYekA5xnm363HYYfeLixl8sdBIY%3D&from=876277922"),
    ("[苦涩]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/e72ce46a6ab640b38c7686f4b77fb7e4?lk3s=343af0a2&x-expires=2108847600&x-signature=4v7TPzBWDV%2FX7WmK9CgDNjx9X4g%3D&from=876277922"),
    ("[逞强落泪]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/67b9bcd2e1d64888bf57fa433e5b4717?lk3s=343af0a2&x-expires=2108847600&x-signature=AOmvUm3nno1C2tKS7WEMN7fHYV0%3D&from=876277922"),
    ("[求机位-黄脸]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/66e8109018b64af1b87ce68ff2bfc18e?lk3s=343af0a2&x-expires=2108847600&x-signature=2D4sZMgkcICbCfrQYfrBjnuP3nk%3D&from=876277922"),
    ("[求机位3]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/ec3d99650c114cb49f22cda9ce06d25f?lk3s=343af0a2&x-expires=2108847600&x-signature=q%2FCIlB5J8m%2BY05qdR16NPrxMkgc%3D&from=876277922"),
    ("[点赞]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/8e7e1b35d32b4623a0240552dc784b0f?lk3s=343af0a2&x-expires=2108847600&x-signature=f%2BsNjKzQNhWWO0J8ZGKcCZZcjNE%3D&from=876277922"),
    ("[精选]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/8d72402dd91a4a3a8cf4591b266ddb14?lk3s=343af0a2&x-expires=2108847600&x-signature=c%2B6iULoI2%2Bwx0XaeV1rpADuTuJE%3D&from=876277922"),
    ("[强壮]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/41a493d326e7475fb8f1ef0c614dbf25?lk3s=343af0a2&x-expires=2108847600&x-signature=yFIgy%2FBzhIi%2Fv9Zp0UpAlzirgKE%3D&from=876277922"),
    ("[碰拳]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/2bba2a8b361e467288d796c1581fec60?lk3s=343af0a2&x-expires=2108847600&x-signature=fEZUNErm8%2FfnA%2Bhv8yKaYaXp9aE%3D&from=876277922"),
    ("[OK]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/28dd2b330ea74d4cac2388f617177fe1?lk3s=343af0a2&x-expires=2108847600&x-signature=fPfUeA2UpgqHsnXFXYCRZrbW3Rg%3D&from=876277922"),
    ("[击掌]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/d3f83268e073468fbe8759638beb42ef?lk3s=343af0a2&x-expires=2108847600&x-signature=uoRoyN4Q%2Fht4mOucGB3Z1DE3h%2F0%3D&from=876277922"),
    ("[左上]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/cf670dd2a4c2487181ee834aa0b048fa?lk3s=343af0a2&x-expires=2108847600&x-signature=ZUOtG9bCGwHZGU1jk548vBNOEec%3D&from=876277922"),
    ("[握手]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/4550a48b841144d7b37fff3d02ec8a25?lk3s=343af0a2&x-expires=2108847600&x-signature=19jEh2tuk%2BdSRn%2FeFUe1TdjQ2aQ%3D&from=876277922"),
    ("[抱拳]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/5c6a7e331b7441ecb615e0adab43935a?lk3s=343af0a2&x-expires=2108847600&x-signature=sY0khO7VMA%2F6ZZHSuYvR%2FaK79ac%3D&from=876277922"),
    ("[勾引]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/6db49f314c1d4754a8205c3231edd396?lk3s=343af0a2&x-expires=2108847600&x-signature=JdHHoevD2xrjUrMVZ5XzVUXya8c%3D&from=876277922"),
    ("[拳头]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/5bede8c4134b4be798c924faee784650?lk3s=343af0a2&x-expires=2108847600&x-signature=md10t13H7fQXlCyYg4ctQT5HsJ8%3D&from=876277922"),
    ("[弱]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/5028f8d3638a488b987008c384745afc?lk3s=343af0a2&x-expires=2108847600&x-signature=22Eosu14p9ug6tVuj9DU%2FckYHoQ%3D&from=876277922"),
    ("[胜利]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/85abaf2fa4b940879a7b3bfdb45c3299?lk3s=343af0a2&x-expires=2108847600&x-signature=qVYcWI0RETysMCq7El6aTSaVCtE%3D&from=876277922"),
    ("[右边]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/11aadc9ed7b643279c38f94e8ed347c0?lk3s=343af0a2&x-expires=2108847600&x-signature=dfWPBDd2QFkgjjcG4CXv4PMQJ4A%3D&from=876277922"),
    ("[左边]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/bed344e70c214bf281db6a7944ef909a?lk3s=343af0a2&x-expires=2108847600&x-signature=M0Xb6RbqVAXoJhTS6IAfeoQYIEQ%3D&from=876277922"),
    ("[嘴唇]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/b1b3824e7b6c482b8d52b1c3b5577849?lk3s=343af0a2&x-expires=2108847600&x-signature=1k4rhIs5PbjCYWwD04K67QGVF98%3D&from=876277922"),
    ("[心碎]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/371bb866809f454597ef538f604bd25a?lk3s=343af0a2&x-expires=2108847600&x-signature=E%2Fse0qLJVHgx%2FLHbx8Kf71Zs0aQ%3D&from=876277922"),
    ("[凋谢]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/c93a47ad594b4be3a8c288aff0d7543b?lk3s=343af0a2&x-expires=2108847600&x-signature=RM%2BOxncvEqiRXWAh8ax%2BvBbWyNE%3D&from=876277922"),
    ("[愤怒]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/6dd1d010b5fd4f5197c1ecad3ce6796b?lk3s=343af0a2&x-expires=2108847600&x-signature=qc1cNlOCVSPGXsTsjTF6Gy8XXCc%3D&from=876277922"),
    ("[垃圾]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/d7d09280091445aabfd912bcca4d9a89?lk3s=343af0a2&x-expires=2108847600&x-signature=j6eHCppW%2Fmfv3Ttm1gLgjrZR3s4%3D&from=876277922"),
    ("[啤酒]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/a8ae1c4c2e0f467dbff1ba0964e0175d?lk3s=343af0a2&x-expires=2108847600&x-signature=EsoqPdV77dpEGx21Xmu2iGrjM2M%3D&from=876277922"),
    ("[咖啡]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/dcbcf5d7ca07490a950a37ff66a5be74?lk3s=343af0a2&x-expires=2108847600&x-signature=QC9U0OL0v%2BmfjYh0XI1xe0XGpVQ%3D&from=876277922"),
    ("[蛋糕]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/1153a12fbb514977b3a5349d3dcc509b?lk3s=343af0a2&x-expires=2108847600&x-signature=7z0FAOtqHRPO8mkbwWxFeHDMIKk%3D&from=876277922"),
    ("[礼物]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/e8e086e0f6544f9b87117abebd96e840?lk3s=343af0a2&x-expires=2108847600&x-signature=utpLXsv1SLy1PcXnJygfV%2BMEQ%2Bo%3D&from=876277922"),
    ("[撒花]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/8cefd1b8c16b412298657b6e1d81f8ab?lk3s=343af0a2&x-expires=2108847600&x-signature=K3nDIvF3A8lDHxM7jRLQWjCHtVA%3D&from=876277922"),
    ("[加一]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/496076e82fd74be793ab16d2940e8f8c?lk3s=343af0a2&x-expires=2108847600&x-signature=b%2FA5RrDjJcRvF5tTPiSodTfKYns%3D&from=876277922"),
    ("[减一]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/ffc25006163e4ecfa70968744d1be61e?lk3s=343af0a2&x-expires=2108847600&x-signature=6sWvqB651FNHwvBSkHPAF0V4Y8s%3D&from=876277922"),
    ("[okk]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/9309aa3f9bed475b88538346f56fc6e6?lk3s=343af0a2&x-expires=2108847600&x-signature=ktn2XlAn0KKLwKsnH0Kkp9wsoeQ%3D&from=876277922"),
    ("[V5]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/cddffb522b2d421d81eb9da0f7f3cbb1?lk3s=343af0a2&x-expires=2108847600&x-signature=j2t4Gkj3PMSHK61BUpfI67MpYOY%3D&from=876277922"),
    ("[绝]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/f1e1a02468b7420c841733c780ec2576?lk3s=343af0a2&x-expires=2108847600&x-signature=IZbSNChE996Ax4IuC2ZhwUTkOKE%3D&from=876277922"),
    ("[给力]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/5387fc1361634b579946fbac7c4ea5f4?lk3s=343af0a2&x-expires=2108847600&x-signature=29FxnjxzvzGONwUD9M%2FrXKg0n5k%3D&from=876277922"),
    ("[红包]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/3e78d79aa4964404bb78b445f34c78fd?lk3s=343af0a2&x-expires=2108847600&x-signature=NDm%2B9dF8XMO3PQN3cvJ5BzrtCKY%3D&from=876277922"),
    ("[屎]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/eae0730620d644bdbb91985bd9cfff4e?lk3s=343af0a2&x-expires=2108847600&x-signature=Cv9WvbBFbUN%2B6%2B9XEroV7XWEJJo%3D&from=876277922"),
    ("[发]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/ee4f3e944a62429e8e963bd187d00536?lk3s=343af0a2&x-expires=2108847600&x-signature=j2xa0J1c90M%2FMqrVHPXMFMBRacc%3D&from=876277922"),
    ("[18禁]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/40724dd5538c474693ec54d34a14c613?lk3s=343af0a2&x-expires=2108847600&x-signature=w%2Fb6KZHuL8ZYhzh%2BMDiWPktVgO0%3D&from=876277922"),
    ("[炸弹]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/a309a1e9a10049d88b1c9848f5f7b287?lk3s=343af0a2&x-expires=2108847600&x-signature=kt54%2F1ixtZvOjQ3nmT6iT7gtSkQ%3D&from=876277922"),
    ("[西瓜]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/39ca333313da4187bd5651a5404aed26?lk3s=343af0a2&x-expires=2108847600&x-signature=qCIAGP5Q3e59ECg%2Fhx7ixZ%2BlxdY%3D&from=876277922"),
    ("[加鸡腿]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/1391aac43d004af986f2202f43253f94?lk3s=343af0a2&x-expires=2108847600&x-signature=voCmRijZwLhtdkSNTK4jLZq9fkY%3D&from=876277922"),
    ("[握爪]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/966210d6153146e8b58686aa1cc763ca?lk3s=343af0a2&x-expires=2108847600&x-signature=EIiMWHC76YYnqlbylMqfA%2BTq5ko%3D&from=876277922"),
    ("[太阳]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/5c9e5b67fc524f14b4ae49b5977afd9d?lk3s=343af0a2&x-expires=2108847600&x-signature=5Wu2Irg0zpFEycXaiGL57HRMDD0%3D&from=876277922"),
    ("[月亮]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/cfa03e7f7b534eb2a9bc16fec54ed23f?lk3s=343af0a2&x-expires=2108847600&x-signature=3cy8XQDEIZKCaxjIZLmkM9ac1GU%3D&from=876277922"),
    ("[给跪了]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/21d2a8bbe630496d88c03fa9179b17cb?lk3s=343af0a2&x-expires=2108847600&x-signature=Za2hseHmN55pX%2BggloPINRs5iQc%3D&from=876277922"),
    ("[蕉绿]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/ac2d04da5bf54c8bb12628510d8718ba?lk3s=343af0a2&x-expires=2108847600&x-signature=MQME%2Fl1xijTGdEl6cGjFy1Iz7QA%3D&from=876277922"),
    ("[扎心]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/e6121879b6de4bd08d35a312574f6563?lk3s=343af0a2&x-expires=2108847600&x-signature=VpSDYa8m9TX1%2FTsOvtsz3xVO9gM%3D&from=876277922"),
    ("[胡瓜]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/5f3bb76347cd49e4935300b507353f68?lk3s=343af0a2&x-expires=2108847600&x-signature=362Ey77j9qZzAbyIUje14AYdC2A%3D&from=876277922"),
    ("[打call]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/31a1efcf7c97414fae43d1d08058b0ee?lk3s=343af0a2&x-expires=2108847600&x-signature=5y5IWt8uVuhG1EsELwbGGgW1III%3D&from=876277922"),
    ("[栓Q]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/a948fa11ae344a1d91276235ab6bf70b?lk3s=343af0a2&x-expires=2108847600&x-signature=w0mcePRC2uOeq%2Buomej0rIHEjH8%3D&from=876277922"),
    ("[雪花]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/5144febc02ad42c082b83980d9159e13?lk3s=343af0a2&x-expires=2108847600&x-signature=tGBIB%2BvRFa3gk8RSPeFjDZj8Qcc%3D&from=876277922"),
    ("[圣诞树]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/9d3184f19a0042f2886922fd6d1b6823?lk3s=343af0a2&x-expires=2108847600&x-signature=2HOnhceXis0PcKrdGa3WE8rzQ0g%3D&from=876277922"),
    ("[平安果]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/0855a7b63f1f437db4eaa3519d5dd4d5?lk3s=343af0a2&x-expires=2108847600&x-signature=7TVBPMk5QWx%2B2d0pzu1hsBBcT7o%3D&from=876277922"),
    ("[圣诞帽]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/71d0ea5435d443519eafc2d72eee07c7?lk3s=343af0a2&x-expires=2108847600&x-signature=WPoO93KzCsxRzPVzqU90i9zR1MU%3D&from=876277922"),
    ("[气球]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/2eb779c7734345979bd3f866989a167f?lk3s=343af0a2&x-expires=2108847600&x-signature=ymnC%2F0Y7hKEa%2Bdz0C1vc17Q469U%3D&from=876277922"),
    ("[烟花]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/e679017985b54d19b23668fc3437a1a1?lk3s=343af0a2&x-expires=2108847600&x-signature=WsjujANM%2FlVWhhTD%2F6sremv2r2I%3D&from=876277922"),
    ("[福]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/c5dd439d89754e4aa6c553a244aacc4b?lk3s=343af0a2&x-expires=2108847600&x-signature=eYEPLjPTf6AAwfPwPrI6Wa00eLw%3D&from=876277922"),
    ("[candy]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/cce110c8f9f74a898a21ab413fb6cc63?lk3s=343af0a2&x-expires=2108847600&x-signature=uNgcBJhvo3Lz1QKsWluGuBuBU4E%3D&from=876277922"),
    ("[糖葫芦]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/f99e62388e6c4553974f1fe44f602456?lk3s=343af0a2&x-expires=2108847600&x-signature=63JgRVOyznhcNuHhp3vZ%2BMQCSfc%3D&from=876277922"),
    ("[鞭炮]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/237a828fd0964b2b9510c15de33383ad?lk3s=343af0a2&x-expires=2108847600&x-signature=KhRyZv7ek7WsFQ4gwde66YsnYDA%3D&from=876277922"),
    ("[元宝]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/6c0ebb50021a4b9e84cb5908be4eb5ef?lk3s=343af0a2&x-expires=2108847600&x-signature=si4ZsQkcbX2t8vd74MMV21jRNQU%3D&from=876277922"),
    ("[灯笼]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/a7a43e8805fc425990fed26d8be5b7b6?lk3s=343af0a2&x-expires=2108847600&x-signature=hMIW42ifZbTvffxYXaArjdOE%2F%2BI%3D&from=876277922"),
    ("[锦鲤]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/f88e3e8d639b438aaa084549086fbb46?lk3s=343af0a2&x-expires=2108847600&x-signature=sWygXWi2ku6XvCy8eC6IeltDjVQ%3D&from=876277922"),
    ("[巧克力]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/c6db9180292c47779624a865d80f67e7?lk3s=343af0a2&x-expires=2108847600&x-signature=%2FyUB2jPX2E5ArmifL%2BGFWpOSNbo%3D&from=876277922"),
    ("[戒指]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/f8eb6c5b5d6640cc82de8ecd4acd33d0?lk3s=343af0a2&x-expires=2108847600&x-signature=RiMVMrHAIuENSVJRnEKWuc04M%2FI%3D&from=876277922"),
    ("[棒棒糖]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/38fd079e306a40e4953bdf4a5d8a16db?lk3s=343af0a2&x-expires=2108847600&x-signature=x2Hod2MH%2FQPV7%2B0uBa9UNon%2BksY%3D&from=876277922"),
    ("[纸飞机]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/c0e6de9ee9d646d0a0d01831f3870787?lk3s=343af0a2&x-expires=2108847600&x-signature=NsWAh77opYXvB%2BLd1U69qaFnQyc%3D&from=876277922"),
    ("[粽子]", "https://p3-pc-sign.douyinpic.com/obj/tos-cn-i-tsj2vxp0zn/ce5e59e1376d440b98bb738c90e1a6e1?lk3s=343af0a2&x-expires=2108847600&x-signature=A9b3Y1r%2FY5Il50auC1wRPNYzSqU%3D&from=876277922"),
];

static STATIC_TABLE: Lazy<DouyinEmoticonTable> = Lazy::new(|| {
    DOUYIN_EMOTICONS
        .iter()
        .map(|(name, url)| ((*name).to_string(), (*url).to_string()))
        .collect()
});

/// 内置静态表（只读单例）。
pub fn static_emoticons() -> &'static DouyinEmoticonTable {
    &STATIC_TABLE
}

/// 内置静态表副本，供会话合并动态表。
pub fn static_table() -> DouyinEmoticonTable {
    static_emoticons().clone()
}

/// 拉取线上表情表；失败 / 超时 / 结构变化一律回退静态表，不阻断弹幕主链路。
///
/// 接口返回的签名 URL 会随 `x-expires` 刷新，因此运行时拉取比内置表更新；
/// 静态表只在接口不可用时兜底（地址有效期到 2036 年，短期内不会失效）。
pub async fn fetch(http: Arc<dyn HttpTransport>) -> DouyinEmoticonTable {
    if let Some(table) = cached_table() {
        return table;
    }
    fetch_uncached(http).await
}

/// 绕过缓存的真实拉取（缓存命中 / 测试直达都用它下面这一层）。
async fn fetch_uncached(http: Arc<dyn HttpTransport>) -> DouyinEmoticonTable {
    let request = HttpRequest {
        url: EMOJI_LIST_URL.to_string(),
        query: Vec::new(),
        headers: vec![
            ("user-agent".to_string(), DEFAULT_USER_AGENT.to_string()),
            ("referer".to_string(), REFERER.to_string()),
        ],
    };
    let reply = tokio::time::timeout(FETCH_TIMEOUT, http.get(&request)).await;
    let mut table = static_table();
    match reply {
        Err(_) => log::warn!("[douyin-danmaku] 表情表接口超时，回退内置静态表"),
        Ok(Err(e)) => log::warn!("[douyin-danmaku] 表情表拉取失败，回退内置静态表: {e}"),
        Ok(Ok(reply)) => match reply.json() {
            Ok(value) => match parse_emoji_list(&value) {
                Some(dynamic) => {
                    // 动态表覆盖同名静态项；静态表独有、动态表漏发的码仍可解析。
                    table.extend(dynamic);
                    store_cache(&table);
                    log::info!("[douyin-danmaku] 已加载 {} 个标准表情", table.len());
                }
                None => log::warn!("[douyin-danmaku] 表情表结构不符，回退内置静态表"),
            },
            Err(e) => log::warn!("[douyin-danmaku] 表情表 JSON 解析失败，回退内置静态表: {e}"),
        },
    }
    table
}

/// 命中未过期缓存时返回一份副本（调用方会跨线程持有，不能借用缓存本体）。
fn cached_table() -> Option<DouyinEmoticonTable> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let (fetched_at, table) = guard.as_ref()?;
    now.saturating_sub(*fetched_at)
        .lt(&CACHE_TTL.as_secs())
        .then(|| table.clone())
}

/// 写入缓存（仅在接口成功时调用）。
fn store_cache(table: &DouyinEmoticonTable) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some((now, table.clone()));
}

/// 解析 `aweme/v1/web/emoji/list` 响应；`status_code != 0` 或列表为空 -> `None`。
///
/// - 过滤 `hide == 1`（网页端隐藏的表情不下发给用户）；
/// - 只接受 `[xxx]` 形态的 `display_name`（正文里出现的正是这个码）；
/// - URL 取 `emoji_url.url_list[0]`，统一升 https；
/// - 同名重复保留首个（接口当前无重复，防御未来变动）。
fn parse_emoji_list(value: &Value) -> Option<DouyinEmoticonTable> {
    if value.get("status_code").and_then(Value::as_i64) != Some(0) {
        return None;
    }
    let list = value.get("emoji_list")?.as_array()?;
    let mut table = DouyinEmoticonTable::with_capacity(list.len());
    for item in list {
        if item.get("hide").and_then(Value::as_i64).unwrap_or(0) == 1 {
            continue;
        }
        let Some(name) = item
            .get("display_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| name.starts_with('[') && name.ends_with(']') && name.len() > 2)
        else {
            continue;
        };
        let Some(url) = item
            .get("emoji_url")
            .and_then(|emoji| emoji.get("url_list"))
            .and_then(Value::as_array)
            .and_then(|urls| urls.iter().find_map(|url| url.as_str()))
            .and_then(normalize_url)
        else {
            continue;
        };
        table.entry(name.to_string()).or_insert(url);
    }
    if table.is_empty() {
        None
    } else {
        Some(table)
    }
}

/// `http://` / 协议相对地址统一升 https；其它形态视为不可用。
fn normalize_url(raw: &str) -> Option<String> {
    let url = raw.trim();
    if let Some(rest) = url.strip_prefix("https://") {
        return Some(format!("https://{rest}"));
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return Some(format!("https://{rest}"));
    }
    url.strip_prefix("//").map(|rest| format!("https://{rest}"))
}

/// 从聊天正文提取标准表情码（使用内置静态表）；无命中 -> None。
pub fn extract_emoticons(content: &str) -> Option<Vec<LiveMessageEmoticon>> {
    extract_emoticons_with(content, static_emoticons())
}

/// 从聊天正文提取标准表情码；`table` 由会话按接口注入。
pub fn extract_emoticons_with(
    content: &str,
    table: &DouyinEmoticonTable,
) -> Option<Vec<LiveMessageEmoticon>> {
    let mut result = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 1;
        while j < bytes.len() && bytes[j] != b'[' && bytes[j] != b']' {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b']' && j > start + 1 {
            let token = &content[start..=j];
            if let Some(url) = table.get(token) {
                result.push(LiveMessageEmoticon {
                    name: Some(token.to_string()),
                    url: url.clone(),
                    width: None,
                    height: None,
                    large: false,
                    emoticon_unique: None,
                });
            }
            i = j + 1;
        } else {
            i = start + 1;
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockTransport;
    use serde_json::json;

    #[test]
    fn table_size_matches_web_panel() {
        assert_eq!(DOUYIN_EMOTICONS.len(), 214, "表情数量与网页端一致");
        assert!(static_emoticons().contains_key("[呲牙]"));
        assert!(static_emoticons().contains_key("[互粉]"));
    }

    #[test]
    fn extracts_known_codes_in_order() {
        let got = extract_emoticons("哈喽[微笑][赞]").expect("应提取到表情");
        let names: Vec<&str> = got.iter().filter_map(|e| e.name.as_deref()).collect();
        assert_eq!(names, vec!["[微笑]", "[赞]"]);
        assert!(got.iter().all(|e| e.url.starts_with("https://")));
        assert!(got.iter().all(|e| !e.large));
    }

    #[test]
    fn keeps_duplicates_and_ignores_unknown_or_plain_brackets() {
        let got = extract_emoticons("[呲牙][呲牙] [不存在] 普通文本").expect("应提取到两个");
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|e| e.name.as_deref() == Some("[呲牙]")));
        assert!(extract_emoticons("[不存在] 普通文本").is_none());
        assert!(extract_emoticons("没有方括号").is_none());
    }

    #[test]
    fn parses_emoji_list_and_skips_hidden_or_malformed_items() {
        let value = json!({
            "status_code": 0,
            "emoji_list": [
                {
                    "display_name": "[微笑]",
                    "hide": 0,
                    "emoji_url": {"url_list": ["http://cdn/a.png"]}
                },
                {
                    "display_name": "[隐藏]",
                    "hide": 1,
                    "emoji_url": {"url_list": ["https://cdn/b.png"]}
                },
                {
                    "display_name": "无方括号",
                    "hide": 0,
                    "emoji_url": {"url_list": ["https://cdn/c.png"]}
                },
                {
                    "display_name": "[无图]",
                    "hide": 0,
                    "emoji_url": {"url_list": []}
                },
                {
                    "display_name": "[微笑]",
                    "hide": 0,
                    "emoji_url": {"url_list": ["https://cdn/dup.png"]}
                }
            ]
        });
        let table = parse_emoji_list(&value).expect("应解析出表");
        assert_eq!(table.len(), 1, "隐藏 / 非方括号 / 无图 / 重复都应剔除");
        assert_eq!(
            table.get("[微笑]").map(String::as_str),
            Some("https://cdn/a.png")
        );
    }

    #[test]
    fn parse_rejects_error_status_or_empty_list() {
        assert!(parse_emoji_list(&json!({"status_code": 1, "emoji_list": []})).is_none());
        assert!(parse_emoji_list(&json!({"status_code": 0, "emoji_list": []})).is_none());
        assert!(parse_emoji_list(&json!({"status_code": 0})).is_none());
    }

    #[test]
    fn extract_with_dynamic_table_uses_its_urls() {
        let mut table = DouyinEmoticonTable::new();
        table.insert("[动态]".to_string(), "https://cdn/dynamic.png".to_string());
        let got = extract_emoticons_with("hi[动态]", &table).expect("应提取到动态表情");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].url, "https://cdn/dynamic.png");
        assert!(
            extract_emoticons_with("[微笑]", &table).is_none(),
            "动态表未命中的码不猜"
        );
    }

    #[tokio::test]
    async fn fetch_uses_api_table_when_available() {
        let transport = MockTransport::new();
        transport.stub(
            EMOJI_LIST_URL,
            &json!({
                "status_code": 0,
                "emoji_list": [{
                    "display_name": "[微笑]",
                    "hide": 0,
                    "emoji_url": {"url_list": ["https://cdn/live.png"]}
                }]
            })
            .to_string(),
        );
        let table = fetch_uncached(Arc::new(transport)).await;
        assert_eq!(
            table.get("[微笑]").map(String::as_str),
            Some("https://cdn/live.png"),
            "同名静态项必须被接口下发的新签名 URL 覆盖"
        );
        assert!(table.contains_key("[呲牙]"), "动态表漏发的码仍由静态表补齐");
    }

    #[tokio::test]
    async fn fetch_falls_back_to_static_table_on_error() {
        let transport = MockTransport::new();
        let table = fetch_uncached(Arc::new(transport)).await;
        assert_eq!(
            table.len(),
            DOUYIN_EMOTICONS.len(),
            "接口失败时回退内置静态表"
        );
        assert!(table.contains_key("[呲牙]"));
    }

    #[tokio::test]
    async fn fetch_serves_from_cache_within_ttl() {
        // 缓存是进程级全局状态，与其他测试共享；先清空再按「成功 -> 命中」验证，
        // 用例结束前再清空，避免把一张假表留给后续用例。
        clear_cache();
        let transport = MockTransport::new();
        transport.stub(
            EMOJI_LIST_URL,
            &json!({
                "status_code": 0,
                "emoji_list": [{
                    "display_name": "[缓存]",
                    "hide": 0,
                    "emoji_url": {"url_list": ["https://cdn/cached.png"]}
                }]
            })
            .to_string(),
        );
        let first = fetch(Arc::new(transport)).await;
        assert_eq!(
            first.get("[缓存]").map(String::as_str),
            Some("https://cdn/cached.png"),
            "首次应真实拉取并写入缓存"
        );

        // 第二次换成一个必然失败的传输：命中缓存就不该再发请求。
        let second = fetch(Arc::new(MockTransport::new())).await;
        assert_eq!(
            second.get("[缓存]").map(String::as_str),
            Some("https://cdn/cached.png"),
            "TTL 内应直接命中缓存，不再发请求"
        );
        clear_cache();
    }

    /// 清空进程级缓存（测试专用）。
    fn clear_cache() {
        let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
}
