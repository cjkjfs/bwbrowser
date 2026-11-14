# 项目长期记忆 — bwbrowser-main

## ⚠️ 用户硬性偏好（2026-11-09 明确要求）
- **绝对不要跑 cargo 编译类命令**（cargo check / clippy / fmt / build 全部不要跑）——用户有自己的自动编译，跑它会抢构建锁（os error 5）且太慢
- 改完 Rust 代码后**只改代码、不验证编译**，直接告诉用户"需要重编译"即可，由用户自己编译
- 前端只改代码；`tsc`/`biome` 这类秒级静态检查如需要可快速跑，但尽量精简、不要反复跑

## VPS（PHP/宝塔服务器）连接信息

> 来源：`D:\bian\vps\连接信息.txt`（已核实）。用于本地部署脚本复用。

### SSH 连接（推荐私钥）
- 服务器地址：**47.93.197.114**
- SSH 端口：**220**（注意不是 22，22 已被拒绝）
- 用户名：**root**
- 认证：SSH 私钥（推荐），私钥文件 `D:\bian\vps\47.93.197.114_id_ed25519`
- 公钥注释：`root@baowenku.com`
- 密码（备选）：`cQJGJceNfD2CHZnZ`
- 最有效连接：SSH 私钥认证 + 端口 220

### Web 与面板
- 宝塔面板：宝塔（BaoTa）
- 宝塔账号：`bjz` / 密码 `Aa850120@`
- Web 根目录：`/www/wwwroot/www.yacm.xin`
- 考勤脚本目录：`/www/wwwroot/www.yacm.xin/tk/`

### 部署实操要点
1. 连接端口固定 220
2. 部署前先备份线上文件为 `.bak_日期` 再覆盖
3. 上传后跑 `php -l` 校验，`chown www:www` 且 `chmod 644`

### 面板/网关及坑点（历史经验）
- Magpie 面板：`http://47.93.197.114:3426/?k=UAPti8ayai3Ek9e7oHb74kZX`（页面密钥存于 `magpie_admin.php`）；网关 3425
- 局域网反向代理/页面跳转需**固定写死公网 IP `47.93.197.114`**，因为 `$_SERVER['SERVER_ADDR']` 返回内网 IP（172.24.44.175）
- ⚠️【2026-11-07】VPS 上的 3x-ui 及 xray 已**全部卸载清理干净**。原配置（xray SOCKS5 `127.0.0.1:10808`、VLESS+REALITY 入站 58888、xray-socks5.service、xray-socks5.service 手动服务）均**已不存在**，请勿再引用。若以后需让 baowenku.php 的 YouTube/谷歌请求走代理，需重新新增独立的 xray vless **出站**客户端。

### 待办/后续
- ✅【2026-11-07】节点隧道已建成并可用（见下）。**✅【2026-11-07 下午】接线已完成**：5 个文件统一插 `vless_curl_apply_proxy()`（baowenku:15、youtube_api_lib:6、media_api:5、batch_update_single:2、ai_agent_cron_api:1）。hot_news/hot_news_cron 端口已改 10810。**待用户操作**：api_stats「⚙️上游API统一配置」把 `pf_youtube_url` 设 `https://www.googleapis.com`（目前仍是 192.3.233.161）。

### VLESS 出站节点体系（2026-11-07 建成）
- **本地代理**：`socks5h://127.0.0.1:10810`，由 `xray-vless.service`（xray v26.10.31 官方单二进制 `/usr/local/bin/xray`，User=www）提供，开机自启。
- **配置文件**：`/www/wwwroot/www.yacm.xin/tk/vless-proxy/config.json`（www 属主，由 PHP 生成）。
- **默认节点**：us002（70.39.192.95:443，VLESS+REALITY，sni=www.toyota.com），存于 `site_settings` 表：
  `vless_nodes_json`（节点JSON数组）、`vless_active_id`、`vless_socks_port`。
- **启停代理**：`vless-agent.service`（root，`/opt/vless-agent/agent.sh`）轮询 `vless-proxy/run/queue.json`，
  仅执行 start/stop/restart 白名单动作并回写 `result.json`。原因：宝塔 PHP-FPM 禁用了 exec/shell_exec/system/proc_open。
- **管理界面**：`api_stats.php` 新增「🛰️ 节点设置」tab（仅超管），功能：添加/编辑/删除 vless 节点、设默认、启动/停止/重启、测试谷歌连通、**多节点自动切换 checkbox**。
  核心库 `tk/vless_proxy_lib.php`（解析/生成配置/排队/探测/测连通/给curl应用节点代理/多节点故障切换）。
- **已弃用**：sudoers 白名单方案（因 disable_functions 放弃），`/etc/sudoers.d/xray-vless` 已删除。
- **多节点自动切换**（2026-11-07 下午）：`vless_curl_apply_proxy`（仅 googleapis/google/youtube 域名走代理，开关 `vless_proxy_curl_enabled`）、`vless_failover_check`（不通依序切其它节点，失败回滚）、`vless-restart_service`。看门狗 `vless_failover_watchdog.php` + agent.sh 每 5 分钟跑一次。开关 `vless_failover` 默认 ON。
- **代理中心节点拉取 + 轮询勾选**（2026-11-09）：指纹浏览器代理中心=`simprint_proxies` 表（124 节点，含 company_id→公司名；vless 完整链接在 protocol_config，裸串或 JSON vless_url 两种格式）。api_stats「节点设置」tab 新增「📥 代理中心节点」卡片：`list_center_proxies` 拉全部节点（显示公司名）、`import_center_nodes` 勾选导入节点库（id 前缀 c，按 host:port 去重；socks5/trojan/http 暂不支持导入）。**节点列表新增「轮询」勾选列**：勾选直接写节点记录 `rotate` 字段（不单独存池），`toggle_rotate` 切换；`vless_rotate_nodes`/`vless_rotate_next`（lib）实现看门狗每 5 分钟把出口轮换到下一个勾选节点（≥2 个生效），failover 候选优先池内。曾经做过又删掉的：「分配节点到指纹账号」功能（用户明确说无用）。备份 api_stats.php.bak_20261109_nodeassign。
- **备份**：`api_stats.php.bak_20261107`。本地工作稿：`D:\bian\vps\api_stats.php`、`D:\bian\vps\vless_proxy_lib.php`、`D:\bian\vps\vless_failover_watchdog.php`、`D:\bian\vps\_vless_proxy\`。

### 相关文档
- Magpie Trae 登录修复：`D:\bian\vps\_work\magpie_trae_login_fix.md`

---
## 视频嗅探下载扩展（内置 MV3，2026-11-09~10）

- **加载目录**：Chrome `--load-extension` 加载的是 `target/debug/extensions/video-sniffer/`（构建时从 `src-tauri/extensions/` 拷贝的独立副本，**非链接**）。改源码必须手动 `cp` 同步 target，且 content script 改动需**重开爆文库浏览器**才生效。
- **⚠️ verbatim 路径缓存坑（2026-11-10 定位，最关键）**：Rust `Path::canonicalize()` 在 Windows 返回 `\\?\C:\...`；Chromium 对 verbatim 路径加载的 unpacked 扩展会**错误缓存文件**（reload/后续读取永远拿旧内容）。修复：`extension_manager.rs` 新增 `strip_verbatim_prefix`（需重编译），现有 metadata.json linked_path 已手动去 `\\?\`。**以后凡新增/编辑内置扩展且涉及 canonicalize→--load-extension，必须保证路径无 `\\?\` 前缀。**
- **扩展 id**：嗅探=`jegkpjdekbkkdikkplpppnniankhlgpj`（"视频嗅探下载"）；达人精灵=`llpkfdob...`（DISABLED）；文字转语音=`iekblgkc...`。`--load-extension` 里 verbatim path 项即嗅探扩展。
- **下载链路**：扩展按钮 `POST 127.0.0.1:[10118..10120]/v1/video-download/enqueue`（常驻 mini server，不依赖 api_enabled，免 token 窄端）→ `get_downloader().add_task()`（与前端导航栏视频下载同队列）→ 按钮轮询 `GET .../v1/video-download/task/{id}` 显示百分比。SW 休眠丢事件已用页面内 hook（hook.js 注入 MAIN world + postMessage）修复；该 Chromium 151 对 manifest `world:"MAIN"` 无效，须用 `<script src=getURL>` 注入 + web_accessible_resources。
- **平台页 URL 下载（2026-11-10）**：主程序下载器=yt-dlp，原生支持 YT/TikTok/抖音/B站 页面 URL。扩展对已知平台页（`PLATFORM_RE` 与 Rust 白名单一致）：
  主播放器（`scoreVideo` 打分）无 http 直链（blob:/MSE）时，按钮改走 `location.href`（viaPage，交 yt-dlp 解析），文案「下载视频」；`engage()` 另有 `extraPageFallback` 兜底，保证平台页必出下载入口。这解决了 YT/TikTok「识别不到视频」。改 content.js 需重开浏览器生效。
---
## 附加运营人（2026-11-14 上线，客户端不动）

- **DB**：`tiktok_accounts.co_owner_ids VARCHAR(255) NULL`（逗号分隔用户id）。主归属 owner_id 完全不动。
- **语义**：账号可设多个附加运营人，被设置者即可见/可操作该账号（主归属+附加运营）。member 可见=`owner_id=我 OR FIND_IN_SET(我,co_owner_ids)`；leader 组=组员主归属 OR 组员是附加运营；manager 筛某人=他的主归属+他是附加运营的；private 对附加运营放行；家族继承母账号集合同步扩展；benchmark 无此字段（tiktok-only）。summary tiktok 聚合=可见口径，owners 人员分布仍算主归属头上。
- **网页端**：account_detail_admin.php detailModal「👥 附加运营人」checkbox 多选（同公司成员、排除主归属），save_detail 校验同公司 active 用户后落库。
- **回退**：`tk/*.bak_precowner_20261114_145156`。**踩坑**：MySQL 5.7 REGEXP 是 POSIX 不支持 `(?:)`；纯字面量 WHERE 时 execute 不可再合并 ownerParams（参数多→聚合静默归零）；临时改密码必须**先 SELECT 备份再 UPDATE**（users 有 plain_password 明文列可兜底重建）。
