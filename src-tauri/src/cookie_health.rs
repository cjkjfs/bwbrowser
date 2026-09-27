//! Platform cookie validity checker.
//!
//! Probes each configured platform by navigating to its main URL with the
//! profile's cookies attached. A response that is a normal page (not a
//! redirect to a login screen) means the cookie is still valid.
//!
//! The check is fire-and-forget: launched as a detached async task so it
//! never blocks the browser launch itself.

use crate::cookie_manager::CookieManager;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// How long one platform probe may take before giving up.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long between two consecutive runs for the same profile.
const COOLDOWN_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
  /// All indicators point to a live session — the cookie is good.
  Valid,
  /// The response was reachable but something in it said "you need to log in".
  Expired,
  /// The cookie is missing for this domain entirely.
  Missing,
  /// We couldn't tell because the network was unavailable or the request failed.
  Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieHealthResult {
  pub profile_id: String,
  pub profile_name: String,
  pub domain: String,
  pub url: String,
  pub status: HealthStatus,
  pub message: String,
}

/// One platform to probe.
#[derive(Debug, Clone)]
pub struct PlatformRule {
  /// Stable key used to look the rule up from a platform name (e.g. "douyin").
  pub key: &'static str,
  /// The URL the probe navigates to.
  pub url: &'static str,
  /// Cookie domains that belong to this platform. A stored cookie whose domain
  /// equals one of these, or is a subdomain of one, is attached to the probe.
  pub domains: &'static [&'static str],
  /// Login-session cookie names (aligned with `cookie_sync::has_login_cookies`).
  /// At least two, present and unexpired, mean the session is valid. This is
  /// the primary verdict — SPA platforms (douyin, tiktok, …) render the same
  /// HTML shell to logged-in and logged-out visitors, so response-body probing
  /// cannot distinguish them and would misreport valid sessions as expired.
  pub session_cookies: &'static [&'static str],
  /// Whether to additionally confirm with a live HTTP probe. Only enabled for
  /// platforms whose server-rendered HTML reliably separates login states
  /// (YouTube is verified; the rest of the catalog keeps this off).
  pub network_probe: bool,
  /// Response body substrings that mean "you are logged in".
  pub session_indicators: &'static [&'static str],
  /// Response body substrings that mean "you are NOT logged in".
  pub login_indicators: &'static [&'static str],
  /// HTTP status codes that are considered a successful page load.
  pub ok_status_range: (u16, u16),
}

/// Look up a platform rule by its key. Case-insensitive, with a few aliases
/// ("twitter" -> "x"). Returns None when the platform has no probe rule, so
/// callers can silently fall back to their existing logic.
pub fn rule_for_platform(platform: &str) -> Option<&'static PlatformRule> {
  let key = platform.trim().to_lowercase();
  let canonical = match key.as_str() {
    "twitter" | "tw" => "x",
    "youtube" | "yt" => "youtube",
    "douyincn" | "douyin" | "抖音" | "dy" | "douyin_hao" | "抖音号" => "douyin",
    "tiktok" | "tk" | "tiktok us" => "tiktok",
    "toutiao" | "头条" | "今日头条" | "tt" => "toutiao",
    "netease" | "网易" | "网易邮箱" | "mail_163" => "netease",
    "bilibili" | "b站" | "哔哩哔哩" => "bilibili",
    "weibo" | "微博" | "wb" | "wb_zt" => "weibo",
    "xiaohongshu" | "小红书" | "xhs" | "little_red_book" => "xiaohongshu",
    "kuaishou" | "快手" => "kuaishou",
    "facebook" | "fb" => "facebook",
    "instagram" | "ig" => "instagram",
    "github" | "gh" => "github",
    "baijiahao" | "百度号" | "百家号" | "baidu" | "百度" => "baijiahao",
    "tieba" | "百度贴吧" => "tieba",
    "taobao" | "淘宝" => "taobao",
    "youku" | "优酷" => "youku",
    "jingdong" | "京东" | "jd" => "jingdong",
    "tengxun_video" | "腾讯视频" | "qq" | "qq视频" => "tengxun_video",
    "zhihu" | "知乎" | "zhihu_zhuanlan" | "知乎专栏" => "zhihu",
    "xueqiu" | "雪球" => "xueqiu",
    "pdd" | "拼多多" => "pdd",
    "csdn" => "csdn",
    "juejin" | "掘金" => "juejin",
    "wechat_mp" | "微信公众号" => "wechat_mp",
    "xigua" | "西瓜视频" => "xigua",
    "douyin_xingtu" | "巨量星图" => "douyin_xingtu",
    "dian" | "大众点评" | "点评" => "dian",
    "wechat_video" | "微信视频号" | "视频号" => "wechat_video",
    "iqiyi" | "爱奇艺" => "iqiyi",
    "sohu_video" | "sohu" | "搜狐" | "搜狐视频" => "sohu_video",
    "meituan" | "美团" => "meituan",
    "eleme" | "饿了么" => "eleme",
    "vipshop" | "vip" | "唯品会" => "vipshop",
    "suning" | "苏宁" => "suning",
    "dingtalk" | "钉钉" => "dingtalk",
    "mango_tv" | "芒果tv" | "芒果" | "芒果TV" => "mango_tv",
    "mogu_v" | "蘑菇街" => "mogu_v",
    "dangdang" | "当当" => "dangdang",
    "kaola" | "考拉" | "网易考拉" => "kaola",
    "yhd" | "一号店" => "yhd",
    "flyme" | "meizu" | "魅族" => "flyme",
    "oppo" => "oppo",
    "vivo" => "vivo",
    "huawei" | "华为" => "huawei",
    "xiaomi" | "小米" => "xiaomi",
    "360" => "360",
    "sina" | "新浪" => "sina",
    "eastmoney" | "东方财富" => "eastmoney",
    "wallstreetcn" | "华尔街见闻" => "wallstreetcn",
    "cls" | "财联社" => "cls",
    "yicai" | "第一财经" => "yicai",
    "ifeng" | "凤凰网" => "ifeng",
    "people" | "人民网" => "people",
    "cctv" | "央视网" => "cctv",
    "china" | "中国网" => "china",
    "gmw" | "光明网" => "gmw",
    "huanqiu" | "环球网" => "huanqiu",
    "eastday" | "东方网" => "eastday",
    "redstar" | "红星新闻" => "redstar",
    other => other,
  };
  PLATFORM_RULES.iter().find(|r| r.key == canonical)
}

static PLATFORM_RULES: LazyLock<Vec<PlatformRule>> = LazyLock::new(platform_rules);

fn platform_rules() -> Vec<PlatformRule> {
  vec![
    // YouTube — session cookies (SID family) plus a verified body probe.
    PlatformRule {
      key: "youtube",
      url: "https://www.youtube.com",
      domains: &["youtube.com", "google.com", "youtu.be"],
      session_cookies: &[
        "LOGIN_INFO",
        "SID",
        "__Secure-1PSID",
        "__Secure-3PSID",
        "SIDCC",
      ],
      network_probe: true,
      session_indicators: &["INNERTUBE_API_KEY", "clientScript"],
      login_indicators: &["signin", "login", "/accounts/signin"],
      ok_status_range: (200, 299),
    },
    // Douyin (Chinese TikTok) — session cookies are the only reliable signal;
    // the creator center serves an identical SPA shell to guests.
    PlatformRule {
      key: "douyin",
      url: "https://creator.douyin.com/",
      domains: &["douyin.com", "douyincdn.com", "amemv.com"],
      session_cookies: &["sessionid", "sessionid_ss", "sid_tt", "uid_tt"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // TikTok (international) — same session-cookie family as douyin.
    PlatformRule {
      key: "tiktok",
      url: "https://www.tiktok.com",
      domains: &["tiktok.com", "tiktokcdn.com", "tiktokv.com"],
      session_cookies: &["sessionid", "sessionid_ss", "sid_tt", "uid_tt"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Instagram — sessionid family.
    PlatformRule {
      key: "instagram",
      url: "https://www.instagram.com",
      domains: &["instagram.com", "cdninstagram.com"],
      session_cookies: &["sessionid", "ds_user_id", "csrftoken"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Twitter / X — auth_token + twid are set on login only.
    PlatformRule {
      key: "x",
      url: "https://x.com",
      domains: &["x.com", "twitter.com", "twimg.com"],
      session_cookies: &["auth_token", "twid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Facebook — c_user + xs are the classic logged-in pair.
    PlatformRule {
      key: "facebook",
      url: "https://www.facebook.com",
      domains: &["facebook.com", "fbcdn.net"],
      session_cookies: &["c_user", "xs", "fr", "sb"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // LinkedIn — li_at is the session bearer.
    PlatformRule {
      key: "linkedin",
      url: "https://www.linkedin.com",
      domains: &["linkedin.com", "licdn.com"],
      session_cookies: &["li_at", "li_rm"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Reddit — reddit_session plus the API token.
    PlatformRule {
      key: "reddit",
      url: "https://www.reddit.com",
      domains: &["reddit.com", "redd.it"],
      session_cookies: &["reddit_session", "token_v2"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Toutiao / Jinri Toutiao — sessionid family (mp.toutiao.com is the
    // creator console, SPA like douyin so no network probe).
    PlatformRule {
      key: "toutiao",
      url: "https://mp.toutiao.com/",
      domains: &["toutiao.com", "bytedance.com", "byteimg.com", "douyin.com"],
      session_cookies: &[
        "sessionid",
        "sessionid_ss",
        "sid_tt",
        "passport_csrf_token",
        "uid_tt_ss",
        "sid_guard",
      ],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Netease mail — NTES session cookies.
    PlatformRule {
      key: "netease",
      url: "https://mail.163.com/",
      domains: &["163.com", "126.com", "netease.com"],
      session_cookies: &[
        "P_INFO",
        "S_INFO",
        "NTES_SESS",
        "NTES_PASS",
        "MAIL_SINFO",
        "MAIL163_SINFO",
      ],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Outlook / Microsoft account — MS auth cookies.
    PlatformRule {
      key: "outlook",
      url: "https://outlook.live.com/mail/",
      domains: &[
        "outlook.com",
        "live.com",
        "office.com",
        "microsoft.com",
        "msn.com",
      ],
      session_cookies: &["MSCC", "MSPAuth", "MSAuth1", "RPSSecAuth"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Bilibili — SESSDATA is the session bearer.
    PlatformRule {
      key: "bilibili",
      url: "https://www.bilibili.com/",
      domains: &["bilibili.com", "bilivideo.com", "bilivideo.cn", "hdslb.com"],
      session_cookies: &["SESSDATA", "bili_jct", "DedeUserID", "DedeUserID__ckMd5"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Weibo — SUB family.
    PlatformRule {
      key: "weibo",
      url: "https://weibo.com/",
      domains: &["weibo.com", "weibo.cn", "sina.com.cn", "sinaimg.cn"],
      session_cookies: &["SUB", "SUBP", "SSOLockpin"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Xiaohongshu — web_session is the session bearer.
    PlatformRule {
      key: "xiaohongshu",
      url: "https://www.xiaohongshu.com/",
      domains: &["xiaohongshu.com", "xhscdn.com"],
      session_cookies: &["web_session", "xsecappid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Kuaishou — userId/passToken session pair.
    PlatformRule {
      key: "kuaishou",
      url: "https://www.kuaishou.com/",
      domains: &[
        "kuaishou.com",
        "gifshow.com",
        "kwaicdn.com",
        "yxixy.com",
        "chenzhongtech.com",
      ],
      session_cookies: &["userId", "passToken", "kuaishou.server.web_st"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // GitHub — logged_in/dotcom_user/user_session are set on login only.
    PlatformRule {
      key: "github",
      url: "https://github.com/",
      domains: &["github.com", "githubusercontent.com"],
      session_cookies: &["logged_in", "dotcom_user", "user_session"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Baidu / 百家号 — BDUSS family is set on passport login only.
    PlatformRule {
      key: "baijiahao",
      url: "https://baijiahao.baidu.com/",
      domains: &["baidu.com", "bdstatic.com", "bdimg.com"],
      session_cookies: &["BDUSS", "BDUSS_BFESS", "STOKEN"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Tieba — shares the Baidu passport SSO, so the same cookie family applies.
    PlatformRule {
      key: "tieba",
      url: "https://tieba.baidu.com/",
      domains: &["tieba.baidu.com", "baidu.com", "bdstatic.com"],
      session_cookies: &["BDUSS", "BDUSS_BFESS", "STOKEN"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Taobao — cookie2 + unb are the Alibaba login pair (guests never hold them).
    PlatformRule {
      key: "taobao",
      url: "https://www.taobao.com/",
      domains: &["taobao.com", "taobaoimg.com", "tmall.com"],
      session_cookies: &["cookie2", "unb"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Youku — Alibaba passport, same cookie2/unb login pair.
    PlatformRule {
      key: "youku",
      url: "https://www.youku.com/",
      domains: &["youku.com", "ykimg.com", "youkutv.com"],
      session_cookies: &["cookie2", "unb"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // JD — pt_key/pt_pin are the classic login pair.
    PlatformRule {
      key: "jingdong",
      url: "https://www.jd.com/",
      domains: &["jd.com", "jdstatic.com", "360buyimg.com", "paipai.com"],
      session_cookies: &["pt_key", "pt_pin", "wlfstk_smdl"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Tencent Video / QQ — uin + skey are set on QQ login.
    PlatformRule {
      key: "tengxun_video",
      url: "https://v.qq.com/",
      domains: &["qq.com", "tencent.com", "qpic.cn", "gtimg.com"],
      session_cookies: &["uin", "skey", "p_skey"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Zhihu — z_c0 is the login bearer; d_c0 is the always-present device cookie,
    // so a guest (z_c0 absent) can never reach the two-cookie threshold.
    PlatformRule {
      key: "zhihu",
      url: "https://www.zhihu.com/",
      domains: &["zhihu.com", "zhimg.cn"],
      session_cookies: &["z_c0", "d_c0"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Xueqiu — xq_a_token/xq_r_token are set on login.
    PlatformRule {
      key: "xueqiu",
      url: "https://xueqiu.com/",
      domains: &["xueqiu.com", "xueqiu.net"],
      session_cookies: &["xq_a_token", "xq_r_token"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Pinduoduo — pdd_user_id/pdd_user_uin are set on login.
    PlatformRule {
      key: "pdd",
      url: "https://www.pinduoduo.com/",
      domains: &["pinduoduo.com", "yangkeduo.com", "pddpic.com"],
      session_cookies: &["pdd_user_id", "pdd_user_uin"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // CSDN — UserName/UserInfo are the login pair.
    PlatformRule {
      key: "csdn",
      url: "https://www.csdn.net/",
      domains: &["csdn.net", "csdn.com"],
      session_cookies: &["UserName", "UserInfo"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Juejin — sessionid/sessionid.sig are the login pair.
    PlatformRule {
      key: "juejin",
      url: "https://juejin.cn/",
      domains: &["juejin.cn", "juejin.com"],
      session_cookies: &["sessionid", "sessionid.sig"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // WeChat MP (公众号平台) — slave_sid/slave_user are the session pair.
    PlatformRule {
      key: "wechat_mp",
      url: "https://mp.weixin.qq.com/",
      domains: &["mp.weixin.qq.com", "weixin.qq.com", "wx.qq.com"],
      session_cookies: &["slave_sid", "slave_user", "token"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Xigua Video — Bytedance passport session family.
    PlatformRule {
      key: "xigua",
      url: "https://www.ixigua.com/",
      domains: &["ixigua.com", "toutiao.com", "bytedance.com", "douyin.com"],
      session_cookies: &["sessionid", "sessionid_ss", "sid_tt", "uid_tt"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 巨量星图 — same Bytedance passport session family.
    PlatformRule {
      key: "douyin_xingtu",
      url: "https://www.xingtu.cn/",
      domains: &[
        "douyin.com",
        "bytedance.com",
        "oceanengine.com",
        "xingtu.cn",
      ],
      session_cookies: &["sessionid", "sessionid_ss", "sid_tt", "uid_tt"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Dianping — dper/ua are the login pair (Meituan unified account).
    PlatformRule {
      key: "dian",
      url: "https://www.dianping.com/",
      domains: &["dianping.com", "51ping.com", "meituan.com"],
      session_cookies: &["dper", "ua"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 微信视频号 — shares the WeChat web SSO with 公众平台, same cookie family.
    PlatformRule {
      key: "wechat_video",
      url: "https://channels.weixin.qq.com/",
      domains: &["weixin.qq.com", "wx.qq.com", "tenpay.com"],
      session_cookies: &["slave_sid", "slave_user", "token"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // iQiyi — P00003 (访问票据) + QC006 (用户标识) are set on login.
    PlatformRule {
      key: "iqiyi",
      url: "https://www.iqiyi.com/",
      domains: &["iqiyi.com", "iqiyipic.com", "qiyi.com"],
      session_cookies: &["P00003", "QC006"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 搜狐视频 — 搜狐通行证 session pair.
    PlatformRule {
      key: "sohu_video",
      url: "https://tv.sohu.com/",
      domains: &["sohu.com", "sohucs.com"],
      session_cookies: &["sessionid", "userid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Meituan — CK (auth token) + acctId (account id) are set on login.
    PlatformRule {
      key: "meituan",
      url: "https://www.meituan.com/",
      domains: &["meituan.com", "meituan.net", "dianping.com", "51ping.com"],
      session_cookies: &["CK", "acctId"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // Ele.me — SID is the local session, cookie2 the Alibaba SSO pair (as taobao).
    PlatformRule {
      key: "eleme",
      url: "https://www.ele.me/",
      domains: &["ele.me", "eleme.com"],
      session_cookies: &["SID", "cookie2"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 唯品会 — 保守占位：登录 Cookie 名无公开文档，宁偏"未登录"。
    PlatformRule {
      key: "vipshop",
      url: "https://www.vip.com/",
      domains: &["vip.com", "vipstatic.com"],
      session_cookies: &["vip_sess", "vip_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 苏宁 — 保守占位。
    PlatformRule {
      key: "suning",
      url: "https://www.suning.com/",
      domains: &["suning.com", "suncdn.com"],
      session_cookies: &["suning_sess", "suning_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 钉钉网页版 — 保守占位。
    PlatformRule {
      key: "dingtalk",
      url: "https://www.dingtalk.com/",
      domains: &["dingtalk.com", "aliyun.com"],
      session_cookies: &["dingtalk_sess", "dingtalk_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 芒果TV — 保守占位。
    PlatformRule {
      key: "mango_tv",
      url: "https://www.mgtv.com/",
      domains: &["mgtv.com", "hunantv.com"],
      session_cookies: &["mgtv_sess", "mgtv_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 蘑菇街 — 保守占位。
    PlatformRule {
      key: "mogu_v",
      url: "https://www.mogujie.com/",
      domains: &["mogujie.com", "mogu.com", "mogucdn.com"],
      session_cookies: &["mogujie_sess", "mogujie_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 当当 — 保守占位。
    PlatformRule {
      key: "dangdang",
      url: "https://www.dangdang.com/",
      domains: &["dangdang.com"],
      session_cookies: &["dangdang_sess", "dangdang_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 网易考拉 — NetEase passport SSO (same family as netease mail).
    PlatformRule {
      key: "kaola",
      url: "https://www.kaola.com/",
      domains: &["kaola.com", "kaolacdn.com"],
      session_cookies: &["NTES_SESS", "P_INFO", "S_INFO"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 一号店 — JD SSO (YHD merged into JD, shares pt_key/pt_pin).
    PlatformRule {
      key: "yhd",
      url: "https://www.yhd.com/",
      domains: &["yhd.com", "yihaodian.com"],
      session_cookies: &["pt_key", "pt_pin"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 魅族账号 — 保守占位。
    PlatformRule {
      key: "flyme",
      url: "https://www.flyme.cn/",
      domains: &["flyme.cn", "meizu.com"],
      session_cookies: &["flyme_sess", "flyme_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // OPPO 账号 — 保守占位。
    PlatformRule {
      key: "oppo",
      url: "https://www.oppo.com/",
      domains: &["oppo.com", "nearme.com.cn", "coloros.com"],
      session_cookies: &["oppo_sess", "oppo_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // vivo 账号 — 保守占位。
    PlatformRule {
      key: "vivo",
      url: "https://www.vivo.com/",
      domains: &["vivo.com", "vivvo.com"],
      session_cookies: &["vivo_sess", "vivo_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 华为账号 — 保守占位。
    PlatformRule {
      key: "huawei",
      url: "https://consumer.huawei.com/",
      domains: &["huawei.com", "vmall.com", "hicloud.com"],
      session_cookies: &["huawei_sess", "huawei_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 小米账号 — passToken/userId are the account session pair.
    PlatformRule {
      key: "xiaomi",
      url: "https://www.mi.com/",
      domains: &["mi.com", "xiaomi.com", "miui.com"],
      session_cookies: &["passToken", "userId"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 360 通行证 — 保守占位。
    PlatformRule {
      key: "360",
      url: "https://www.360.cn/",
      domains: &["360.cn", "haosou.com", "qihoo.com", "360.com"],
      session_cookies: &["QI_USERNAME", "SM_ID"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 新浪通行证 — 保守占位（微博已有独立规则）。
    PlatformRule {
      key: "sina",
      url: "https://www.sina.com.cn/",
      domains: &["sina.com.cn", "sina.cn", "sina.com"],
      session_cookies: &["login_sid_t", "SINA_ID"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 东方财富 — 保守占位。
    PlatformRule {
      key: "eastmoney",
      url: "https://www.eastmoney.com/",
      domains: &["eastmoney.com", "eastmoney.cn"],
      session_cookies: &["eastmoney_sess", "eastmoney_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 华尔街见闻 — 保守占位。
    PlatformRule {
      key: "wallstreetcn",
      url: "https://www.wallstreetcn.com/",
      domains: &["wallstreetcn.com", "wallstreetcn.cn"],
      session_cookies: &["wallstreetcn_sess", "wallstreetcn_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 财联社 — 保守占位。
    PlatformRule {
      key: "cls",
      url: "https://www.cls.cn/",
      domains: &["cls.cn", "cailianpress.com"],
      session_cookies: &["cls_sess", "cls_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 第一财经 — 保守占位。
    PlatformRule {
      key: "yicai",
      url: "https://www.yicai.com/",
      domains: &["yicai.com", "yicai.tv"],
      session_cookies: &["yicai_sess", "yicai_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 凤凰网 — 保守占位。
    PlatformRule {
      key: "ifeng",
      url: "https://www.ifeng.com/",
      domains: &["ifeng.com", "ifengimg.com"],
      session_cookies: &["ifeng_sess", "ifeng_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 人民网 — 保守占位。
    PlatformRule {
      key: "people",
      url: "https://www.people.com.cn/",
      domains: &["people.com.cn", "people.cn"],
      session_cookies: &["people_sess", "people_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 央视网 — 保守占位。
    PlatformRule {
      key: "cctv",
      url: "https://www.cctv.com/",
      domains: &["cctv.com", "cntv.cn", "cctv.cn"],
      session_cookies: &["cctv_sess", "cctv_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 中国网 — 保守占位。
    PlatformRule {
      key: "china",
      url: "https://www.china.com.cn/",
      domains: &["china.com.cn", "china.com"],
      session_cookies: &["china_sess", "china_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 光明网 — 保守占位。
    PlatformRule {
      key: "gmw",
      url: "https://www.gmw.cn/",
      domains: &["gmw.cn", "gmw.com.cn"],
      session_cookies: &["gmw_sess", "gmw_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 环球网 — 保守占位。
    PlatformRule {
      key: "huanqiu",
      url: "https://www.huanqiu.com/",
      domains: &["huanqiu.com", "huanqiu.net"],
      session_cookies: &["huanqiu_sess", "huanqiu_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 东方网 — 保守占位。
    PlatformRule {
      key: "eastday",
      url: "https://www.eastday.com/",
      domains: &["eastday.com", "eastday.cn"],
      session_cookies: &["eastday_sess", "eastday_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
    // 红星新闻 — 保守占位。
    PlatformRule {
      key: "redstar",
      url: "https://www.redstar.cn/",
      domains: &["redstar.cn", "redstar.com"],
      session_cookies: &["redstar_sess", "redstar_uid"],
      network_probe: false,
      session_indicators: &[],
      login_indicators: &[],
      ok_status_range: (200, 299),
    },
  ]
}

/// True when `cookie_domain` belongs to a platform whose domains list contains
/// `platform_domain` as a suffix match (mirrors `cookie_sync::cookie_domain_matches`).
fn domain_matches(cookie_domain: &str, platform_domains: &[&str]) -> bool {
  let cd = cookie_domain.trim_start_matches('.');
  platform_domains.iter().any(|pd| {
    let pd = pd.trim_start_matches('.');
    cd == pd || cd.ends_with(&format!(".{pd}"))
  })
}

/// Tracks the last time we ran a health check per profile.
static LAST_CHECK: LazyLock<Mutex<HashMap<String, u64>>> =
  LazyLock::new(|| Mutex::new(HashMap::new()));

/// Probe a single platform with the given cookies attached.
///
/// Verdict order:
/// 1. No cookie matches the platform's domains → Missing.
/// 2. Session cookies (≥2, unexpired) are the primary signal. Missing or
///    expired here means the session is gone → Expired. This catches the
///    "cookie file still there but the session was revoked" case that the
///    score-only check misses, without relying on page HTML (SPA shells are
///    identical for guests and members).
/// 3. When the rule opts into a network probe, confirm with a live request.
async fn probe_platform(
  rule: &PlatformRule,
  cookies: &[crate::cookie_manager::UnifiedCookie],
) -> (HealthStatus, String) {
  let matching: Vec<&crate::cookie_manager::UnifiedCookie> = cookies
    .iter()
    .filter(|c| domain_matches(&c.domain, rule.domains))
    .collect();

  if matching.is_empty() {
    return (
      HealthStatus::Missing,
      "No cookies found for this domain".to_string(),
    );
  }

  // Primary verdict: session cookies present and unexpired.
  let now_secs = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs() as i64;
  let mut found = 0;
  for c in &matching {
    if rule.session_cookies.contains(&c.name.as_str()) {
      if c.expires > 0 && c.expires < now_secs {
        continue;
      }
      found += 1;
      if found >= 2 {
        break;
      }
    }
  }
  if found < 2 {
    if found == 0 {
      return (
        HealthStatus::Expired,
        "Session cookies missing — logged out".to_string(),
      );
    }
    return (
      HealthStatus::Expired,
      format!("Only {found} session cookie(s) present — logged out"),
    );
  }

  if !rule.network_probe {
    return (
      HealthStatus::Valid,
      "Session cookies present and unexpired".to_string(),
    );
  }

  // Secondary verdict: live HTTP probe (only for rules that opt in).
  let cookie_header: String = matching
    .iter()
    .map(|c| format!("{}={}", c.name, c.value))
    .collect::<Vec<_>>()
    .join("; ");

  let client = match reqwest::Client::builder()
    .timeout(PROBE_TIMEOUT)
    .redirect(reqwest::redirect::Policy::none())
    .build()
  {
    Ok(c) => c,
    Err(_) => {
      return (
        HealthStatus::Valid,
        "Session cookies valid; network probe unavailable".to_string(),
      );
    }
  };

  let resp = match client
    .get(rule.url)
    .header(
      "User-Agent",
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    )
    .header("Accept", "text/html,application/xhtml+xml")
    .header("Cookie", &cookie_header)
    .send()
    .await
  {
    Ok(r) => r,
    Err(e) => {
      return (HealthStatus::Valid, format!("Session cookies valid; probe failed: {e}"));
    }
  };

  let status = resp.status().as_u16();
  let location = resp
    .headers()
    .get("location")
    .map(|v| v.to_str().unwrap_or(""))
    .unwrap_or("");

  // Check for redirect to login.
  if (300..399).contains(&status) {
    let loc_lower = location.to_lowercase();
    if rule
      .login_indicators
      .iter()
      .any(|ind| loc_lower.contains(ind))
    {
      return (
        HealthStatus::Expired,
        format!("Redirected to login ({status})"),
      );
    }
    return (
      HealthStatus::Expired,
      format!("Redirect ({status}) to: {location}"),
    );
  }

  if !(rule.ok_status_range.0..=rule.ok_status_range.1).contains(&status) {
    return (
      HealthStatus::Expired,
      format!("Unexpected status: {status}"),
    );
  }

  let body = match resp.text().await {
    Ok(b) => b,
    Err(e) => {
      return (
        HealthStatus::Valid,
        format!("Session cookies valid; body read failed: {e}"),
      )
    }
  };

  // Check session indicators first (positive signal).
  if rule.session_indicators.iter().any(|ind| body.contains(ind)) {
    return (HealthStatus::Valid, "Session detected".to_string());
  }

  // Check login indicators (negative signal).
  let body_lower = body.to_lowercase();
  if rule
    .login_indicators
    .iter()
    .any(|ind| body_lower.contains(ind.to_lowercase().as_str()))
  {
    return (
      HealthStatus::Expired,
      "Page indicates logged out state".to_string(),
    );
  }

  // Default: session cookies are valid, so trust them over the page.
  (
    HealthStatus::Valid,
    format!("Session cookies valid (page loaded, status {status})"),
  )
}

/// Probe one platform with cookies in the CDP export format
/// (`Network.getAllCookies` / `Network.setCookies` objects). Used to verify a
/// cookie set that lives in the browser right now — e.g. cloud cookies fetched
/// at launch time — without requiring the cookies to be persisted on disk.
///
/// Returns `None` when the platform has no probe rule (caller falls back to
/// its existing logic). `Missing` means none of the cookies matched the
/// platform's domains.
pub async fn probe_cdp_cookies(
  platform: &str,
  cdp_cookies: &[serde_json::Value],
) -> Option<(HealthStatus, String)> {
  let rule = rule_for_platform(platform)?;

  let cookies: Vec<crate::cookie_manager::UnifiedCookie> =
    cdp_cookies.iter().map(unified_from_cdp).collect();

  let (status, message) = probe_platform(rule, &cookies).await;
  debug!(
    "Cookie health [{rule_key}]: {status:?} — {message}",
    rule_key = rule.key
  );
  Some((status, message))
}

/// Dump the stored cookies of one profile whose domain contains `suffix`,
/// as a `name=value; …` header string. Used by the diagnostic binary to
/// reproduce a live probe outside the app.
pub fn dump_platform_cookies(profile_id: &str, suffix: &str) -> String {
  let Ok(read) = CookieManager::read_cookies(profile_id) else {
    return String::new();
  };
  let mut out: Vec<String> = Vec::new();
  for d in &read.domains {
    for c in &d.cookies {
      if c.domain.contains(suffix) {
        out.push(format!("{}={}", c.name, c.value));
      }
    }
  }
  out.join("; ")
}

/// Convert a CDP `Network.Cookie` JSON object to our unified cookie type.
fn unified_from_cdp(c: &serde_json::Value) -> crate::cookie_manager::UnifiedCookie {
  crate::cookie_manager::UnifiedCookie {
    name: c
      .get("name")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_string(),
    value: c
      .get("value")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_string(),
    domain: c
      .get("domain")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_string(),
    path: c
      .get("path")
      .and_then(|v| v.as_str())
      .unwrap_or("/")
      .to_string(),
    expires: c
      .get("expires")
      .and_then(|v| v.as_f64())
      .map(|f| f as i64)
      .unwrap_or(0),
    is_secure: c.get("secure").and_then(|v| v.as_bool()).unwrap_or(false),
    is_http_only: c.get("httpOnly").and_then(|v| v.as_bool()).unwrap_or(false),
    same_site: match c.get("sameSite").and_then(|v| v.as_str()) {
      Some("Strict") => 0,
      Some("Lax") => 1,
      _ => 2,
    },
    creation_time: 0,
    last_accessed: 0,
  }
}

/// Run health checks for all configured platforms against a single profile's cookies.
pub async fn check_profile_health(profile_id: &str) -> Vec<CookieHealthResult> {
  let now_secs = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs();

  // Respect cooldown.
  {
    let mut last = LAST_CHECK.lock().await;
    if let Some(&last_check) = last.get(profile_id) {
      if now_secs - last_check < COOLDOWN_SECS {
        debug!("Cookie health: profile {profile_id} checked {COOLDOWN_SECS}s ago, skipping");
        return vec![];
      }
    }
    last.insert(profile_id.to_string(), now_secs);
    drop(last);
  }

  // Read cookies synchronously from the existing manager.
  let read_result = match CookieManager::read_cookies(profile_id) {
    Ok(r) => r,
    Err(e) => {
      debug!("Cookie health: failed to read cookies for {profile_id}: {e}");
      return vec![];
    }
  };

  // Flatten all cookies across domains.
  let cookies: Vec<crate::cookie_manager::UnifiedCookie> = read_result
    .domains
    .iter()
    .flat_map(|d| d.cookies.clone())
    .collect();

  if cookies.is_empty() {
    debug!("Cookie health: no cookies found for profile {profile_id}");
    return vec![];
  }

  debug!(
    "Cookie health: probing {} platforms for profile {} ({} cookies) domains={:?}",
    PLATFORM_RULES.len(),
    profile_id,
    cookies.len(),
    cookies.iter().map(|c| &c.domain).collect::<Vec<_>>()
  );

  let rules = &*PLATFORM_RULES;
  let mut results = Vec::with_capacity(rules.len());

  for rule in rules {
    let domain = rule
      .url
      .strip_prefix("https://")
      .or_else(|| rule.url.strip_prefix("http://"))
      .unwrap_or(rule.url)
      .split('/')
      .next()
      .unwrap_or("")
      .to_string();

    let (status, message) = probe_platform(rule, &cookies).await;
    debug!("Cookie health [{domain}]: {status:?} — {message}");
    results.push(CookieHealthResult {
      profile_id: profile_id.to_string(),
      profile_name: profile_id.to_string(),
      domain,
      url: rule.url.to_string(),
      status: status.clone(),
      message,
    });
  }

  let valid_count = results
    .iter()
    .filter(|r| r.status == HealthStatus::Valid)
    .count();
  info!(
    "Cookie health: profile {profile_id} — {valid_count}/{}/{} valid",
    results.len(),
    rules.len(),
  );

  results
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashSet;

  #[test]
  fn every_rule_has_session_cookies_and_domains() {
    let rules = platform_rules();
    let mut keys = HashSet::new();
    for rule in &rules {
      assert!(
        !rule.session_cookies.is_empty(),
        "{} has no session cookies",
        rule.key
      );
      assert!(!rule.domains.is_empty(), "{} has no domains", rule.key);
      assert!(
        rule.url.starts_with("https://"),
        "{} url must be https",
        rule.key
      );
      assert!(keys.insert(rule.key), "duplicate platform key {}", rule.key);
    }
  }

  #[test]
  fn rule_for_platform_resolves_aliases() {
    for (alias, expect) in [
      ("toutiao", "toutiao"),
      ("头条", "toutiao"),
      ("今日头条", "toutiao"),
      ("tt", "toutiao"),
      ("douyin", "douyin"),
      ("抖音", "douyin"),
      ("dy", "douyin"),
      ("tiktok", "tiktok"),
      ("tk", "tiktok"),
      ("netease", "netease"),
      ("网易", "netease"),
      ("mail_163", "netease"),
      ("bilibili", "bilibili"),
      ("哔哩哔哩", "bilibili"),
      ("weibo", "weibo"),
      ("微博", "weibo"),
      ("xiaohongshu", "xiaohongshu"),
      ("小红书", "xiaohongshu"),
      ("kuaishou", "kuaishou"),
      ("快手", "kuaishou"),
      ("facebook", "facebook"),
      ("fb", "facebook"),
      ("instagram", "instagram"),
      ("ig", "instagram"),
      ("github", "github"),
      ("gh", "github"),
      ("twitter", "x"),
      ("tw", "x"),
      ("x", "x"),
      ("youtube", "youtube"),
      ("yt", "youtube"),
      ("baijiahao", "baijiahao"),
      ("baidu", "baijiahao"),
      ("百度", "baijiahao"),
      ("百家号", "baijiahao"),
      ("tieba", "tieba"),
      ("百度贴吧", "tieba"),
      ("taobao", "taobao"),
      ("淘宝", "taobao"),
      ("youku", "youku"),
      ("优酷", "youku"),
      ("jingdong", "jingdong"),
      ("jd", "jingdong"),
      ("京东", "jingdong"),
      ("tengxun_video", "tengxun_video"),
      ("qq", "tengxun_video"),
      ("腾讯视频", "tengxun_video"),
      ("zhihu", "zhihu"),
      ("知乎", "zhihu"),
      ("xueqiu", "xueqiu"),
      ("雪球", "xueqiu"),
      ("pdd", "pdd"),
      ("拼多多", "pdd"),
      ("csdn", "csdn"),
      ("juejin", "juejin"),
      ("掘金", "juejin"),
      ("wechat_mp", "wechat_mp"),
      ("微信公众号", "wechat_mp"),
      ("xigua", "xigua"),
      ("西瓜视频", "xigua"),
      ("douyin_xingtu", "douyin_xingtu"),
      ("巨量星图", "douyin_xingtu"),
      ("dian", "dian"),
      ("大众点评", "dian"),
      ("wechat_video", "wechat_video"),
      ("微信视频号", "wechat_video"),
      ("视频号", "wechat_video"),
      ("iqiyi", "iqiyi"),
      ("爱奇艺", "iqiyi"),
      ("sohu_video", "sohu_video"),
      ("sohu", "sohu_video"),
      ("搜狐", "sohu_video"),
      ("搜狐视频", "sohu_video"),
      ("meituan", "meituan"),
      ("美团", "meituan"),
      ("eleme", "eleme"),
      ("饿了么", "eleme"),
      ("vipshop", "vipshop"),
      ("vip", "vipshop"),
      ("唯品会", "vipshop"),
      ("suning", "suning"),
      ("苏宁", "suning"),
      ("dingtalk", "dingtalk"),
      ("钉钉", "dingtalk"),
      ("mango_tv", "mango_tv"),
      ("芒果tv", "mango_tv"),
      ("芒果", "mango_tv"),
      ("芒果TV", "mango_tv"),
      ("mogu_v", "mogu_v"),
      ("蘑菇街", "mogu_v"),
      ("dangdang", "dangdang"),
      ("当当", "dangdang"),
      ("kaola", "kaola"),
      ("考拉", "kaola"),
      ("yhd", "yhd"),
      ("一号店", "yhd"),
      ("flyme", "flyme"),
      ("meizu", "flyme"),
      ("魅族", "flyme"),
      ("oppo", "oppo"),
      ("vivo", "vivo"),
      ("huawei", "huawei"),
      ("华为", "huawei"),
      ("xiaomi", "xiaomi"),
      ("小米", "xiaomi"),
      ("360", "360"),
      ("sina", "sina"),
      ("新浪", "sina"),
      ("eastmoney", "eastmoney"),
      ("东方财富", "eastmoney"),
      ("wallstreetcn", "wallstreetcn"),
      ("华尔街见闻", "wallstreetcn"),
      ("cls", "cls"),
      ("财联社", "cls"),
      ("yicai", "yicai"),
      ("第一财经", "yicai"),
      ("ifeng", "ifeng"),
      ("凤凰网", "ifeng"),
      ("people", "people"),
      ("人民网", "people"),
      ("cctv", "cctv"),
      ("央视网", "cctv"),
      ("china", "china"),
      ("中国网", "china"),
      ("gmw", "gmw"),
      ("光明网", "gmw"),
      ("huanqiu", "huanqiu"),
      ("环球网", "huanqiu"),
      ("eastday", "eastday"),
      ("东方网", "eastday"),
      ("redstar", "redstar"),
      ("红星新闻", "redstar"),
      ("douyin_hao", "douyin"),
      ("抖音号", "douyin"),
      ("little_red_book", "xiaohongshu"),
      ("wb_zt", "weibo"),
    ] {
      assert_eq!(
        rule_for_platform(alias).map(|r| r.key),
        Some(expect),
        "alias {alias}"
      );
    }
    assert!(rule_for_platform("no_such_platform").is_none());
  }

  #[test]
  fn domain_matches_suffix() {
    assert!(domain_matches(
      ".douyin.com",
      &["douyin.com", "toutiao.com"]
    ));
    assert!(domain_matches("www.douyin.com", &["douyin.com"]));
    assert!(domain_matches("creator.douyin.com", &["douyin.com"]));
    assert!(!domain_matches("douyin.com.cn", &["douyin.com"]));
    assert!(!domain_matches("evil-douyin.com", &["douyin.com"]));
  }
}
