//! Doubao web chat protocol (port of DoubaoManager `src/doupool/video/protocol.py`).
//!
//! Everything here is a pure JSON transformation: payload construction for
//! `/chat/completion`, and parsing of the SSE stream / pull responses. The
//! executor (CDP injection) is responsible for actually sending these payloads
//! inside a logged-in Doubao page.

use serde::Serialize;

/// Bot id of the Doubao web video agent.
pub const DOUBAO_BOT_ID: &str = "7338286299411103781";

/// One reference image prepared for upload. `uri` is the Doubao-side upload
/// uri, `identifier` the client-generated attachment id; both must exist
/// before the completion payload can be built.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DoubaoImageRef {
  pub uri: String,
  pub identifier: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub name: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub width: Option<i64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub height: Option<i64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub url: Option<String>,
}

/// Classified failure of the chat protocol. The executor maps these onto task
/// retries (rate limited / too many), user actions (verify) or a plain fail.
#[derive(Debug, Clone, PartialEq)]
pub enum DoubaoProtocolError {
  /// Doubao demands a human captcha; the user must solve it in the browser.
  VerifyRequired(String),
  /// Account hit its generation quota ("rate limited" / "额度").
  RateLimited(String),
  /// Sending too fast ("频繁" / "too many" / "frequency").
  TooManyRequests(String),
  /// The streamed ACK stream never carried a usable event.
  MissingAck,
  /// Any other STREAM_ERROR message.
  Message(String),
}

impl std::fmt::Display for DoubaoProtocolError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::VerifyRequired(m) => write!(f, "豆包要求人工验证码：{m}"),
      Self::RateLimited(m) => write!(f, "账号额度受限：{m}"),
      Self::TooManyRequests(m) => write!(f, "发送过于频繁：{m}"),
      Self::MissingAck => write!(f, "豆包响应缺少 SSE_ACK"),
      Self::Message(m) => write!(f, "{m}"),
    }
  }
}

impl std::error::Error for DoubaoProtocolError {}

fn uuid_str() -> String {
  uuid::Uuid::new_v4().to_string()
}

fn now_ms() -> u64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis() as u64)
    .unwrap_or(0)
}

fn random_local_conversation_id() -> String {
  let n = rand::random::<u64>() % 9_000_000_000_000_000 + 1_000_000_000_000_000;
  format!("local_{n}")
}

/// `{"ratio":...,"model":...,"duration":...}` built by hand to keep the exact
/// key order the original client sends (serde_json would sort keys).
fn ability_param(ratio: &str, model: &str, duration: u32) -> String {
  format!(r#"{{"ratio":"{ratio}","model":"{model}","duration":{duration}}}"#)
}

fn base_option(
  now_ms: u64,
  unique_key: &str,
  need_create_conversation: bool,
  collect_id: &str,
) -> serde_json::Value {
  serde_json::json!({
    "send_message_scene": "",
    "create_time_ms": now_ms,
    "collect_id": collect_id,
    "is_audio": false,
    "answer_with_suggest": false,
    "tts_switch": false,
    "need_deep_think": 0,
    "click_clear_context": false,
    "from_suggest": false,
    "is_regen": false,
    "is_replace": false,
    "is_from_click_option": false,
    "is_from_click_softlink": false,
    "disable_sse_cache": false,
    "select_text_action": "",
    "is_select_text": false,
    "resend_for_regen": false,
    "scene_type": 0,
    "unique_key": unique_key,
    "start_seq": 0,
    "need_create_conversation": need_create_conversation,
    "conversation_init_option": { "need_ack_conversation": true },
    "regen_query_id": [],
    "edit_query_id": [],
    "regen_instruction": "",
    "no_replace_for_regen": false,
    "message_from": 0,
    "shared_app_name": "",
    "shared_app_id": "",
    "sse_recv_event_options": { "support_chunk_delta": true },
    "is_ai_playground": false,
    "is_old_user": true,
    "recovery_option": {
      "is_recovery": false,
      "req_create_time_sec": now_ms / 1000,
      "append_sse_event_scene": 0,
    },
    "message_storage_type": 0,
  })
}

fn text_message(prompt: &str, ratio: &str) -> serde_json::Value {
  let message_text = format!("生成视频：{prompt}，{ratio}");
  serde_json::json!({
    "local_message_id": uuid_str(),
    "content_block": [{
      "block_type": 10000,
      "content": {
        "text_block": {
          "text": message_text,
          "icon_url": "",
          "icon_url_dark": "",
          "summary": "",
        },
        "pc_event_block": "",
      },
      "block_id": uuid_str(),
      "parent_id": "",
      "meta_info": [],
      "append_fields": [],
    }],
    "message_status": 0,
  })
}

fn attachment_message(images: &[DoubaoImageRef]) -> serde_json::Value {
  let attachments: Vec<serde_json::Value> = images
    .iter()
    .map(|image| {
      let mut image_ori = serde_json::Map::new();
      if let Some(url) = &image.url {
        image_ori.insert("url".into(), serde_json::json!(url));
      }
      if let Some(width) = image.width {
        image_ori.insert("width".into(), serde_json::json!(width));
      }
      if let Some(height) = image.height {
        image_ori.insert("height".into(), serde_json::json!(height));
      }
      serde_json::json!({
        "type": 1,
        "identifier": image.identifier,
        "image": {
          "name": image.name.clone().unwrap_or_else(|| "image.png".to_string()),
          "uri": image.uri,
          "image_ori": image_ori,
        },
        "parse_state": 0,
        "review_state": 1,
        "upload_status": 1,
        "progress": 100,
        "src": "",
      })
    })
    .collect();
  serde_json::json!({
    "local_message_id": uuid_str(),
    "content_block": [{
      "block_type": 10052,
      "content": {
        "attachment_block": { "attachments": attachments },
        "pc_event_block": "",
      },
      "block_id": uuid_str(),
      "parent_id": "",
      "meta_info": [],
      "append_fields": [],
    }],
    "message_status": 0,
  })
}

/// Arguments for one `/chat/completion` video-generation send.
pub struct CompletionParams<'a> {
  pub prompt: &'a str,
  pub model: &'a str,
  pub ratio: &'a str,
  pub duration: u32,
  /// Doubao page fingerprint (ext.fp).
  pub fingerprint: &'a str,
  /// "t2v" (no images) or "i2v" (1-9 images).
  pub mode: &'a str,
  pub images: &'a [DoubaoImageRef],
  /// Existing conversation to continue; empty starts a new one.
  pub conversation_id: Option<&'a str>,
  pub need_create_conversation: bool,
}

/// Build the `/chat/completion` body that starts a video generation.
pub fn build_completion_payload(params: CompletionParams<'_>) -> Result<serde_json::Value, String> {
  let CompletionParams {
    prompt,
    model,
    ratio,
    duration,
    fingerprint,
    mode,
    images,
    conversation_id,
    need_create_conversation,
  } = params;
  let prompt = prompt.trim();
  if prompt.is_empty() {
    return Err("prompt is required".into());
  }
  if model != "seedance_v2.0_std" && model != "seedance_v2.0" && model != "seedance_v2.0_mini" {
    return Err("unsupported model".into());
  }
  if !crate::doubao_manager::RATIOS.contains(&ratio) {
    return Err("unsupported ratio".into());
  }
  if !crate::doubao_manager::DURATIONS.contains(&duration) {
    return Err("unsupported duration".into());
  }
  if mode != "t2v" && mode != "i2v" {
    return Err("unsupported mode".into());
  }
  if mode == "i2v" {
    if images.is_empty() {
      return Err("image is required for i2v".into());
    }
    if images.len() > crate::doubao_manager::MAX_I2V_IMAGES {
      return Err(format!(
        "i2v supports at most {} images",
        crate::doubao_manager::MAX_I2V_IMAGES
      ));
    }
    for image in images {
      if image.uri.is_empty() || image.identifier.is_empty() {
        return Err("image uri and identifier are required".into());
      }
    }
  } else if !images.is_empty() {
    return Err("images are only supported for i2v".into());
  }

  let now = now_ms();
  let collect_id = if images.is_empty() {
    String::new()
  } else {
    uuid_str()
  };
  let mut messages = Vec::with_capacity(2);
  if !images.is_empty() {
    messages.push(attachment_message(images));
  }
  messages.push(text_message(prompt, ratio));

  Ok(serde_json::json!({
    "client_meta": {
      "local_conversation_id": random_local_conversation_id(),
      "conversation_id": conversation_id.unwrap_or(""),
      "bot_id": DOUBAO_BOT_ID,
      "last_section_id": "",
      "last_message_index": null,
    },
    "messages": messages,
    "option": base_option(now, &uuid_str(), need_create_conversation, &collect_id),
    "chat_ability": {
      "ability_type": 17,
      "ability_param": ability_param(ratio, model, duration),
    },
    "user_context": [],
    "ext": {
      "answer_with_suggest": "0",
      "fp": fingerprint,
      "sub_conv_firstmet_type": "1",
      "collection_id": collect_id,
      "conversation_init_option": r#"{"need_ack_conversation":true}"#,
      "commerce_credit_config_enable": "0",
    },
  }))
}

/// Reply payload that answers Doubao's "confirm parameters" step.
pub fn build_confirm_payload(conversation_id: &str, fingerprint: &str) -> serde_json::Value {
  let now = now_ms();
  serde_json::json!({
    "client_meta": {
      "local_conversation_id": random_local_conversation_id(),
      "conversation_id": conversation_id,
      "bot_id": DOUBAO_BOT_ID,
      "last_section_id": "",
      "last_message_index": null,
    },
    "messages": [
      {
        "local_message_id": uuid_str(),
        "content_block": [{
          "block_type": 10000,
          "content": {
            "text_block": {
              "text": "确认",
              "icon_url": "",
              "icon_url_dark": "",
              "summary": "",
            },
            "pc_event_block": "",
          },
          "block_id": uuid_str(),
          "parent_id": "",
          "meta_info": [],
          "append_fields": [],
        }],
        "message_status": 0,
      }
    ],
    "option": base_option(now, &uuid_str(), false, ""),
    "chat_ability": {
      "ability_type": 17,
      "ability_param": ability_param("1:1", "seedance_v2.0_mini", 5),
    },
    "user_context": [],
    "ext": {
      "answer_with_suggest": "0",
      "fp": fingerprint,
      "sub_conv_firstmet_type": "1",
      "collection_id": "",
      "conversation_init_option": r#"{"need_ack_conversation":true}"#,
      "commerce_credit_config_enable": "0",
    },
  })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SseAck {
  pub conversation_id: String,
  pub section_id: String,
  pub question_id: String,
}

fn stream_error_to_protocol_error(data: &str) -> DoubaoProtocolError {
  let error: serde_json::Value = serde_json::from_str(data).unwrap_or(serde_json::json!({}));
  let message = error
    .get("error_msg")
    .and_then(|v| v.as_str())
    .unwrap_or("豆包接口返回错误")
    .to_string();
  let lowered = message.to_lowercase();
  // 人机验证：rate limited + decision.type == "verify"（加载 rmc 验证码 iframe）
  if let Some(decision_raw) = error
    .get("extra")
    .and_then(|extra| extra.get("decision"))
    .and_then(|v| v.as_str())
  {
    if let Ok(decision) = serde_json::from_str::<serde_json::Value>(decision_raw) {
      if decision
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t.eq_ignore_ascii_case("verify"))
        .unwrap_or(false)
      {
        let detail = decision
          .get("detail")
          .and_then(|v| v.as_str())
          .unwrap_or(&message)
          .to_string();
        return DoubaoProtocolError::VerifyRequired(detail);
      }
    }
  }
  if lowered.contains("rate limited") || message.contains("额度") {
    return DoubaoProtocolError::RateLimited(message);
  }
  if message.contains("频繁") || lowered.contains("too many") || lowered.contains("frequency") {
    return DoubaoProtocolError::TooManyRequests(message);
  }
  DoubaoProtocolError::Message(message)
}

/// Parse the SSE text captured for one send until an ACK or error arrives.
pub fn parse_sse_ack(text: &str) -> Result<SseAck, DoubaoProtocolError> {
  for packet in text.replace("\r\n", "\n").split("\n\n") {
    let mut event = String::new();
    let mut data = String::new();
    for line in packet.lines() {
      if let Some(rest) = line.strip_prefix("event:") {
        event = rest.trim().to_string();
      } else if let Some(rest) = line.strip_prefix("data:") {
        data.push_str(rest.trim());
      }
    }
    if event == "STREAM_ERROR" {
      return Err(stream_error_to_protocol_error(&data));
    }
    if event == "SSE_ACK" {
      let payload: serde_json::Value = serde_json::from_str(&data)
        .map_err(|_| DoubaoProtocolError::Message("豆包响应缺少 SSE_ACK".to_string()))?;
      let meta = payload.get("ack_client_meta").cloned().unwrap_or_default();
      let question_id = payload
        .get("query_list")
        .and_then(|q| q.as_array())
        .and_then(|list| list.first())
        .and_then(|first| first.get("question_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
      return Ok(SseAck {
        conversation_id: meta
          .get("conversation_id")
          .and_then(|v| v.as_str())
          .unwrap_or("")
          .to_string(),
        section_id: meta
          .get("section_id")
          .and_then(|v| v.as_str())
          .unwrap_or("")
          .to_string(),
        question_id,
      });
    }
  }
  Err(DoubaoProtocolError::MissingAck)
}

/// Doubao messages carry blocks either as a direct `content_block` list (new)
/// or as a JSON string in `content` (legacy).
fn message_blocks(message: &serde_json::Value) -> Vec<serde_json::Value> {
  if let Some(blocks) = message.get("content_block").and_then(|v| v.as_array()) {
    return blocks.clone();
  }
  if let Some(content) = message.get("content").and_then(|v| v.as_str()) {
    if let Ok(serde_json::Value::Array(blocks)) = serde_json::from_str::<serde_json::Value>(content)
    {
      return blocks;
    }
  }
  Vec::new()
}

fn downlink_messages(response: &serde_json::Value) -> Vec<&serde_json::Value> {
  response
    .get("downlink_body")
    .and_then(|b| b.get("pull_singe_chain_downlink_body"))
    .and_then(|b| b.get("messages"))
    .and_then(|m| m.as_array())
    .map(|a| a.iter().collect())
    .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreationResult {
  pub remote_task_id: String,
  pub vid: String,
  pub fallback_result_url: String,
  pub cover_url: String,
}

/// First finished video creation (status 3 + download_url) in a pull response.
pub fn parse_creation_result(response: &serde_json::Value) -> Option<CreationResult> {
  for message in downlink_messages(response) {
    for block in message_blocks(message) {
      let creations = block
        .get("content")
        .and_then(|c| c.get("creation_block"))
        .and_then(|c| c.get("creations"))
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
      for creation in creations {
        let video = creation.get("video").cloned().unwrap_or_default();
        if video.get("status").and_then(|v| v.as_i64()) == Some(3) {
          if let Some(download_url) = video.get("download_url").and_then(|v| v.as_str()) {
            return Some(CreationResult {
              remote_task_id: creation
                .get("id")
                .map(|v| v.to_string())
                .unwrap_or_default()
                .trim_matches('"')
                .to_string(),
              vid: video
                .get("vid")
                .map(|v| v.to_string())
                .unwrap_or_default()
                .trim_matches('"')
                .to_string(),
              fallback_result_url: download_url.to_string(),
              cover_url: video
                .get("cover")
                .and_then(|c| c.get("image_thumb"))
                .and_then(|t| t.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            });
          }
        }
      }
    }
  }
  None
}

/// Doubao sometimes replies with a parameter-confirmation message and only
/// starts generating after the user replies "确认". 页面文案变过几次
/// （「确认参数」「确认生成」…），命中任意一个都算在等确认；被动监听的页面
/// 扫描也用这份清单（doubao_executor::confirmation_cue）。
pub const CONFIRMATION_MARKERS: [&str; 6] = [
  "确认参数",
  "确认后我再开始生成",
  "参数确认",
  "确认生成",
  "确认后开始",
  "回复「确认」",
];

pub fn is_awaiting_confirmation(response: &serde_json::Value) -> bool {
  for message in downlink_messages(response) {
    for block in message_blocks(message) {
      if let Some(text) = block
        .get("content")
        .and_then(|c| c.get("text_block"))
        .and_then(|t| t.get("text"))
        .and_then(|v| v.as_str())
      {
        if CONFIRMATION_MARKERS
          .iter()
          .any(|marker| text.contains(marker))
        {
          return true;
        }
      }
    }
  }
  false
}

/// Id of the "我的创作" cloud-drive directory under the file API response.
pub fn find_creation_directory(response: &serde_json::Value) -> Option<String> {
  if response.get("code").and_then(|v| v.as_i64()) != Some(0) {
    return None;
  }
  for child in response
    .get("data")
    .and_then(|d| d.get("children"))
    .and_then(|c| c.as_array())
    .into_iter()
    .flatten()
  {
    if child.get("name").and_then(|v| v.as_str()) == Some("我的创作") {
      return child
        .get("id")
        .map(|v| v.to_string())
        .map(|s| s.trim_matches('"').to_string())
        .filter(|s| !s.is_empty());
    }
  }
  None
}

/// File-API node whose `key` matches `vid` (video nodes only).
pub fn find_video_node(response: &serde_json::Value, vid: &str) -> Option<String> {
  if response.get("code").and_then(|v| v.as_i64()) != Some(0) {
    return None;
  }
  for child in response
    .get("data")
    .and_then(|d| d.get("children"))
    .and_then(|c| c.as_array())
    .into_iter()
    .flatten()
  {
    if child.get("key").and_then(|v| v.as_str()) == Some(vid) {
      return child
        .get("id")
        .map(|v| v.to_string())
        .map(|s| s.trim_matches('"').to_string())
        .filter(|s| !s.is_empty());
    }
  }
  None
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreationNode {
  pub vid: String,
  pub node_id: String,
  pub name: String,
}

/// Children of the creation directory (`key` == vid; keyless entries are
/// directories and skipped).
pub fn parse_creation_nodes(response: &serde_json::Value) -> Vec<CreationNode> {
  if response.get("code").and_then(|v| v.as_i64()) != Some(0) {
    return Vec::new();
  }
  response
    .get("data")
    .and_then(|d| d.get("children"))
    .and_then(|c| c.as_array())
    .map(|children| {
      children
        .iter()
        .filter_map(|child| {
          let vid = child.get("key").and_then(|v| v.as_str())?;
          if vid.is_empty() {
            return None;
          }
          Some(CreationNode {
            vid: vid.to_string(),
            node_id: child
              .get("id")
              .map(|v| {
                v.as_str()
                  .map(|s| s.to_string())
                  .unwrap_or_else(|| v.to_string().trim_matches('"').to_string())
              })
              .unwrap_or_default(),
            name: child
              .get("name")
              .and_then(|v| v.as_str())
              .unwrap_or("")
              .to_string(),
          })
        })
        .collect()
    })
    .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DownloadInfo {
  pub result_url: String,
  pub backup_result_url: String,
}

/// `main_url`/`backup_url` pair from the download-info API response.
pub fn parse_download_info(response: &serde_json::Value) -> Option<DownloadInfo> {
  if response.get("code").and_then(|v| v.as_i64()) != Some(0) {
    return None;
  }
  let infos = response
    .get("data")
    .and_then(|d| d.get("download_infos"))
    .and_then(|v| v.as_array())?;
  let first = infos.first()?;
  let main_url = first.get("main_url").and_then(|v| v.as_str())?;
  if main_url.is_empty() {
    return None;
  }
  Some(DownloadInfo {
    result_url: main_url.to_string(),
    backup_result_url: first
      .get("backup_url")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_string(),
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn completion_payload_rejects_bad_input() {
    let build = |prompt: &str,
                 model: &str,
                 ratio: &str,
                 duration: u32,
                 mode: &str,
                 images: &[DoubaoImageRef]| {
      build_completion_payload(CompletionParams {
        prompt,
        model,
        ratio,
        duration,
        fingerprint: "fp",
        mode,
        images,
        conversation_id: None,
        need_create_conversation: true,
      })
    };
    let err = build("", "seedance_v2.0_mini", "1:1", 5, "t2v", &[]).unwrap_err();
    assert_eq!(err, "prompt is required");
    let err = build("p", "bad", "1:1", 5, "t2v", &[]).unwrap_err();
    assert_eq!(err, "unsupported model");
    let err = build("p", "seedance_v2.0_mini", "9:99", 5, "t2v", &[]).unwrap_err();
    assert_eq!(err, "unsupported ratio");
    let err = build("p", "seedance_v2.0_mini", "1:1", 7, "t2v", &[]).unwrap_err();
    assert_eq!(err, "unsupported duration");
    let err = build("p", "seedance_v2.0_mini", "1:1", 5, "x", &[]).unwrap_err();
    assert_eq!(err, "unsupported mode");
    let err = build("p", "seedance_v2.0_mini", "1:1", 5, "t2v", &[test_image()]).unwrap_err();
    assert_eq!(err, "images are only supported for i2v");
    let err = build("p", "seedance_v2.0_mini", "1:1", 5, "i2v", &[]).unwrap_err();
    assert_eq!(err, "image is required for i2v");
  }

  fn test_image() -> DoubaoImageRef {
    DoubaoImageRef {
      uri: "tos://x/abc".into(),
      identifier: "id-1".into(),
      name: Some("a.png".into()),
      width: Some(100),
      height: Some(200),
      url: Some("https://x/a.png".into()),
    }
  }

  #[test]
  fn t2v_payload_shape() {
    let payload = build_completion_payload(CompletionParams {
      prompt: " 一只猫 ",
      model: "seedance_v2.0_mini",
      ratio: "9:16",
      duration: 5,
      fingerprint: "fp-1",
      mode: "t2v",
      images: &[],
      conversation_id: None,
      need_create_conversation: true,
    })
    .unwrap();
    assert_eq!(payload["client_meta"]["bot_id"], DOUBAO_BOT_ID);
    let messages = payload["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content_block"][0]["block_type"], 10000);
    let text = messages[0]["content_block"][0]["content"]["text_block"]["text"]
      .as_str()
      .unwrap();
    assert_eq!(text, "生成视频：一只猫，9:16");
    assert_eq!(
      payload["chat_ability"]["ability_param"],
      r#"{"ratio":"9:16","model":"seedance_v2.0_mini","duration":5}"#
    );
    assert_eq!(payload["option"]["collect_id"], "");
    assert_eq!(payload["option"]["need_create_conversation"], true);
    assert_eq!(
      payload["ext"]["conversation_init_option"],
      r#"{"need_ack_conversation":true}"#
    );
  }

  #[test]
  fn i2v_payload_has_attachment_first() {
    let payload = build_completion_payload(CompletionParams {
      prompt: "p",
      model: "seedance_v2.0",
      ratio: "1:1",
      duration: 10,
      fingerprint: "fp",
      mode: "i2v",
      images: &[test_image()],
      conversation_id: Some("conv-1"),
      need_create_conversation: false,
    })
    .unwrap();
    let messages = payload["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["content_block"][0]["block_type"], 10052);
    let attachments = messages[0]["content_block"][0]["content"]["attachment_block"]["attachments"]
      .as_array()
      .unwrap();
    assert_eq!(attachments[0]["image"]["uri"], "tos://x/abc");
    assert_eq!(attachments[0]["image"]["image_ori"]["width"], 100);
    assert_eq!(messages[1]["content_block"][0]["block_type"], 10000);
    assert_eq!(payload["client_meta"]["conversation_id"], "conv-1");
    assert_eq!(payload["option"]["need_create_conversation"], false);
    assert_ne!(payload["option"]["collect_id"], "");
  }

  #[test]
  fn sse_ack_parsed() {
    let text = "event: SSE_ACK\r\ndata: {\"ack_client_meta\":{\"conversation_id\":\"c1\",\"section_id\":\"s1\"},\"query_list\":[{\"question_id\":\"q1\"}]}\n\n";
    let ack = parse_sse_ack(text).unwrap();
    assert_eq!(
      ack,
      SseAck {
        conversation_id: "c1".into(),
        section_id: "s1".into(),
        question_id: "q1".into(),
      }
    );
  }

  #[test]
  fn sse_error_classified() {
    let verify = "event: STREAM_ERROR\ndata: {\"error_msg\":\"rate limited\",\"extra\":{\"decision\":\"{\\\"type\\\":\\\"Verify\\\",\\\"detail\\\":\\\"need captcha\\\"}\"}}\n\n";
    assert_eq!(
      parse_sse_ack(verify).unwrap_err(),
      DoubaoProtocolError::VerifyRequired("need captcha".into())
    );
    let quota = "event: STREAM_ERROR\ndata: {\"error_msg\":\"今日额度已用完\"}\n\n";
    assert_eq!(
      parse_sse_ack(quota).unwrap_err(),
      DoubaoProtocolError::RateLimited("今日额度已用完".into())
    );
    let frequent = "event: STREAM_ERROR\ndata: {\"error_msg\":\"操作太频繁了\"}\n\n";
    assert_eq!(
      parse_sse_ack(frequent).unwrap_err(),
      DoubaoProtocolError::TooManyRequests("操作太频繁了".into())
    );
    let generic = "event: STREAM_ERROR\ndata: {\"error_msg\":\"boom\"}\n\n";
    assert_eq!(
      parse_sse_ack(generic).unwrap_err(),
      DoubaoProtocolError::Message("boom".into())
    );
    assert_eq!(
      parse_sse_ack("event: other\ndata: 1\n\n").unwrap_err(),
      DoubaoProtocolError::MissingAck
    );
  }

  #[test]
  fn creation_result_and_confirmation() {
    let response = serde_json::json!({
      "downlink_body": {
        "pull_singe_chain_downlink_body": {
          "messages": [{
            "content_block": [{
              "content": {
                "creation_block": {
                  "creations": [{
                    "id": 12345,
                    "video": {
                      "status": 3,
                      "vid": "v-1",
                      "download_url": "https://dl/x.mp4",
                      "cover": { "image_thumb": { "url": "https://dl/c.jpg" } },
                    },
                  }],
                },
              },
            }],
          }],
        },
      },
    });
    let result = parse_creation_result(&response).unwrap();
    assert_eq!(
      result,
      CreationResult {
        remote_task_id: "12345".into(),
        vid: "v-1".into(),
        fallback_result_url: "https://dl/x.mp4".into(),
        cover_url: "https://dl/c.jpg".into(),
      }
    );

    let confirm = serde_json::json!({
      "downlink_body": {
        "pull_singe_chain_downlink_body": {
          "messages": [{
            "content": "[{\"content\":{\"text_block\":{\"text\":\"请确认参数，确认后我再开始生成\"}}}]",
          }],
        },
      },
    });
    assert!(is_awaiting_confirmation(&confirm));
    assert!(!is_awaiting_confirmation(&serde_json::json!({})));
  }

  #[test]
  fn drive_parsers() {
    let dir = serde_json::json!({
      "code": 0,
      "data": { "children": [
        { "name": "我的创作", "id": 999 },
        { "key": "v-1", "id": 111, "name": "a.mp4" },
        { "name": "subdir" },
      ]},
    });
    assert_eq!(find_creation_directory(&dir).as_deref(), Some("999"));
    assert_eq!(find_video_node(&dir, "v-1").as_deref(), Some("111"));
    assert_eq!(
      parse_creation_nodes(&dir),
      vec![CreationNode {
        vid: "v-1".into(),
        node_id: "111".into(),
        name: "a.mp4".into(),
      }]
    );

    let dl = serde_json::json!({
      "code": 0,
      "data": { "download_infos": [{ "main_url": "https://m", "backup_url": "https://b" }] },
    });
    assert_eq!(
      parse_download_info(&dl),
      Some(DownloadInfo {
        result_url: "https://m".into(),
        backup_result_url: "https://b".into(),
      })
    );
    assert_eq!(parse_download_info(&serde_json::json!({"code": 1})), None);
  }
}
