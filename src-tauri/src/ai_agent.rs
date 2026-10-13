//! AI 助手（rig agent）：LLM 多轮对话 + 工具调用。
//!
//! 模型端点复用 AI 洗稿的「文案模型（llm）」配置（provider / base_url /
//! api_key / model / 代理线路）；HTTP 请求经 rig-reqwest 注入项目自身的
//! reqwest client，出口与洗稿主链路一致（VLESS/Trojan 经本地 Xray 侧车）。
//! 工具面一期只读：视频库、洗稿成果列表、单条成果详情、代理中心线路。
//! 对话历史由前端持有并全量传入，后端不落盘。

use std::time::Duration;

use lazy_static::lazy_static;
use rig_agent::{Agent, AgentBuilder};
use rig_core::completion::message::{ToolName, UserContent};
use rig_core::completion::message::{ToolResult as RigToolResult, ToolResultContent};
use rig_core::completion::Message;
use rig_core::operation::Completion;
use rig_core::providers::anthropic::AnthropicConfig;
use rig_core::providers::gemini::GeminiConfig;
use rig_core::providers::openai::OpenAIConfig;
use rig_core::tool::{DynamicTool, ToolErrorKind, ToolExecutionError, ToolOutput};
use rig_core::DynModel;
use rig_reqwest::ReqwestClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ai_rewrite::{AiEndpointConfig, AI_REWRITE};

/// 工具循环上限：单条消息最多经历的「模型请求 → 工具执行」轮数。
const MAX_TURNS: usize = 6;
/// LLM 请求超时：多轮工具调用下模型可能思考较久，比洗稿链路的 60s 放宽。
const HTTP_TIMEOUT: Duration = Duration::from_secs(300);

lazy_static! {
  /// 按 llm 端点配置指纹缓存已构建的 Agent；配置（含代理线路）变化时重建。
  static ref CACHED_AGENT: tokio::sync::Mutex<Option<(String, Agent)>> =
    tokio::sync::Mutex::new(None);
}

// ============================================================
// Tauri 入参 / 出参
// ============================================================

#[derive(Debug, Deserialize)]
pub struct AgentChatTurn {
  pub role: String,
  pub content: String,
}

/// 一次工具调用的可展示步骤（前端渲染在回复上方的过程区）。
#[derive(Debug, Serialize)]
pub struct AgentChatStep {
  /// "tool_call"（模型请求调用）或 "tool_result"（工具返回）
  pub kind: String,
  pub name: String,
  pub detail: String,
  pub is_error: bool,
}

#[derive(Debug, Serialize)]
pub struct AgentChatReply {
  pub reply: String,
  pub steps: Vec<AgentChatStep>,
}

// ============================================================
// 本地文件夹授权：用户在前端用原生选择器挑的目录清单，随每轮对话传入。
// 工具只允许读这份清单内的路径，LLM 无法自行指定系统任意位置。
// ============================================================

/// 校验 path 是否位于某个已授权文件夹内（canonicalize 后做前缀匹配）。
fn path_is_allowed(path: &str, allowed: &[String]) -> Result<std::path::PathBuf, String> {
  if allowed.is_empty() {
    return Err("未授权任何文件夹：请先在对话上方点击「选择文件夹」".to_string());
  }
  let target =
    std::fs::canonicalize(path).map_err(|e| format!("路径不存在或不可访问：{path}（{e}）"))?;
  for folder in allowed {
    if let Ok(root) = std::fs::canonicalize(folder) {
      if target.starts_with(&root) {
        return Ok(target);
      }
    }
  }
  Err(format!(
    "路径不在已授权的文件夹内：{}。只能读取用户明确选择的文件夹",
    path
  ))
}

const FOLDER_LIST_LIMIT: usize = 300;

/// 列出授权文件夹（或其子目录）的直接内容：名字、类型、大小。
async fn list_folder_impl(folder: &str, allowed: &[String]) -> Result<Value, String> {
  let target = path_is_allowed(folder, allowed)?;
  let mut read_dir = tokio::fs::read_dir(&target)
    .await
    .map_err(|e| format!("无法读取目录：{e}"))?;
  let mut entries: Vec<Value> = Vec::new();
  let mut truncated = false;
  while let Some(item) = read_dir
    .next_entry()
    .await
    .map_err(|e| format!("遍历目录失败：{e}"))?
  {
    if entries.len() >= FOLDER_LIST_LIMIT {
      truncated = true;
      break;
    }
    let meta = item.metadata().await.map_err(|e| e.to_string())?;
    entries.push(json!({
        "name": item.file_name().to_string_lossy(),
        "type": if meta.is_dir() { "dir" } else { "file" },
        "size": if meta.is_dir() { Value::Null } else { json!(meta.len()) },
    }));
  }
  entries.sort_by(|a, b| {
    let dir_a = a.get("type").and_then(Value::as_str) == Some("dir");
    let dir_b = b.get("type").and_then(Value::as_str) == Some("dir");
    dir_b.cmp(&dir_a).then_with(|| {
      a.get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase()
        .cmp(
          &b.get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase(),
        )
    })
  });
  let mut value = json!({
      "folder": target.to_string_lossy(),
      "count": entries.len(),
      "truncated": truncated,
      "entries": entries,
      "_scope_note": "目录列表按文件夹在前排序；需要某个文本文件的内容用 read_text_file。二进制/超大文件（>512KB）无法读取。",
  });
  truncate_json(&mut value, 8000);
  Ok(value)
}

/// 读取授权文件夹内的单个文本文件（≤512KB；内容里出现 NUL 字节判为二进制拒绝）。
async fn read_text_file_impl(path: &str, allowed: &[String]) -> Result<Value, String> {
  let target = path_is_allowed(path, allowed)?;
  let meta = tokio::fs::metadata(&target)
    .await
    .map_err(|e| format!("无法访问文件：{e}"))?;
  if meta.is_dir() {
    return Err(format!(
      "这是一个目录，用 list_folder 列出：{}",
      target.to_string_lossy()
    ));
  }
  if meta.len() > 512 * 1024 {
    return Err(format!("文件超过 512KB（{} 字节），不支持读取", meta.len()));
  }
  let bytes = tokio::fs::read(&target)
    .await
    .map_err(|e| format!("读取失败：{e}"))?;
  if bytes.contains(&0) {
    return Err("文件是二进制格式，无法作为文本读取".to_string());
  }
  let text = String::from_utf8_lossy(&bytes);
  let truncated = text.len() > 64 * 1024;
  let content = if truncated {
    let cut: String = text.chars().take(16 * 1024).collect();
    format!("{cut}\n…[内容过长，已截断]")
  } else {
    text.into_owned()
  };
  Ok(json!({
      "path": target.to_string_lossy(),
      "size": meta.len(),
      "truncated": truncated,
      "content": content,
  }))
}

// ============================================================
// VPS：用户在对话中提供连接信息，助手经 SSH 执行单条命令。
// 连接信息只在当轮使用，不落盘。
// ============================================================

const VPS_EXEC_TIMEOUT: Duration = Duration::from_secs(30);

struct NoVerifyHostKey;

impl russh::client::Handler for NoVerifyHostKey {
  type Error = russh::Error;

  // 一期不做 host key 校验（TOFU/指纹校验二期限定）。错误回调里提示用户。
  async fn check_server_key(
    &mut self,
    _server_public_key: &russh::keys::PublicKeyOrCertificate,
  ) -> Result<bool, Self::Error> {
    Ok(true)
  }
}

/// SSH 连上执行一条命令，收集 stdout/stderr 与退出码（30s 超时）。
async fn vps_exec_impl(
  host: &str,
  port: u16,
  username: &str,
  password: &str,
  command: &str,
) -> Result<Value, String> {
  let config = std::sync::Arc::new(russh::client::Config::default());
  let connect_fut = russh::client::connect(config, (host, port), NoVerifyHostKey);
  let mut handle = tokio::time::timeout(VPS_EXEC_TIMEOUT, connect_fut)
    .await
    .map_err(|_| format!("连接超时（{VPS_EXEC_TIMEOUT:?}）：{host}:{port}"))?
    .map_err(|e| format!("SSH 连接失败：{e}"))?;
  let authed = tokio::time::timeout(
    VPS_EXEC_TIMEOUT,
    handle.authenticate_password(username, password),
  )
  .await
  .map_err(|_| "认证超时".to_string())?
  .map_err(|e| format!("SSH 认证失败：{e}"))?;
  if !matches!(authed, russh::client::AuthResult::Success) {
    return Err("SSH 认证被拒绝：用户名或密码错误".to_string());
  }
  let mut channel = handle
    .channel_open_session()
    .await
    .map_err(|e| format!("打开会话通道失败：{e}"))?;
  channel
    .exec(true, command.to_string())
    .await
    .map_err(|e| format!("执行命令失败：{e}"))?;
  let mut stdout = String::new();
  let mut stderr = String::new();
  let mut exit_code: Option<u32> = None;
  loop {
    let Some(msg) = tokio::time::timeout(VPS_EXEC_TIMEOUT, channel.wait())
      .await
      .unwrap_or(None)
    else {
      break;
    };
    match msg {
      russh::ChannelMsg::Data { ref data } => {
        stdout.push_str(&String::from_utf8_lossy(data));
      }
      russh::ChannelMsg::ExtendedData { ref data, .. } => {
        stderr.push_str(&String::from_utf8_lossy(data));
      }
      russh::ChannelMsg::ExitStatus { exit_status } => {
        exit_code = Some(exit_status);
      }
      russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
      _ => {}
    }
  }
  let _ = handle
    .disconnect(russh::Disconnect::ByApplication, "done", "en")
    .await;
  // 输出截断：命令输出可能巨大，防上下文爆炸。
  let cut = |s: &str, max: usize| -> String {
    if s.chars().count() <= max {
      s.to_string()
    } else {
      let head: String = s.chars().take(max).collect();
      format!("{head}\n…[输出过长，已截断]")
    }
  };
  Ok(json!({
      "host": host,
      "port": port,
      "command": command,
      "exit_code": exit_code,
      "stdout": cut(&stdout, 4000),
      "stderr": cut(&stderr, 2000),
  }))
}

// ============================================================
// 错误：统一走 backend error code JSON
// ============================================================

fn code_err(code: &str, message: Option<&str>) -> String {
  match message {
    Some(m) => json!({ "code": code, "params": { "message": m } }).to_string(),
    None => json!({ "code": code }).to_string(),
  }
}

// ============================================================
// Agent 构建
// ============================================================

const PREAMBLE: &str = "你是 BW Browser 内置的 AI 助手，可以调用工具查询与执行：视频库（query_videos）、洗稿成果列表（query_results）、单条成果详情（get_rewrite_result）、代理中心线路（list_proxies）、发起洗稿任务（start_rewrite_tasks）、查洗稿任务进度（query_running_tasks）、平台数据更新（cron_update）、实时拉取账号主页视频（fetch_account_videos）、实时拉取视频最新数据（fetch_video_data）、列本地文件夹（list_folder）、读本地文本文件（read_text_file）、在用户提供的 VPS 上执行命令（vps_exec）。\n\
\n\
数据边界（必须严格遵守，回答口径保持一致）：\n\
- 所有工具返回的数据都在当前登录公司可见范围内：自有账号 + 对标账号，由云端服务器按登录账号过滤，不存在其他公司的数据。\n\
- account_id 是账号入库编号（每个自有/对标账号各有一个编号），不是“公司ID”。绝对不要把 account_id 解读成公司归属，也不要输出“拉取了全库 / 其他公司数据”这类结论。\n\
- account_type：own=自有账号，benchmark=对标账号。视频库里绝大多数是对标账号属于正常现象，那是贵公司配置的对标名单。\n\
\n\
回答规则：\n\
1. 需要数据时先调用工具，再根据结果回答，不要凭空编造；工具结果里没有的信息就说没有。\n\
2. 提炼关键信息后作答，不要整段粘贴原始 JSON；列表很长时挑重点，说明总数与分页。\n\
3. 用户要“完整文案 / 给我全文 / 原文”时，把 get_rewrite_result 返回的洗稿文案（钩子、正文、结尾引导）完整输出，不要概括截断。\n\
4. 直接给结论，不要每轮都罗列一堆选项反问；最多保留一个必要的追问。\n\
5. 用用户的语言回答（默认简体中文），保持简洁。\n\
6. 发起洗稿任务：用户给出视频链接、或明确点名某个视频要洗稿/分析时，直接调用 start_rewrite_tasks，不要再向用户确认（点名即授权）；只有意图模糊（没说清处理哪个视频）时才问一次。一次最多 3 条，用户要求更多时说明限制并分批。任务发起后会自动出现在应用的任务列表（洗稿页任务 Tab），用户可直接看到实时进度，不要反问“要不要查进度”；发起后用 query_running_tasks 查一次当前阶段做简报，完成后用 query_results 查成果。\n\
7. 本地文件夹：list_folder / read_text_file 只能访问用户通过「选择文件夹」按钮授权的目录，工具会拒绝其他路径，被拒绝就告知用户请先选择对应文件夹，不要换路径绕过。\n\
8. VPS 的硬规则：连接信息（主机/端口/用户名/密码）必须由用户在对话里提供，不要主动索要，也不要猜测。执行前向用户复述将运行的命令；破坏性命令（rm、mkfs、dd、reboot、shutdown、重启服务等）必须有用户的明确同意才能执行。连接信息只在当轮使用。\n\
9. 用户问数据归属、隔离或“是不是别家公司的数据”时，用上面的数据边界回答。\n\
10. 触发数据更新的硬规则：cron_update 的 action=run 会按次计费并让服务器后台更新当前公司的平台账号数据（tiktok / youtube / douyin）。调用 run 前必须先告知用户平台与单价（先用 action=prices 查价目、余额），得到用户明确同意后才能 run；run 成功后告知用户约 5 分钟内完成，稍后可用 action=status 查进度、query_videos 看新数据。status 和 prices 是只读免费查询，可随意使用。\n\
11. 实时拉取的硬规则：fetch_account_videos / fetch_video_data 每次调用都会扣费（走公司配置的上游数据 API，播放量/发布时间为拉取时刻的实时值）。调用前必须告知用户将拉取什么（哪个账号/哪个视频）并征得同意；拿到数据后重点解读发布时间、播放量、点赞率等指标，可用于洗稿前的重点分析。查询视频库里已有的数据用 query_videos（免费），只有用户要「最新实时数据」时才用拉取工具。";

/// 工具闭包需要的应用句柄（发起洗稿任务要用），对话命令首次调用时注入。
static APP_HANDLE: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

/// 本轮对话授权的本地文件夹清单：前端每次随请求传入，工具闭包读这里。
/// Agent 是缓存的，授权清单是每轮变的，所以放全局而不是闭包捕获。
static ALLOWED_FOLDERS: std::sync::LazyLock<tokio::sync::Mutex<Vec<String>>> =
  std::sync::LazyLock::new(|| tokio::sync::Mutex::new(Vec::new()));

/// LLM 端点指纹：任一字段（含代理）变化都触发 Agent 重建。
fn endpoint_fingerprint(ep: &AiEndpointConfig) -> String {
  serde_json::to_string(ep).unwrap_or_default()
}

/// 构建走指定代理线路的 HTTP transport，注入 rig。
fn build_http_transport(proxy_url: Option<&str>) -> Result<ReqwestClient, String> {
  let mut builder = reqwest::Client::builder()
    .timeout(HTTP_TIMEOUT)
    .connect_timeout(Duration::from_secs(15));
  if let Some(url) = proxy_url {
    builder = builder.proxy(reqwest::Proxy::all(url).map_err(|e| format!("代理地址无效：{e}"))?);
  }
  Ok(ReqwestClient::from(
    builder
      .build()
      .map_err(|e| format!("构建 HTTP 客户端失败：{e}"))?,
  ))
}

/// 代理 URL 脱敏：socks5://user:pass@host:port -> socks5://user:***@host:port，
/// 用于日志输出，密码不落盘。
fn mask_proxy_url(url: &str) -> String {
  let Some(idx) = url.find("://") else {
    return url.to_string();
  };
  let (scheme, rest) = url.split_at(idx + 3);
  let Some(at) = rest.rfind('@') else {
    return url.to_string();
  };
  let user = rest[..at].split(':').next().unwrap_or("");
  format!("{scheme}{user}:***{}", &rest[at..])
}

/// 线路选择（配置即真相，无静默回退）：配置了代理就强制走代理——代理不可达时
/// 请求直接失败，绝不回退直连；显式的直连配置则直连（国内可直连的模型合法）。
/// 两种线路都写日志，运行线路永远与配置一致。
fn log_line(proxy_url: Option<&str>) {
  match proxy_url {
    Some(url) => log::info!("AI 助手 LLM 请求线路：走代理 {}", mask_proxy_url(url)),
    None => log::info!("AI 助手 LLM 请求线路：直连（未配置代理）"),
  }
}

/// 按端点配置构建 rig 模型（openai_compatible / anthropic / gemini）。
async fn build_model(ep: &AiEndpointConfig) -> Result<DynModel<Completion>, String> {
  if ep.provider.trim().is_empty() || ep.api_key.trim().is_empty() || ep.model.trim().is_empty() {
    return Err(code_err("AI_AGENT_NOT_CONFIGURED", None));
  }
  let proxy_url = crate::ai_rewrite::resolve_proxy_url(&ep.proxy)
    .await
    .map_err(|e| code_err("AI_AGENT_PROXY_FAILED", Some(&e)))?;
  log_line(proxy_url.as_deref());
  let http = build_http_transport(proxy_url.as_deref())
    .map_err(|e| code_err("AI_AGENT_PROXY_FAILED", Some(&e)))?;
  let model_name = ep.model.trim();
  let base_url = ep.base_url.trim();
  match ep.provider.as_str() {
    "openai_compatible" => {
      let mut cfg = OpenAIConfig::new(ep.api_key.clone());
      if !base_url.is_empty() {
        cfg = cfg.with_base_url(base_url);
      }
      // Chat Completions 端点：DeepSeek 等兼容网关都实现这一族。
      Ok(cfg.connect(http).chat(model_name).into())
    }
    "anthropic" => {
      let mut cfg = AnthropicConfig::new(ep.api_key.clone());
      if !base_url.is_empty() {
        cfg = cfg.with_base_url(base_url);
      }
      Ok(cfg.connect(http).completion(model_name).into())
    }
    "gemini" => {
      let mut cfg = GeminiConfig::new(ep.api_key.clone());
      if !base_url.is_empty() {
        cfg = cfg.with_base_url(base_url);
      }
      Ok(cfg.connect(http).completion(model_name).into())
    }
    other => Err(code_err("AI_AGENT_PROVIDER_UNSUPPORTED", Some(other))),
  }
}

async fn build_agent(ep: &AiEndpointConfig) -> Result<Agent, String> {
  let model = build_model(ep).await?;
  Ok(
    AgentBuilder::new(model)
      .preamble(PREAMBLE)
      .default_max_turns(MAX_TURNS)
      .dynamic_tool(tool_query_videos())
      .dynamic_tool(tool_query_results())
      .dynamic_tool(tool_get_rewrite_result())
      .dynamic_tool(tool_list_proxies())
      .dynamic_tool(tool_start_rewrite_tasks())
      .dynamic_tool(tool_query_running_tasks())
      .dynamic_tool(tool_cron_update())
      .dynamic_tool(tool_fetch_account_videos())
      .dynamic_tool(tool_fetch_video_data())
      .dynamic_tool(tool_list_folder())
      .dynamic_tool(tool_read_text_file())
      .dynamic_tool(tool_vps_exec())
      .build(),
  )
}

/// 取当前 Agent：每次对话拉一次服务器端点配置（云端后台可随时改），
/// 指纹未变则复用缓存实例。
async fn current_agent() -> Result<Agent, String> {
  let config = AI_REWRITE
    .api_config()
    .await
    .map_err(|e| code_err("AI_AGENT_CONFIG_UNAVAILABLE", Some(&e)))?;
  let ep = &config.llm;
  let fingerprint = endpoint_fingerprint(ep);
  let mut cached = CACHED_AGENT.lock().await;
  if let Some((cached_fp, agent)) = cached.as_ref() {
    if *cached_fp == fingerprint {
      return Ok(agent.clone());
    }
  }
  let agent = build_agent(ep).await?;
  *cached = Some((fingerprint, agent.clone()));
  Ok(agent)
}

// ============================================================
// 工具
// ============================================================

fn tool_err(message: impl Into<String>) -> ToolExecutionError {
  ToolExecutionError::new(ToolErrorKind::Other, message)
}

fn arg_str(args: &Value, key: &str) -> Option<String> {
  args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn arg_u32(args: &Value, key: &str) -> Option<u32> {
  args
    .get(key)
    .and_then(Value::as_u64)
    .and_then(|v| u32::try_from(v).ok())
}

fn arg_i64(args: &Value, key: &str) -> Option<i64> {
  args.get(key).and_then(Value::as_i64)
}

/// 工具结果里的超长字符串截断，避免转写全文撑爆对话上下文。
fn truncate_json(value: &mut Value, max_chars: usize) {
  match value {
    Value::String(s) => {
      if s.chars().count() > max_chars {
        let kept: String = s.chars().take(max_chars).collect();
        *s = format!("{kept}…（已截断）");
      }
    }
    Value::Array(items) => items.iter_mut().for_each(|v| truncate_json(v, max_chars)),
    Value::Object(map) => map.values_mut().for_each(|v| truncate_json(v, max_chars)),
    _ => {}
  }
}

/// 数据范围口径：注入每个查询工具的返回里，让模型有据可依，
/// 防止把 account_id 误读成“公司ID”再向用户输出错误的数据归属结论。
const SCOPE_NOTE: &str = "数据范围：以下均为当前登录公司可见的数据（自有账号 + 对标账号），由服务器按登录账号过滤，不含其他公司数据。account_id 是账号入库编号，不是公司编号，不要据此推断数据归属。";

/// 在返回 JSON 顶层注入数据范围说明；非对象返回值原样保留。
fn with_scope_note(mut value: Value) -> Value {
  if let Value::Object(map) = &mut value {
    map.insert("_scope_note".to_string(), json!(SCOPE_NOTE));
  }
  value
}

async fn tool_output(value: Value) -> Result<ToolOutput, ToolExecutionError> {
  let mut value = with_scope_note(value);
  truncate_json(&mut value, 2000);
  Ok(ToolOutput::json(value))
}

fn tool_query_videos() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("query_videos").expect("tool name is static and valid"),
    "查询视频库（仅当前公司可见：自有账号 + 对标账号已入库的视频/任务列表）。可按平台、关键词、最小播放量过滤，支持分页。account_type 字段：own=自有账号，benchmark=对标账号。",
    json!({
        "type": "object",
        "properties": {
            "platform": { "type": "string", "description": "平台筛选，取值如 tiktok / youtube / douyin（可空）" },
            "keyword": { "type": "string", "description": "标题关键词（可空）" },
            "min_play": { "type": "integer", "description": "最小播放量（可空）" },
            "page": { "type": "integer", "description": "页码，从 1 开始，默认 1" },
            "page_size": { "type": "integer", "description": "每页条数，1-100，默认 20" }
        }
    }),
    move |args: Value| {
      Box::pin(async move {
        AI_REWRITE
          .ensure_login()
          .await
          .map_err(tool_err)?;
        let value = AI_REWRITE
          .fetch_videos(json!({
              "platform": arg_str(&args, "platform").unwrap_or_default(),
              "keyword": arg_str(&args, "keyword").unwrap_or_default(),
              "min_play": arg_i64(&args, "min_play").unwrap_or(0),
              "page": arg_u32(&args, "page").unwrap_or(1).max(1),
              "page_size": arg_u32(&args, "page_size").unwrap_or(20).clamp(1, 100),
              "skip_done": 0,
          }))
          .await
          .map_err(tool_err)?;
        tool_output(value).await
      })
    },
  )
}

fn tool_query_results() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("query_results").expect("tool name is static and valid"),
    "查询洗稿成果列表（仅当前公司可见：已完成转写 / 爆点分析 / 洗稿任务的历史记录）。可按关键词、状态过滤，支持分页。",
    json!({
        "type": "object",
        "properties": {
            "keyword": { "type": "string", "description": "标题关键词（可空）" },
            "status": { "type": "string", "description": "状态筛选（可空）" },
            "page": { "type": "integer", "description": "页码，从 1 开始，默认 1" },
            "page_size": { "type": "integer", "description": "每页条数，1-100，默认 20" }
        }
    }),
    move |args: Value| {
      Box::pin(async move {
        AI_REWRITE
          .ensure_login()
          .await
          .map_err(tool_err)?;
        let value = AI_REWRITE
          .fetch_results(json!({
              "keyword": arg_str(&args, "keyword").unwrap_or_default(),
              "status": arg_str(&args, "status").unwrap_or_default(),
              "only_mine": 0,
              "page": arg_u32(&args, "page").unwrap_or(1).max(1),
              "page_size": arg_u32(&args, "page_size").unwrap_or(20).clamp(1, 100),
          }))
          .await
          .map_err(tool_err)?;
        tool_output(value).await
      })
    },
  )
}

fn tool_get_rewrite_result() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("get_rewrite_result").expect("tool name is static and valid"),
    "查询单条洗稿成果详情（仅当前公司可见的任务）：转写全文、爆点分析、洗稿文案与打分。需要先从 query_results 拿到 task_id。",
    json!({
        "type": "object",
        "properties": {
            "task_id": { "type": "integer", "description": "成果任务 ID（query_results 返回的 task_id）" }
        },
        "required": ["task_id"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let task_id = arg_i64(&args, "task_id").ok_or_else(|| {
          tool_err("缺少 task_id 参数，请先调用 query_results 获取任务 ID")
        })?;
        AI_REWRITE
          .ensure_login()
          .await
          .map_err(tool_err)?;
        let value = AI_REWRITE
          .fetch_result(task_id)
          .await
          .map_err(tool_err)?;
        tool_output(value).await
      })
    },
  )
}

fn tool_list_proxies() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("list_proxies").expect("tool name is static and valid"),
    "查询代理中心当前可用的线路列表（名称、协议、地址、国家/城市）。只读，不包含账号密码。",
    json!({ "type": "object", "properties": {} }),
    move |_args: Value| {
      Box::pin(async move {
        let items = crate::bwbrowser_cloud::BWBROWSER_AUTH
          .list_cloud_proxies(None)
          .await
          .map_err(tool_err)?;
        let entries: Vec<Value> = items
          .iter()
          .map(|p| {
            json!({
                "name": p.proxy_name.clone().unwrap_or_else(|| format!("{}:{}", p.host, p.port)),
                "type": p.proxy_type,
                "host": p.host,
                "port": p.port,
                "country": p.country,
                "city": p.city,
            })
          })
          .collect();
        tool_output(json!({ "total": entries.len(), "proxies": entries })).await
      })
    },
  )
}

/// 一次发起洗稿任务的条数上限：防止单轮对话烧掉过多服务器额度。
const MAX_START_ITEMS: usize = 3;

fn tool_start_rewrite_tasks() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("start_rewrite_tasks").expect("tool name is static and valid"),
    "发起洗稿任务（后台执行，立即返回任务清单，任务会出现在应用任务列表并实时更新）。items 里每项需带 source_url（或 play_url），可带 video_id/title/platform。一次最多 3 条；用户已明确给出链接或点名视频时直接调用，无需再次确认。",
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "description": "要洗稿的视频列表（从 query_videos 的行里取，每项含 source_url）",
                "items": { "type": "object" }
            },
            "concurrency": { "type": "integer", "description": "并发数 1-6，默认 3（可省略）" }
        },
        "required": ["items"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let app = APP_HANDLE.get().ok_or_else(|| {
          tool_err("应用尚未就绪，请稍后重试")
        })?;
        let items = args
          .get("items")
          .and_then(Value::as_array)
          .ok_or_else(|| tool_err("缺少 items 参数（视频列表）"))?;
        if items.is_empty() {
          return Err(tool_err("items 为空：请先从视频库（query_videos）选定要处理的视频"));
        }
        let (items, truncated): (Vec<Value>, bool) = if items.len() > MAX_START_ITEMS {
          (items[..MAX_START_ITEMS].to_vec(), true)
        } else {
          (items.clone(), false)
        };
        let concurrency = arg_u32(&args, "concurrency")
          .map(|v| v as usize)
          .filter(|v| *v >= 1);
        let value = crate::ai_rewrite::ai_rewrite_run_links(
          app.clone(),
          items,
          concurrency,
          Some(false),
        )
        .await
        .map_err(tool_err)?;
        let mut value = value;
        if let Value::Object(map) = &mut value {
          if truncated {
            map.insert(
              "note".to_string(),
              json!(format!("一次最多发起 {MAX_START_ITEMS} 条，已只取前 {MAX_START_ITEMS} 条；请告知用户分批处理。")),
            );
          }
          map.insert(
            "hint".to_string(),
            json!("任务已开始并出现在应用任务列表（洗稿页任务 Tab），用户可直接看到实时进度。立即用 query_running_tasks 查一次当前阶段做简报；不要反问用户要不要跟踪进度；完成后用 query_results 查成果。"),
          );
        }
        tool_output(value).await
      })
    },
  )
}

fn tool_query_running_tasks() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("query_running_tasks").expect("tool name is static and valid"),
    "查询本机洗稿任务的实时进度（下载/转写/分析/洗稿/打分各阶段、百分比、错误信息）。stage=done 表示完成，error 表示失败。",
    json!({ "type": "object", "properties": {} }),
    move |_args: Value| {
      Box::pin(async move {
        let value = crate::ai_rewrite::ai_rewrite_running_tasks()
          .await
          .map_err(tool_err)?;
        let mut value = value;
        if let Value::Object(map) = &mut value {
          map.insert(
            "_scope_note".to_string(),
            json!("以下为本机最近发起的洗稿任务实时状态（含历史终态，2 小时后清除）。"),
          );
        }
        let mut value = value;
        truncate_json(&mut value, 2000);
        Ok(ToolOutput::json(value))
      })
    },
  )
}

/// 触发/查询云端平台数据更新（run 按次计费，status/prices 只读免费）。
fn tool_cron_update() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("cron_update").expect("tool name is static and valid"),
    "触发或查询平台数据更新（服务器端把本公司账号的最新数据入库）。action=run 按次计费且必须先征得用户同意；action=status 查执行进度（约 5 分钟内的调度节拍）；action=prices 查价目与余额。platform 取值 tiktok / youtube / douyin。",
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["run", "status", "prices"], "description": "run=触发更新（计费，需用户同意），status=查进度，prices=查价目余额" },
            "platform": { "type": "string", "enum": ["tiktok", "youtube", "douyin"], "description": "平台（run 和 status 必填）" },
            "type": { "type": "string", "enum": ["account", "benchmark", "both"], "description": "仅 run 有效：account=只更新账号，benchmark=只更新基准，both=全部（默认）" }
        },
        "required": ["action"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let action = arg_str(&args, "action").ok_or_else(|| tool_err("缺少 action 参数"))?;
        let api_action = match action.as_str() {
          "run" => "run_cron",
          "status" => "cron_status",
          "prices" => "prices",
          other => {
            return Err(tool_err(format!(
              "action 必须是 run / status / prices，收到：{other}"
            )))
          }
        };
        let platform = arg_str(&args, "platform");
        if api_action != "prices" && platform.as_deref().map(str::is_empty).unwrap_or(true) {
          return Err(tool_err("缺少 platform 参数（tiktok / youtube / douyin）"));
        }
        let mut params = json!({});
        if let Some(p) = platform {
          params["platform"] = json!(p);
        }
        if api_action == "run_cron" {
          let typ = arg_str(&args, "type").unwrap_or_else(|| "both".to_string());
          if !matches!(typ.as_str(), "account" | "benchmark" | "both") {
            return Err(tool_err(format!(
              "type 必须是 account / benchmark / both，收到：{typ}"
            )));
          }
          params["type"] = json!(typ);
        }
        AI_REWRITE
          .ensure_login()
          .await
          .map_err(tool_err)?;
        let mut value = AI_REWRITE
          .cron_call(api_action, params)
          .await
          .map_err(tool_err)?;
        if api_action == "run_cron" {
          if let Value::Object(map) = &mut value {
            map.insert(
              "hint".to_string(),
              json!("已加入服务器执行队列（只更新当前公司账号），约 5 分钟内被调度执行；稍后用 action=status 查进度，完成后可用 query_videos 查看新数据。"),
            );
          }
        }
        tool_output(value).await
      })
    },
  )
}

/// 实时拉取某账号主页最新视频（按次计费，上游为公司配置的 tikwm/tikhub/YouTube API）。
fn tool_fetch_account_videos() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("fetch_account_videos").expect("tool name is static and valid"),
    "实时拉取某个账号主页的最新视频列表（每条含播放量、点赞、评论、发布时间、链接）。优先用 account_id（账号库编号，query_videos 返回里有）；没有编号时传 username。每次调用按次计费（调用公司上游数据 API），调用前须征得用户同意。",
    json!({
        "type": "object",
        "properties": {
            "platform": { "type": "string", "enum": ["tiktok", "youtube", "douyin"], "description": "平台" },
            "account_id": { "type": "integer", "description": "账号库编号（优先；查自有/对标账号表，限当前公司）" },
            "account_type": { "type": "string", "enum": ["own", "benchmark"], "description": "账号归属：own=自有，benchmark=对标（account_id 冲突时用）" },
            "username": { "type": "string", "description": "账号名（没有 account_id 时用，tiktok/youtube 直接用，douyin 需账号库里有 sec_user_id）" },
            "count": { "type": "integer", "description": "拉取条数 1-30，默认 10" }
        },
        "required": ["platform"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let platform = arg_str(&args, "platform")
          .ok_or_else(|| tool_err("缺少 platform 参数"))?;
        let account_id = arg_i64(&args, "account_id").unwrap_or(0);
        if account_id <= 0 && arg_str(&args, "username").is_none() {
          return Err(tool_err(
            "需要 account_id（从 query_videos 结果里拿）或 username 参数",
          ));
        }
        let mut params = json!({
            "action": "account_videos",
            "platform": platform,
        });
        if account_id > 0 {
          params["account_id"] = json!(account_id);
        }
        if let Some(t) = arg_str(&args, "account_type") {
          params["account_type"] = json!(t);
        }
        if let Some(u) = arg_str(&args, "username") {
          params["username"] = json!(u);
        }
        if let Some(c) = arg_u32(&args, "count") {
          params["count"] = json!(c.clamp(1, 30));
        }
        AI_REWRITE
          .ensure_login()
          .await
          .map_err(tool_err)?;
        let value = AI_REWRITE
          .cron_call("account_videos", params)
          .await
          .map_err(tool_err)?;
        tool_output(value).await
      })
    },
  )
}

/// 实时拉取某视频的最新数据（播放量等，按次计费）。
fn tool_fetch_video_data() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("fetch_video_data").expect("tool name is static and valid"),
    "实时拉取某个视频的最新数据（播放量、点赞、评论、发布时间）。传 video_id 或 video_url；platform=tiktok 时还需 account_id 或 username（在该账号最新 30 条主页视频里匹配）。每次调用按次计费（调用公司上游数据 API），调用前须征得用户同意。",
    json!({
        "type": "object",
        "properties": {
            "platform": { "type": "string", "enum": ["tiktok", "youtube", "douyin"], "description": "平台" },
            "video_id": { "type": "string", "description": "视频 ID（或从 video_url 解析）" },
            "video_url": { "type": "string", "description": "视频链接（没有 video_id 时用它解析）" },
            "account_id": { "type": "integer", "description": "账号库编号（platform=tiktok 时必填其一）" },
            "username": { "type": "string", "description": "账号名（platform=tiktok 时必填其一）" }
        },
        "required": ["platform"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let platform = arg_str(&args, "platform")
          .ok_or_else(|| tool_err("缺少 platform 参数"))?;
        let video_id = arg_str(&args, "video_id");
        let video_url = arg_str(&args, "video_url");
        if video_id.is_none() && video_url.is_none() {
          return Err(tool_err("需要 video_id 或 video_url 参数"));
        }
        if platform == "tiktok"
          && arg_i64(&args, "account_id").unwrap_or(0) <= 0
          && arg_str(&args, "username").is_none()
        {
          return Err(tool_err(
            "platform=tiktok 时需要 account_id 或 username（在该账号主页视频里匹配该视频）",
          ));
        }
        let mut params = json!({
            "action": "video_data",
            "platform": platform,
        });
        if let Some(v) = video_id {
          params["video_id"] = json!(v);
        }
        if let Some(v) = video_url {
          params["video_url"] = json!(v);
        }
        if let Some(a) = arg_i64(&args, "account_id") {
          if a > 0 {
            params["account_id"] = json!(a);
          }
        }
        if let Some(u) = arg_str(&args, "username") {
          params["username"] = json!(u);
        }
        if let Some(t) = arg_str(&args, "account_type") {
          params["account_type"] = json!(t);
        }
        AI_REWRITE
          .ensure_login()
          .await
          .map_err(tool_err)?;
        let value = AI_REWRITE
          .cron_call("video_data", params)
          .await
          .map_err(tool_err)?;
        tool_output(value).await
      })
    },
  )
}

fn tool_list_folder() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("list_folder").expect("tool name is static and valid"),
    "列出用户已授权的本地文件夹（或其子目录）的内容。只能访问用户通过「选择文件夹」按钮授权的目录。",
    json!({
        "type": "object",
        "properties": {
            "folder": { "type": "string", "description": "要列出的文件夹完整路径（必须是已授权目录或其子目录）" }
        },
        "required": ["folder"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let folder = arg_str(&args, "folder")
          .ok_or_else(|| tool_err("缺少 folder 参数"))?;
        let allowed = ALLOWED_FOLDERS.lock().await.clone();
        let value = list_folder_impl(&folder, &allowed)
          .await
          .map_err(tool_err)?;
        tool_output(value).await
      })
    },
  )
}

fn tool_read_text_file() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("read_text_file").expect("tool name is static and valid"),
    "读取用户已授权文件夹内的单个文本文件（≤512KB，纯文本）。二进制文件会被拒绝。",
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "文件的完整路径（必须在已授权目录内）" }
        },
        "required": ["path"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let path = arg_str(&args, "path").ok_or_else(|| tool_err("缺少 path 参数"))?;
        let allowed = ALLOWED_FOLDERS.lock().await.clone();
        let value = read_text_file_impl(&path, &allowed)
          .await
          .map_err(tool_err)?;
        tool_output(value).await
      })
    },
  )
}

fn tool_vps_exec() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("vps_exec").expect("tool name is static and valid"),
    "在用户自己提供的 VPS 上经 SSH 执行一条命令（连接信息必须来自用户的对话内容）。返回 stdout/stderr/exit_code。",
    json!({
        "type": "object",
        "properties": {
            "host": { "type": "string", "description": "SSH 主机地址" },
            "port": { "type": "integer", "description": "SSH 端口，默认 22（可省略）" },
            "username": { "type": "string", "description": "SSH 用户名" },
            "password": { "type": "string", "description": "SSH 密码（来自用户对话）" },
            "command": { "type": "string", "description": "要执行的命令（破坏性命令须先征得用户同意）" }
        },
        "required": ["host", "username", "password", "command"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let host = arg_str(&args, "host").ok_or_else(|| tool_err("缺少 host 参数"))?;
        let port = arg_u32(&args, "port").unwrap_or(22) as u16;
        let username = arg_str(&args, "username").ok_or_else(|| tool_err("缺少 username 参数"))?;
        let password = arg_str(&args, "password").ok_or_else(|| tool_err("缺少 password 参数"))?;
        let command = arg_str(&args, "command").ok_or_else(|| tool_err("缺少 command 参数"))?;
        let value = vps_exec_impl(&host, port, &username, &password, &command)
          .await
          .map_err(tool_err)?;
        tool_output(value).await
      })
    },
  )
}

// ============================================================
// 对话命令
// ============================================================

/// 从 rig 的运行消息里提取工具调用 / 结果，作为前端可展示的步骤。
fn steps_from_messages(messages: &[Message]) -> Vec<AgentChatStep> {
  const DETAIL_MAX_CHARS: usize = 400;
  // 工具参数里的密码不在前端步骤区明文展示（对话内容本身由用户自己掌握）。
  let mask_args = |mut args: Value| -> Value {
    if let Value::Object(map) = &mut args {
      if map.contains_key("password") {
        map.insert("password".to_string(), json!("***"));
      }
    }
    args
  };
  let mut steps = Vec::new();
  for message in messages {
    match message {
      Message::Assistant(turn) => {
        for call in turn.tool_calls() {
          let args = mask_args(Value::Object(call.function.arguments.clone()));
          steps.push(AgentChatStep {
            kind: "tool_call".to_string(),
            name: call.function.name.to_string(),
            detail: args.to_string(),
            is_error: false,
          });
        }
      }
      Message::User { content } => {
        for item in content {
          if let UserContent::ToolResult(RigToolResult {
            name,
            content: items,
            is_error,
            ..
          }) = item
          {
            let text = items
              .iter()
              .filter_map(|c| match c {
                ToolResultContent::Text(t) => Some(t.text.clone()),
                _ => None,
              })
              .collect::<Vec<_>>()
              .join("\n");
            let detail: String = text.chars().take(DETAIL_MAX_CHARS).collect();
            let detail = if text.chars().count() > DETAIL_MAX_CHARS {
              format!("{detail}…")
            } else {
              detail
            };
            steps.push(AgentChatStep {
              kind: "tool_result".to_string(),
              name: name.to_string(),
              detail,
              is_error: *is_error,
            });
          }
        }
      }
      _ => {}
    }
  }
  steps
}

/// AI 助手对话：前端全量传入历史（最后一条必须是 user）与已授权的本地文件夹，
/// 返回最终回复与工具执行步骤。历史由前端持有，后端不落盘。
#[tauri::command]
pub async fn ai_agent_chat(
  app: tauri::AppHandle,
  messages: Vec<AgentChatTurn>,
  allowed_folders: Option<Vec<String>>,
) -> Result<AgentChatReply, String> {
  let _ = APP_HANDLE.set(app);
  {
    let mut folders = ALLOWED_FOLDERS.lock().await;
    *folders = allowed_folders.unwrap_or_default();
  }
  let Some(last) = messages.last() else {
    return Err(code_err("AI_AGENT_EMPTY_CONVERSATION", None));
  };
  if last.role != "user" || last.content.trim().is_empty() {
    return Err(code_err("AI_AGENT_EMPTY_CONVERSATION", None));
  }
  let agent = current_agent().await?;
  let mut history: Vec<Message> = messages[..messages.len() - 1]
    .iter()
    .map(|t| {
      if t.role == "assistant" {
        Message::assistant(t.content.trim())
      } else {
        Message::user(t.content.trim())
      }
    })
    .collect();
  let prompt = last.content.trim();
  let response = agent
    .chat(prompt, &mut history)
    .await
    .map_err(|e| code_err("AI_AGENT_LLM_FAILED", Some(&e.to_string())))?;
  Ok(AgentChatReply {
    reply: response.output(),
    steps: steps_from_messages(response.messages()),
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn truncate_json_caps_long_strings() {
    let mut value = json!({ "text": "字".repeat(3000), "list": ["x".repeat(2500)] });
    truncate_json(&mut value, 2000);
    let obj = value.as_object().unwrap();
    let text = obj["text"].as_str().unwrap();
    assert!(text.chars().count() < 2010 && text.ends_with("（已截断）"));
    let item = obj["list"][0].as_str().unwrap();
    assert!(item.chars().count() < 2010);
  }

  #[test]
  fn truncate_json_keeps_short_values() {
    let mut value = json!({ "a": "short", "n": 1, "b": [true, null] });
    let before = value.clone();
    truncate_json(&mut value, 2000);
    assert_eq!(value, before);
  }

  #[test]
  fn mask_proxy_url_hides_password() {
    assert_eq!(
      mask_proxy_url("socks5://alice:s3cret@1.2.3.4:1080"),
      "socks5://alice:***@1.2.3.4:1080"
    );
    // 无凭据、无 scheme 的形式原样保留。
    assert_eq!(
      mask_proxy_url("socks5://1.2.3.4:1080"),
      "socks5://1.2.3.4:1080"
    );
    assert_eq!(mask_proxy_url("1.2.3.4:1080"), "1.2.3.4:1080");
  }

  #[test]
  fn scope_note_injected_into_objects_only() {
    let with = with_scope_note(json!({ "total": 3, "items": [] }));
    assert!(with["_scope_note"].as_str().unwrap().contains("account_id"));
    assert!(with.get("total").is_some());
    // 非对象返回值原样保留（服务端偶发返回数组/标量时不炸）。
    assert_eq!(with_scope_note(json!([1, 2])), json!([1, 2]));
  }

  #[test]
  fn endpoint_fingerprint_changes_with_config() {
    let mut ep = AiEndpointConfig::default();
    let a = endpoint_fingerprint(&ep);
    ep.model = "gpt-x".to_string();
    let b = endpoint_fingerprint(&ep);
    assert_ne!(a, b);
  }
}
