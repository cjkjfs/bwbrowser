# 项目长期记忆 — bwbrowser-main

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
- **备份**：`api_stats.php.bak_20261107`。本地工作稿：`D:\bian\vps\api_stats.php`、`D:\bian\vps\vless_proxy_lib.php`、`D:\bian\vps\_vless_proxy\`。

### 相关文档
- Magpie Trae 登录修复：`D:\bian\vps\_work\magpie_trae_login_fix.md`