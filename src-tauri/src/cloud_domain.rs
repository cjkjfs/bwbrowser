//! 云端域名的唯一来源。
//!
//! 整个后端所有云端地址都由本文件派生：只改 `cloud_host!()` 里的域名，
//! API、同步、远程中继、PHP 接口、下载、更新、内核清单等全部 URL 跟着变，
//! 不需要再动其他任何文件。
//!
//! 前端对应文件是 `src/lib/cloud-config.ts` 的 `CLOUD_DOMAIN`，两处需保持一致。

/// 云端主域名（唯一需要修改的地方）
macro_rules! cloud_host {
  () => {
    "yacm.xin"
  };
}

/// 裸域，例如 `yacm.xin`
pub const CLOUD_DOMAIN: &str = cloud_host!();

/// 站点根（HTTPS），例如 `https://yacm.xin`
pub const CLOUD_ROOT: &str = concat!("https://", cloud_host!());

/// 站点根（HTTP），下载与部分 PHP 页面仍走 HTTP 以规避证书问题
pub const CLOUD_ROOT_HTTP: &str = concat!("http://", cloud_host!());

/// 后端 REST API + 远程 MCP，例如 `https://api.yacm.xin`
pub const CLOUD_API_URL: &str = concat!("https://api.", cloud_host!());

/// 云端同步服务，例如 `https://sync.yacm.xin`
pub const CLOUD_SYNC_URL: &str = concat!("https://sync.", cloud_host!());

/// 远程桌面中继 WebSocket，例如 `wss://yacm.xin/relay`
pub const RELAY_URL: &str = concat!("wss://", cloud_host!(), "/relay");

/// 远程管理 / VPS 登录接口（remote_api.php）
pub const REMOTE_API_URL: &str = concat!("http://", cloud_host!(), "/tk/remote_api.php");

/// 中继控制页，局域网不可达时的回退入口（remote_app.php）
pub const RELAY_CONTROL_URL: &str = concat!("http://", cloud_host!(), "/tk/remote_app.php");

/// Bwbrowser 云端 API（登录/同步）
pub const BWBROWSER_API_URL: &str = concat!("http://", cloud_host!(), "/tk/bwbrowser_sync.php");

/// 平台配置 API（平台 creator_url / login_check）
pub const PLATFORM_CONFIG_API_URL: &str =
  concat!("http://", cloud_host!(), "/tk/api_get_platform_config.php");

/// Wayfern Token 接口
pub const WAYFERN_TOKEN_API_URL: &str =
  concat!("http://", cloud_host!(), "/tk/api_auth_wayfern_start.php");

/// Simprint 云端账号 API
pub const SIMPRINT_ACCOUNTS_URL: &str =
  concat!("http://", cloud_host!(), "/tk/simprint_accounts.php");

/// 用户管理 API
pub const BWBROWSER_USERS_API_URL: &str = concat!("http://", cloud_host!(), "/tk/users.php");

/// 爆文库账号详情页
pub const BAOWENKU_URL: &str = concat!("https://", cloud_host!(), "/tk/baowenku.php");

/// VPS 自动登录页
pub const LOGIN_URL: &str = concat!("http://", cloud_host!(), "/tk/login.php");

/// 浏览器二进制下载目录
pub const BROWSER_DOWNLOAD_URL: &str = concat!("https://", cloud_host!(), "/download");

/// 应用更新检查接口
pub const UPDATE_CHECK_URL: &str = concat!("https://", cloud_host!(), "/tk/bwbrowser_updates.php");

/// Wayfern 内核版本清单
pub const WAYFERN_JSON_URL: &str =
  concat!("http://", cloud_host!(), "/tk/download/kernel/wayfern.json");
