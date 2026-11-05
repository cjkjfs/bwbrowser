//! Doubao video task executor: drives a logged-in Doubao tab of a running BW
//! profile over CDP (port of DoubaoManager `src/doupool/video/browser.py`).
//!
//! Submission prefers the real UI (type + Enter), because a forged fetch
//! without the frontend signature is flagged by Doubao's verify risk control.
//! The fetch-protocol path stays as a fallback for the one case the UI cannot
//! cover: multiple reference images on a file input that rejects `multiple`.
//! Result polling and the watermark-free download resolution are injected
//! fetches of Doubao's own chain/aispace endpoints.

use base64::Engine as _;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::cdp_target::CdpTarget;
use crate::profile::types::BrowserProfile;
use crate::wayfern_cdp::WayfernSession;

pub const DOUBAO_CHAT_URL: &str = "https://www.doubao.com/chat/";

const CHAT_URL_PREFIX: &str = "https://www.doubao.com";

#[derive(Debug, Clone, PartialEq)]
pub enum DoubaoExecutorError {
  /// The page shows the login button: server-side session expired.
  SessionExpired,
  /// Verify-captcha retries were exhausted on the protocol path.
  VerifyFailed,
  /// Doubao refused the request because the account hit its quota.
  RateLimited(String),
  /// Doubao is throttling the account (too many requests).
  TooManyRequests(String),
  Cancelled,
  Timeout,
  /// The browser connection is unusable; clicking another page control won't repair it.
  Browser(String),
  Message(String),
}

impl std::fmt::Display for DoubaoExecutorError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::SessionExpired => write!(f, "豆包登录态已失效（服务端会话过期），需重新登录"),
      Self::VerifyFailed => write!(f, "豆包验证失败，请稍后重试或更换账号"),
      Self::RateLimited(m) => write!(f, "{m}"),
      Self::TooManyRequests(m) => write!(f, "{m}"),
      Self::Cancelled => write!(f, "任务已取消"),
      Self::Timeout => write!(f, "视频生成超时"),
      Self::Browser(m) | Self::Message(m) => write!(f, "{m}"),
    }
  }
}

impl std::error::Error for DoubaoExecutorError {}

/// One video-generation job the executor runs end to end.
#[derive(Debug, Clone)]
pub struct DoubaoRunParams {
  pub prompt: String,
  pub model: String,
  pub ratio: String,
  pub duration: u32,
  /// "t2v" or "i2v".
  pub mode: String,
  pub image_paths: Vec<PathBuf>,
  /// Overall budget in seconds; 0 = max(420, duration * 60).
  pub timeout_secs: u64,
  pub poll_interval_secs: u64,
}

/// Final artifacts of one successful run.
#[derive(Debug, Clone, Default)]
pub struct DoubaoRunResult {
  pub conversation_id: String,
  pub section_id: String,
  pub question_id: String,
  pub remote_task_id: String,
  pub vid: String,
  pub result_url: String,
  pub backup_result_url: String,
  pub fallback_result_url: String,
  pub cover_url: String,
}

/// Partial progress update pushed to the caller while the task runs.
#[derive(Debug, Clone, Default)]
pub struct DoubaoProgress {
  pub status: Option<String>,
  pub error_message: Option<String>,
  pub conversation_id: Option<String>,
  pub section_id: Option<String>,
  pub question_id: Option<String>,
}

fn cancel_requested(cancel: &AtomicBool) -> bool {
  cancel.load(Ordering::Relaxed)
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), DoubaoExecutorError> {
  if cancel_requested(cancel) {
    return Err(DoubaoExecutorError::Cancelled);
  }
  Ok(())
}

// ============================================================
// Injected page scripts (ported verbatim from browser.py)
// ============================================================

const LOGIN_CHECK_JS: &str = r#"(() => {
  for (const el of document.querySelectorAll('button, a, [role=button]')) {
    if ((el.textContent || '').trim() === '登录') return true;
  }
  return false;
})()"#;

const FINGERPRINT_JS: &str = r#"(() => {
  const match = document.cookie.match(/(?:^|;\s*)s_v_web_id=([^;]+)/);
  return match ? decodeURIComponent(match[1]) : '';
})()"#;

const CAPTCHA_VISIBLE_JS: &str = r#"(() => {
  for (const f of document.querySelectorAll('iframe')) {
    const src = f.src || '';
    if (/verifycenter|rmc\.bytedance\.com|captcha/.test(src)) {
      const r = f.getBoundingClientRect();
      if (r.width > 10 && r.height > 10 && r.bottom > 0) return true;
    }
  }
  return false;
})()"#;

const CLOSE_PROMO_JS: &str = r#"(() => {
  const dlg = document.querySelector('[role=dialog][data-state=open]');
  if (dlg) {
    const b = dlg.querySelector('button[aria-label*=lose], [data-slot=dialog-close]');
    if (b) b.click();
  }
})()"#;

/// 豆包工作版把视频能力收进了「技能」：工具栏的 `[data-testid
/// =general-agent-skill-button]`（文案「更多技能」）打开一个可滚动的技能抽屉，
/// 按分类列出全部技能，选「创意视频」后输入框内出现一个 mention 标记；抽屉里没有
/// 搜索框（输入只会落进主输入框），也没有 Radix 弹层。旧版豆包是工具栏直接有
/// 「视频生成」（或收在「更多」里）。两套标签都试，新版优先。
const VIDEO_SKILL_LABELS: [&str; 2] = ["创意视频", "视频生成"];
const SKILL_BUTTON_SELECTOR: &str = "[data-testid=\"general-agent-skill-button\"]";
const UPLOAD_BUTTON_SELECTOR: &str = "[data-testid=\"upload_file_button\"]";
/// 「+」打开的「添加」菜单里，真正挂出文件框的那一条。文案匹配是整段相等
/// （见 `collect_leaf_labels`），新版豆包写「上传文件或图片」，少一个字都点不到。
const UPLOAD_ENTRY_LABELS: [&str; 6] = [
  "上传文件或图片",
  "上传文件",
  "上传图片",
  "本地文件",
  "从本地上传",
  "照片和视频",
];
/// 豆包前端把附件挂上就自己发上传请求，这条请求是「图真的进输入框了」的信号。
const UPLOAD_MARKERS: [&str; 4] = [
  "prepare_upload",
  "ApplyImageUpload",
  "CommitImageUpload",
  "pre_handle_v2",
];
/// 额度用尽时豆包写在页面上的原话（「近 7 天免费额度已用完，预计 11 月 9 日 10:31
/// 恢复。开通订阅畅享更多额度。」）。只认「额度…用完」这类结论：单独的
/// 「开通豆包订阅」是常驻促销，会误判。
const QUOTA_MARKERS: [&str; 4] = ["额度已用完", "额度已用尽", "额度已耗尽", "免费额度已使用"];
/// Radix 弹层挂在 body 上的 wrapper，弹出层里的条目只有在这里出现才可信
/// （空任务页的「为你推荐」也写着「创意视频」，点它会连带塞进示例文案）。
const POPUP_SCOPE_SELECTOR: &str =
  "[data-radix-popper-content-wrapper], [role=menu], [role=listbox]";

/// Point of the last leaf inside an on-screen popup whose text is one of
/// `labels`; `{}` when the popup has not mounted or holds no such entry.
const FIND_POPUP_LEAF_JS: &str = r#"async ({labels}) => {
  for (const scope of document.querySelectorAll('[data-radix-popper-content-wrapper], [role=menu], [role=listbox]')) {
    const sr = scope.getBoundingClientRect();
    if (sr.width <= 0 || sr.height <= 0) continue;
    let found = {};
    for (const el of scope.querySelectorAll('*')) {
      const r = el.getBoundingClientRect();
      if (el.children.length === 0 && labels.includes((el.textContent || '').trim()) && r.width > 0) {
        found = { label: (el.textContent || '').trim(), x: r.x + r.width / 2, y: r.y + r.height / 2 };
      }
    }
    if (found.label) return found;
  }
  return {};
})()"#;

/// A skill is active when the composer carries a mention chip naming it.
const SKILL_ACTIVE_JS: &str = r#"async ({labels}) => {
  const box = document.querySelector('[contenteditable="true"]');
  if (!box) return false;
  for (const el of box.querySelectorAll('[class*=mention]')) {
    const text = (el.textContent || '').trim();
    if (labels.some((label) => text.includes(label))) return true;
  }
  return false;
})()"#;

/// Any leaf on the page whose text is one of `labels` and sits below `minY`,
/// last match first (Doubao puts the live control at the bottom).
const FIND_LEAVES_JS: &str = r#"async ({labels, minY}) => {
  const hits = [];
  for (const el of document.querySelectorAll('*')) {
    const text = (el.textContent || '').trim();
    if (el.children.length !== 0 || !labels.includes(text)) continue;
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.y < minY) continue;
    hits.unshift({ label: text, x: r.x + r.width / 2, y: r.y + r.height / 2 });
  }
  return hits;
})()"#;

const VIDEO_MODE_READY_JS: &str = r#"(() => {
  const el = document.querySelector('input[type=file]');
  return !!(el && (el.accept || '').includes('.apng'));
})()"#;

const INPUT_BOX_POINT_JS: &str = r#"(() => {
  const el = document.querySelector('[contenteditable="true"]');
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
})()"#;

const INPUT_BOX_TEXT_JS: &str = r#"(() => {
  const el = document.querySelector('[contenteditable="true"]');
  return el ? el.textContent.trim() : '';
})()"#;

/// One fetch of `/chat/completion` inside the page: returns {status, text}.
const COMPLETION_SCRIPT: &str = r#"async ({payload}) => {
  const read = key => { try { return JSON.parse(localStorage.getItem(key) || '{}') } catch { return {} } };
  const device = read('samantha_web_web_id');
  const tea = read('__tea_cache_tokens_497858');
  const query = new URLSearchParams({
    aid:'497858', device_id:device.web_id || '', device_platform:'web',
    doubao_device_platform:'web', doubao_pc_version:'3.27.4', fp:payload.ext.fp || '',
    language:'zh', pc_version:'3.27.4', pkg_type:'release_version', real_aid:'497858',
    region:'CN', samantha_web:'1', sys_region:'CN', tea_uuid:tea.user_unique_id || tea.web_id || '',
    version_code:'20800', web_id:tea.web_id || tea.user_unique_id || '', web_platform:'browser', web_tab_id:crypto.randomUUID()
  });
  query.set('use-olympus-account', '1');
  const hex = count => Array.from(crypto.getRandomValues(new Uint8Array(count)), value => value.toString(16).padStart(2, '0')).join('');
  const response = await fetch('/chat/completion?' + query, {
    method:'POST', credentials:'include',
    headers:{
      'content-type':'application/json', 'agw-js-conv':'str, str', 'last-event-id':'undefined',
      'x-flow-trace':`04-${hex(16)}-${hex(8)}-01`
    },
    body:JSON.stringify(payload)
  });
  return {status:response.status, text:await response.text()};
}"#;

/// One fetch of the conversation history endpoint: returns {status, data}.
const CHAIN_SCRIPT: &str = r#"async ({conversationId}) => {
  const read = key => { try { return JSON.parse(localStorage.getItem(key) || '{}') } catch { return {} } };
  const device = read('samantha_web_web_id');
  const tea = read('__tea_cache_tokens_497858');
  const fp = decodeURIComponent((document.cookie.match(/(?:^|;\s*)s_v_web_id=([^;]+)/) || [,''])[1]);
  const query = new URLSearchParams({
    version_code:'20800', language:'zh', device_platform:'web', doubao_device_platform:'web',
    aid:'497858', real_aid:'497858', pkg_type:'release_version', device_id:device.web_id || '',
    pc_version:'3.27.4', doubao_pc_version:'3.27.4', web_id:tea.web_id || tea.user_unique_id || '', fp,
    tea_uuid:tea.user_unique_id || tea.web_id || '', region:'CN', sys_region:'CN',
    samantha_web:'1', web_platform:'browser', web_tab_id:crypto.randomUUID()
  });
  query.set('use-olympus-account', '1');
  const body = {
    cmd:3100,
    uplink_body:{pull_singe_chain_uplink_body:{
      conversation_id:conversationId, anchor_index:9007199254740991, conversation_type:3,
      direction:1, limit:20, ext:{}, filter:{index_list:[]},
      evaluate_ab_params:'', evaluate_common_params:''
    }},
    sequence_id:crypto.randomUUID(), channel:2, version:'1'
  };
  const response = await fetch('/im/chain/single?' + query, {
    method:'POST', credentials:'include',
    headers:{'content-type':'application/json; encoding=utf-8'}, body:JSON.stringify(body)
  });
  return {status:response.status, data:await response.json()};
}"#;

/// One fetch of an aispace endpoint: returns {status, data}.
const AISPACE_SCRIPT: &str = r#"async ({endpoint, body}) => {
  const read = key => { try { return JSON.parse(localStorage.getItem(key) || '{}') } catch { return {} } };
  const device = read('samantha_web_web_id');
  const tea = read('__tea_cache_tokens_497858');
  const fp = decodeURIComponent((document.cookie.match(/(?:^|;\s*)s_v_web_id=([^;]+)/) || [,''])[1]);
  const query = new URLSearchParams({
    version_code:'20800', language:'zh', device_platform:'web', doubao_device_platform:'web',
    aid:'497858', real_aid:'497858', pkg_type:'release_version', device_id:device.web_id || '',
    pc_version:'3.27.4', doubao_pc_version:'3.27.4', web_id:tea.web_id || tea.user_unique_id || '', fp,
    tea_uuid:tea.user_unique_id || tea.web_id || '', region:'CN', sys_region:'CN',
    samantha_web:'1', web_platform:'browser', web_tab_id:crypto.randomUUID()
  });
  query.set('use-olympus-account', '1');
  const response = await fetch(endpoint + '?' + query, {
    method:'POST', credentials:'include', headers:{'content-type':'application/json'},
    body:JSON.stringify(body)
  });
  return {status:response.status, data:await response.json()};
}"#;

/// Protocol image upload (prepare/apply/commit/pre_handle with SigV4 signing
/// done in-page): returns the DoubaoImageRef fields for the completion payload.
const UPLOAD_IMAGE_SCRIPT: &str = r#"async ({name, mime, base64Data}) => {
  const read = key => { try { return JSON.parse(localStorage.getItem(key) || '{}') } catch { return {} } };
  const device = read('samantha_web_web_id');
  const tea = read('__tea_cache_tokens_497858');
  const fp = decodeURIComponent((document.cookie.match(/(?:^|;\s*)s_v_web_id=([^;]+)/) || [,''])[1]);
  const common = () => {
    const query = new URLSearchParams({
      version_code:'20800', language:'zh', device_platform:'web', doubao_device_platform:'web',
      aid:'497858', real_aid:'497858', pkg_type:'release_version', device_id:device.web_id || '',
      pc_version:'3.27.4', doubao_pc_version:'3.27.4', web_id:tea.web_id || tea.user_unique_id || '',
      tea_uuid:tea.user_unique_id || tea.web_id || '', region:'CN', sys_region:'CN',
      samantha_web:'1', web_platform:'browser', web_tab_id:crypto.randomUUID()
    });
    query.set('use-olympus-account', '1');
    if (fp) query.set('fp', fp);
    return query;
  };
  const bytes = Uint8Array.from(atob(base64Data), c => c.charCodeAt(0));
  const extension = (name && name.includes('.')) ? ('.' + name.split('.').pop()) : (mime === 'image/jpeg' ? '.jpg' : '.png');

  const EMPTY_SHA = 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855';
  const enc = new TextEncoder();
  const toHex = buf => [...new Uint8Array(buf)].map(b => b.toString(16).padStart(2, '0')).join('');
  const sha256Hex = async data => toHex(await crypto.subtle.digest('SHA-256', typeof data === 'string' ? enc.encode(data) : data));
  const hmacBytes = async (keyData, msg) => {
    const key = await crypto.subtle.importKey('raw', keyData, {name:'HMAC', hash:'SHA-256'}, false, ['sign']);
    return new Uint8Array(await crypto.subtle.sign('HMAC', key, enc.encode(msg)));
  };
  const aws4 = async (auth, method, params, body) => {
    const amzDate = new Date().toISOString().replace(/[-:]/g, '').replace(/\.\d+/, '');
    const dateStamp = amzDate.slice(0, 8);
    const sortedKeys = Object.keys(params).sort();
    const canonicalQuery = sortedKeys.map(k => encodeURIComponent(k) + '=' + encodeURIComponent(params[k])).join('&');
    const bodyHash = body === null ? EMPTY_SHA : await sha256Hex(body);
    const headerMap = {'x-amz-date': amzDate, 'x-amz-security-token': auth.sessionToken};
    if (body !== null) headerMap['x-amz-content-sha256'] = bodyHash;
    const hKeys = Object.keys(headerMap).sort();
    const canonicalHeaders = hKeys.map(k => k + ':' + headerMap[k] + '\n').join('');
    const canonicalRequest = [method, '/top/v1', canonicalQuery, canonicalHeaders, hKeys.join(';'), bodyHash].join('\n');
    const scope = dateStamp + '/cn-north-1/imagex/aws4_request';
    const stringToSign = ['AWS4-HMAC-SHA256', amzDate, scope, await sha256Hex(canonicalRequest)].join('\n');
    let k = enc.encode('AWS4' + auth.secretKey);
    for (const part of [dateStamp, 'cn-north-1', 'imagex', 'aws4_request']) k = await hmacBytes(k, part);
    const signature = toHex(await hmacBytes(k, stringToSign));
    return {
      authorization: `AWS4-HMAC-SHA256 Credential=${auth.accessKey}/${scope}, SignedHeaders=${hKeys.join(';')}, Signature=${signature}`,
      amzDate, sessionToken: auth.sessionToken, bodyHash,
    };
  };
  const canonicalize = params => Object.keys(params).sort().map(k => encodeURIComponent(k) + '=' + encodeURIComponent(params[k])).join('&');

  const crcTable = (() => {
    const table = new Uint32Array(256);
    for (let i = 0; i < 256; i++) {
      let c = i;
      for (let k = 0; k < 8; k++) c = (c & 1) ? (0xEDB88320 ^ (c >>> 1)) : (c >>> 1);
      table[i] = c >>> 0;
    }
    return table;
  })();
  let crc = 0xFFFFFFFF;
  for (let i = 0; i < bytes.length; i++) crc = crcTable[(crc ^ bytes[i]) & 0xFF] ^ (crc >>> 8);
  const crcHex = ((crc ^ 0xFFFFFFFF) >>> 0).toString(16).padStart(8, '0');

  const prepareQuery = common();
  prepareQuery.set('msToken', '');
  const prepareResp = await fetch('/alice/resource/prepare_upload?' + prepareQuery, {
    method:'POST', credentials:'include', headers:{'content-type':'application/json'},
    body: JSON.stringify({tenant_id:'5', scene_id:'5', resource_type:2})
  });
  const prepareJson = await prepareResp.json();
  if (prepareResp.status !== 200 || prepareJson.code !== 0) {
    throw new Error('prepare_upload failed: ' + (prepareJson.msg || prepareResp.status));
  }
  const serviceId = prepareJson.data.service_id;
  const token = prepareJson.data.upload_auth_token || {};
  const sts = {accessKey: token.access_key, secretKey: token.secret_key, sessionToken: token.session_token};
  if (!sts.accessKey || !sts.secretKey || !sts.sessionToken) {
    throw new Error('prepare_upload missing upload_auth_token');
  }

  const applyParams = {
    Action:'ApplyImageUpload', Version:'2018-08-01', ServiceId:serviceId,
    NeedFallback:'true', FileSize:String(bytes.length), FileExtension:extension,
    s: Math.random().toString(36).slice(2, 12)
  };
  const applySigned = await aws4(sts, 'GET', applyParams, null);
  const applyResp = await fetch('/top/v1?' + canonicalize(applyParams), {
    method:'GET', credentials:'include',
    headers:{'Authorization': applySigned.authorization, 'X-Amz-Date': applySigned.amzDate, 'X-Amz-Security-Token': applySigned.sessionToken}
  });
  const applyText = await applyResp.text();
  let applyJson = {};
  try { applyJson = JSON.parse(applyText); } catch {}
  if (applyResp.status !== 200 || !applyJson.Result) {
    throw new Error('ApplyImageUpload failed: HTTP ' + applyResp.status + ' ' + applyText.slice(0, 400));
  }
  const uploadAddress = applyJson.Result.UploadAddress || {};
  const store = (uploadAddress.StoreInfos || [])[0];
  if (!store) throw new Error('ApplyImageUpload missing StoreInfos');
  const uploadHost = (uploadAddress.UploadHosts || [])[0]
    || ((applyJson.Result.InnerUploadAddress || {}).UploadNodes || [])[0]?.UploadHost;
  if (!uploadHost) throw new Error('ApplyImageUpload missing UploadHost');
  const sessionKey = uploadAddress.SessionKey;
  const storeUri = store.StoreUri;
  const auth = store.Auth;

  const uploadUrl = `https://${uploadHost}/upload/v1/${storeUri}`;
  let uploaded = false;
  let lastError = '';
  for (const url of [
    uploadUrl,
    `https://${uploadHost}/${storeUri}`,
  ]) {
    try {
      const uploadResp = await fetch(url, {
        method:'POST',
        headers:{
          'Authorization': auth,
          'Content-CRC32': crcHex,
          'Content-Type': mime || 'application/octet-stream',
        },
        body: bytes
      });
      if (uploadResp.ok || uploadResp.status === 200) {
        uploaded = true;
        break;
      }
      const putResp = await fetch(url, {
        method:'PUT',
        headers:{
          'Authorization': auth,
          'Content-CRC32': crcHex,
          'Content-Type': mime || 'application/octet-stream',
        },
        body: bytes
      });
      if (putResp.ok || putResp.status === 200) {
        uploaded = true;
        break;
      }
      lastError = `upload HTTP ${uploadResp.status}/${putResp.status} @ ${url}`;
    } catch (err) {
      lastError = String(err);
    }
  }
  if (!uploaded) throw new Error('image binary upload failed: ' + lastError);

  const commitParams = {Action:'CommitImageUpload', Version:'2018-08-01', ServiceId:serviceId};
  const commitBody = JSON.stringify({SessionKey: sessionKey});
  const commitSigned = await aws4(sts, 'POST', commitParams, commitBody);
  const commitResp = await fetch('/top/v1?' + canonicalize(commitParams), {
    method:'POST', credentials:'include',
    headers:{
      'Authorization': commitSigned.authorization,
      'X-Amz-Date': commitSigned.amzDate,
      'X-Amz-Security-Token': commitSigned.sessionToken,
      'X-Amz-Content-Sha256': commitSigned.bodyHash,
      'content-type': 'application/json',
    },
    body: commitBody
  });
  const commitText = await commitResp.text();
  let commitJson = {};
  try { commitJson = JSON.parse(commitText); } catch {}
  if (commitResp.status !== 200 || !commitJson.Result) {
    throw new Error('CommitImageUpload failed: HTTP ' + commitResp.status + ' ' + commitText.slice(0, 400));
  }
  const plugin = ((commitJson.Result.PluginResult || [])[0]) || {};
  const uri = (commitJson.Result.Results || [])[0]?.Uri || plugin.ImageUri || storeUri;

  const identifier = (crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`);
  const localMessageId = crypto.randomUUID ? crypto.randomUUID() : identifier;
  const preQuery = common();
  const preResp = await fetch('/alice/message/pre_handle_v2_without_conv?' + preQuery, {
    method:'POST', credentials:'include', headers:{'content-type':'application/json'},
    body: JSON.stringify({
      uplink_entity:{
        entity_type:2,
        entity_content:{image:{key: uri}},
        identifier
      },
      bot_id:'7338286299411103781',
      local_message_id: localMessageId
    })
  });
  const preJson = await preResp.json();
  if (preResp.status !== 200 || preJson.code !== 0) {
    throw new Error('pre_handle failed: ' + (preJson.msg || preResp.status));
  }

  return {
    identifier,
    uri,
    name: name || plugin.FileName || ('image' + extension),
    width: plugin.ImageWidth || null,
    height: plugin.ImageHeight || null,
    url: '',
    pre_generate_id: (preJson.data && preJson.data.pre_generate_id) || ''
  };
}"#;

// ============================================================
// CDP plumbing
// ============================================================

/// Evaluate an expression and return its by-value result.
pub(crate) async fn evaluate(
  session: &mut WayfernSession,
  expression: &str,
  await_promise: bool,
) -> Result<Value, String> {
  let reply = session
    .call(
      "Runtime.evaluate",
      json!({
        "expression": expression,
        "returnByValue": true,
        "awaitPromise": await_promise,
      }),
    )
    .await
    .map_err(|e| format!("页面通信失败: {e}"))?;
  if let Some(exception) = reply.get("exceptionDetails") {
    let detail = exception
      .pointer("/exception/description")
      .or_else(|| exception.get("text"))
      .and_then(Value::as_str)
      .unwrap_or("unknown page error");
    return Err(format!("页面脚本执行失败: {detail}"));
  }
  Ok(reply.get("result").cloned().unwrap_or_else(|| json!({})))
}

/// Invoke an injected `async ({...}) => {...}` script with a JSON argument.
async fn call_script(
  session: &mut WayfernSession,
  script: &str,
  args: &Value,
) -> Result<Value, String> {
  let expression = format!("({script})({args})");
  evaluate(session, &expression, true).await
}

/// Evaluate a script and require the {status, ...} envelope with status 200.
async fn call_fetch_script(
  session: &mut WayfernSession,
  script: &str,
  args: &Value,
  what: &str,
) -> Result<Value, String> {
  let reply = call_script(session, script, args).await?;
  let status = reply.get("status").and_then(Value::as_u64).unwrap_or(0);
  if status != 200 {
    return Err(format!("{what} 接口返回 HTTP {status}"));
  }
  Ok(reply)
}

/// The Wayfern kernel's automation gate refuses `Runtime.evaluate` on free
/// plans (same gate login_autofill falls back around). DOM / Input / Network /
/// Page / Emulation domains stay open, so every scripted step below has a
/// domain-level fallback.
fn is_gate_error(err: &str) -> bool {
  err.contains("paid")
    || err.contains("requires")
    || err.contains("wasn't found")
    || err.contains("-32000")
}

/// One-shot probe: is this session's Runtime.evaluate gated?
async fn runtime_gated(session: &mut WayfernSession) -> bool {
  matches!(evaluate(session, "1", false).await, Err(e) if is_gate_error(&e))
}

async fn dom_doc_root(session: &mut WayfernSession) -> Result<i64, String> {
  let doc = session
    .call("DOM.getDocument", json!({ "depth": 0 }))
    .await
    .map_err(|e| format!("DOM.getDocument 失败: {e}"))?;
  doc
    .pointer("/root/nodeId")
    .and_then(Value::as_i64)
    .ok_or_else(|| "DOM.getDocument 未返回根节点".to_string())
}

async fn dom_query(session: &mut WayfernSession, root: i64, selector: &str) -> Option<i64> {
  let reply = session
    .call(
      "DOM.querySelector",
      json!({ "nodeId": root, "selector": selector }),
    )
    .await
    .ok()?;
  reply
    .get("nodeId")
    .and_then(Value::as_i64)
    .filter(|id| *id != 0)
}

/// All `name=value` attribute pairs of a node, flattened like the CDP payload.
async fn dom_node_attributes(session: &mut WayfernSession, node_id: i64) -> Vec<String> {
  let Ok(reply) = session
    .call("DOM.getAttributes", json!({ "nodeId": node_id }))
    .await
  else {
    return Vec::new();
  };
  reply
    .get("attributes")
    .and_then(Value::as_array)
    .map(|attrs| {
      attrs
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
    })
    .unwrap_or_default()
}

fn attribute_value(attributes: &[String], name: &str) -> Option<String> {
  attributes
    .iter()
    .position(|attr| attr == name)
    .and_then(|index| attributes.get(index + 1))
    .cloned()
}

/// Center of a node's content box, as `(x, y)`; None when the node has no box.
async fn dom_box_center(
  session: &mut WayfernSession,
  node_id: i64,
) -> Option<(f64, f64, f64, f64)> {
  let reply = session
    .call("DOM.getBoxModel", json!({ "nodeId": node_id }))
    .await
    .ok()?;
  let quad = reply.pointer("/model/content")?.as_array()?;
  let coords: Vec<f64> = quad.iter().filter_map(Value::as_f64).collect();
  if coords.len() < 8 {
    return None;
  }
  let xs = &coords[0..8];
  let (min_x, max_x, min_y, max_y) = xs.chunks(2).fold(
    (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
    |(min_x, max_x, min_y, max_y), point| {
      (
        min_x.min(point[0]),
        max_x.max(point[0]),
        min_y.min(point[1]),
        max_y.max(point[1]),
      )
    },
  );
  if max_x <= min_x || max_y <= min_y {
    return None;
  }
  Some((
    (min_x + max_x) / 2.0,
    (min_y + max_y) / 2.0,
    max_x - min_x,
    max_y - min_y,
  ))
}

/// Scroll a node into the viewport so its box coordinates are on-screen;
/// clicking or hovering a point that was never rendered does nothing.
async fn dom_scroll_into_view(session: &mut WayfernSession, node_id: i64) {
  let _ = session
    .call("DOM.scrollIntoViewIfNeeded", json!({ "nodeId": node_id }))
    .await;
}

/// Center of the last node matching `selector` that has a box. Doubao appends
/// the newest card at the bottom, so "last" is the one just generated.
async fn dom_last_box_center(
  session: &mut WayfernSession,
  root: i64,
  selector: &str,
) -> Option<(f64, f64)> {
  let reply = session
    .call(
      "DOM.querySelectorAll",
      json!({ "nodeId": root, "selector": selector }),
    )
    .await
    .ok()?;
  let node_ids: Vec<i64> = reply
    .get("nodeIds")
    .and_then(Value::as_array)?
    .iter()
    .filter_map(Value::as_i64)
    .collect();
  for node_id in node_ids.into_iter().rev() {
    dom_scroll_into_view(session, node_id).await;
    if let Some((x, y, _, _)) = dom_box_center(session, node_id).await {
      return Some((x, y));
    }
  }
  None
}

/// Elements whose entire text equals `text`, in document order, from a
/// full-depth snapshot. Mirrors the injected scripts' `findLeaf`:
/// `el.children.length === 0 && el.textContent.trim() === text`.
fn collect_leaf_matches(node: &Value, text: &str, out: &mut Vec<i64>) {
  out.extend(
    collect_leaf_labels(node, &[text])
      .into_iter()
      .map(|(_, node_id)| node_id),
  );
}

/// `(label, nodeId)` of every element leaf inside `node` whose whole text is
/// one of `labels`, in document order.
fn collect_leaf_labels(node: &Value, labels: &[&str]) -> Vec<(String, i64)> {
  fn walk(node: &Value, labels: &[&str], out: &mut Vec<(String, i64)>) {
    let Some(children) = node.get("children").and_then(Value::as_array) else {
      return;
    };
    let element_children = children
      .iter()
      .filter(|child| child.get("nodeType").and_then(Value::as_i64) == Some(1))
      .count();
    if element_children == 0 {
      let content = children
        .iter()
        .filter(|child| child.get("nodeType").and_then(Value::as_i64) == Some(3))
        .filter_map(|child| child.get("nodeValue").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
      let text = content.trim();
      if let Some(label) = labels.iter().find(|label| **label == text) {
        if let Some(id) = node.get("nodeId").and_then(Value::as_i64) {
          out.push((label.to_string(), id));
        }
      }
    }
    for child in children {
      walk(child, labels, out);
    }
  }
  let mut out = Vec::new();
  walk(node, labels, &mut out);
  out
}

/// Node ids matching `selector` under `node_id`, in document order.
async fn dom_query_all(session: &mut WayfernSession, node_id: i64, selector: &str) -> Vec<i64> {
  session
    .call(
      "DOM.querySelectorAll",
      json!({ "nodeId": node_id, "selector": selector }),
    )
    .await
    .ok()
    .and_then(|reply| {
      Some(
        reply
          .get("nodeIds")
          .and_then(Value::as_array)?
          .iter()
          .filter_map(Value::as_i64)
          .filter(|id| *id != 0)
          .collect(),
      )
    })
    .unwrap_or_default()
}

/// Deep subtree of one node, so a popup or the composer can be searched on its
/// own instead of guessing from the whole document.
async fn dom_subtree(session: &mut WayfernSession, node_id: i64) -> Option<Value> {
  let reply = session
    .call(
      "DOM.describeNode",
      json!({ "nodeId": node_id, "depth": -1 }),
    )
    .await
    .ok()?;
  reply.get("node").or_else(|| reply.get("root")).cloned()
}

/// Last candidate that renders (scrolling it in first) — menu entries are
/// appended after the group headers they sit under.
async fn dom_last_leaf_hit(
  session: &mut WayfernSession,
  hits: Vec<(String, i64)>,
) -> Option<(String, f64, f64)> {
  for (label, node_id) in hits.into_iter().rev() {
    dom_scroll_into_view(session, node_id).await;
    if let Some((x, y, _, _)) = dom_box_center(session, node_id).await {
      return Some((label, x, y));
    }
  }
  None
}

/// Lowest-rendered leaf among the candidates: the composer's toolbar lives
/// under the sidebar, so the bottom match is the live control.
async fn dom_lowest_leaf_hit(
  session: &mut WayfernSession,
  hits: Vec<(String, i64)>,
  min_y: f64,
) -> Option<(String, f64, f64)> {
  let mut placed = Vec::new();
  for (label, node_id) in hits {
    if let Some((x, y, _, height)) = dom_box_center(session, node_id).await {
      if y - height / 2.0 > min_y {
        placed.push((label, x, y));
      }
    }
  }
  placed.sort_by(|a, b| b.2.total_cmp(&a.2));
  placed.into_iter().next()
}

/// Click point of the last visible leaf whose text is `text` and whose top
/// sits below `min_y` — the same last-match rule the injected scripts use.
async fn dom_find_leaf_point(
  session: &mut WayfernSession,
  text: &str,
  min_y: f64,
) -> Option<(f64, f64)> {
  let doc = session
    .call("DOM.getDocument", json!({ "depth": -1 }))
    .await
    .ok()?;
  let root = doc.get("root")?;
  let mut candidates = Vec::new();
  collect_leaf_matches(root, text, &mut candidates);
  for node_id in candidates.iter().rev() {
    if let Some((x, y, _, height)) = dom_box_center(session, *node_id).await {
      if y - height / 2.0 > min_y {
        return Some((x, y));
      }
    }
  }
  None
}

/// Gate-mode twin of FIND_LEAVES_JS: `(label, x, y)` of the lowest leaf
/// wearing one of `labels` below `min_y`.
async fn dom_find_leaf(
  session: &mut WayfernSession,
  labels: &[&str],
  min_y: f64,
) -> Option<(String, f64, f64)> {
  let doc = session
    .call("DOM.getDocument", json!({ "depth": -1 }))
    .await
    .ok()?;
  let hits = collect_leaf_labels(doc.get("root")?, labels);
  dom_lowest_leaf_hit(session, hits, min_y).await
}

/// Gate-mode twin of FIND_POPUP_LEAF_JS: only entries that live inside a
/// mounted popup count, so the empty-page 「为你推荐」 cards are never clicked.
async fn dom_popup_leaf(
  session: &mut WayfernSession,
  labels: &[&str],
) -> Option<(String, f64, f64)> {
  let root = dom_doc_root(session).await.ok()?;
  for node_id in dom_query_all(session, root, POPUP_SCOPE_SELECTOR)
    .await
    .into_iter()
    .rev()
  {
    let dom_popper_root = dom_subtree(session, node_id).await?;
    let hits = collect_leaf_labels(&dom_popper_root, labels);
    if hits.is_empty() {
      continue;
    }
    if let Some(hit) = dom_last_leaf_hit(session, hits).await {
      return Some(hit);
    }
  }
  None
}

/// Gate-mode twin of SKILL_ACTIVE_JS: a mention chip naming a skill inside the
/// composer means that skill is attached to the message about to be sent.
async fn dom_skill_active(session: &mut WayfernSession, labels: &[&str]) -> bool {
  let Ok(root) = dom_doc_root(session).await else {
    return false;
  };
  let Some(composer) = dom_query(session, root, "[contenteditable=\"true\"]").await else {
    return false;
  };
  let Some(subtree) = dom_subtree(session, composer).await else {
    return false;
  };
  !collect_leaf_labels(&subtree, labels).is_empty()
}

async fn current_url(session: &mut WayfernSession) -> String {
  match evaluate(session, "location.href", false).await {
    Ok(v) => return v.as_str().unwrap_or_default().to_string(),
    Err(e) if !is_gate_error(&e) => return String::new(),
    Err(_) => {}
  }
  session
    .call("Page.getNavigationHistory", json!({}))
    .await
    .ok()
    .and_then(|reply| {
      let index = reply
        .get("currentIndex")
        .and_then(Value::as_u64)
        .unwrap_or(0);
      reply
        .pointer(&format!("/entries/{index}/url"))
        .and_then(Value::as_str)
        .map(str::to_string)
    })
    .unwrap_or_default()
}

/// The doubao chat tab of this profile: reuse one if present, else open it.
/// The session is left on the chat URL after a short settle wait.
async fn open_chat_session_at(
  profile: &BrowserProfile,
  chat_url: &str,
  cancel: &AtomicBool,
) -> Result<WayfernSession, DoubaoExecutorError> {
  check_cancel(cancel)?;
  let pages = crate::cdp_target::list_local_pages(profile)
    .await
    .map_err(DoubaoExecutorError::Message)?;
  let doubao = pages
    .iter()
    .find(|page| page.url.starts_with(CHAT_URL_PREFIX));
  let ws_url = match doubao {
    Some(page) => page.ws_url.clone(),
    None => {
      let page = crate::cdp_target::open_local_page(profile, chat_url)
        .await
        .map_err(DoubaoExecutorError::Message)?;
      page.ws_url
    }
  };
  let target = CdpTarget::Local { ws_url };
  let mut session = WayfernSession::open(&target)
    .await
    .map_err(|e| DoubaoExecutorError::Message(format!("连接豆包页面失败: {e}")))?;
  // A reused tab may sit on a doubao subpage; a fresh one may still be loading.
  if current_url(&mut session).await != chat_url {
    session
      .call("Page.navigate", json!({ "url": chat_url }))
      .await
      .map_err(|e| DoubaoExecutorError::Message(format!("打开豆包页面失败: {e}")))?;
  }
  let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
  loop {
    check_cancel(cancel)?;
    if current_url(&mut session).await.starts_with(CHAT_URL_PREFIX) {
      break;
    }
    if tokio::time::Instant::now() >= deadline {
      return Err(DoubaoExecutorError::Message("豆包页面打开超时".into()));
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
  }
  tokio::time::sleep(Duration::from_secs(2)).await;
  check_cancel(cancel)?;
  session
    .call("Network.enable", json!({}))
    .await
    .map_err(|e| DoubaoExecutorError::Message(format!("Network.enable 失败: {e}")))?;
  Ok(session)
}

pub(crate) async fn open_chat_session(
  profile: &BrowserProfile,
  cancel: &AtomicBool,
) -> Result<WayfernSession, DoubaoExecutorError> {
  open_chat_session_at(profile, DOUBAO_CHAT_URL, cancel).await
}

/// The sidebar shows a login button when the server-side session expired.
async fn assert_logged_in(
  session: &mut WayfernSession,
  cancel: &AtomicBool,
) -> Result<(), DoubaoExecutorError> {
  check_cancel(cancel)?;
  // 先问服务端：cookie 还在不等于会话还在
  if ssr_session_expired(session).await {
    debug_log("豆包服务端判定会话过期（account_info_error/13），cookie 仍在但已不可用");
    return Err(DoubaoExecutorError::SessionExpired);
  }
  match evaluate(session, LOGIN_CHECK_JS, false).await {
    Ok(value) => {
      if value.as_bool() == Some(true) {
        return Err(DoubaoExecutorError::SessionExpired);
      }
      Ok(())
    }
    // 门禁下退化为会话 Cookie 存在性检查（与 verify_login_state 同一判据）
    Err(e) if is_gate_error(&e) => {
      let cookies = session
        .call("Network.getCookies", json!({}))
        .await
        .map_err(|e| DoubaoExecutorError::Message(e.to_string()))?;
      let names: Vec<&str> = cookies["cookies"]
        .as_array()
        .map(|list| {
          list
            .iter()
            .filter_map(|c| c.get("name").and_then(Value::as_str))
            .collect()
        })
        .unwrap_or_default();
      let has = |name: &str, suffix: &str| {
        names.contains(&name) || names.contains(&(format!("{name}{suffix}").as_str()))
      };
      if has("sessionid", "_ss") && has("passport_auth_status", "_ss") {
        Ok(())
      } else {
        Err(DoubaoExecutorError::SessionExpired)
      }
    }
    // A broken page (offline, still loading) must not fail the task by itself.
    Err(_) => Ok(()),
  }
}

/// `s_v_web_id` cookie value; the fp field of every injected fetch.
async fn read_fingerprint(
  session: &mut WayfernSession,
  cancel: &AtomicBool,
) -> Result<String, DoubaoExecutorError> {
  check_cancel(cancel)?;
  let fp = match evaluate(session, FINGERPRINT_JS, false).await {
    Ok(value) => value.as_str().unwrap_or_default().to_string(),
    Err(e) if is_gate_error(&e) => {
      let cookies = session
        .call("Network.getCookies", json!({}))
        .await
        .map_err(|e| DoubaoExecutorError::Message(e.to_string()))?;
      cookies["cookies"]
        .as_array()
        .and_then(|list| {
          list
            .iter()
            .find(|c| c.get("name").and_then(Value::as_str) == Some("s_v_web_id"))
            .and_then(|c| c.get("value").and_then(Value::as_str))
        })
        .unwrap_or_default()
        .to_string()
    }
    Err(e) => return Err(DoubaoExecutorError::Message(e)),
  };
  if fp.is_empty() {
    return Err(DoubaoExecutorError::Message(
      "豆包浏览器指纹不可用，请重新登录".into(),
    ));
  }
  Ok(fp)
}

async fn captcha_visible(session: &mut WayfernSession) -> bool {
  match evaluate(session, CAPTCHA_VISIBLE_JS, false).await {
    Ok(v) => return v.as_bool().unwrap_or(false),
    Err(e) if !is_gate_error(&e) => return false,
    Err(_) => {}
  }
  let Ok(root) = dom_doc_root(session).await else {
    return false;
  };
  let Ok(reply) = session
    .call(
      "DOM.querySelectorAll",
      json!({ "nodeId": root, "selector": "iframe" }),
    )
    .await
  else {
    return false;
  };
  let node_ids: Vec<i64> = reply
    .get("nodeIds")
    .and_then(Value::as_array)
    .map(|ids| ids.iter().filter_map(Value::as_i64).collect())
    .unwrap_or_default();
  for node_id in node_ids {
    let attributes = dom_node_attributes(session, node_id).await;
    let Some(src) = attribute_value(&attributes, "src") else {
      continue;
    };
    let is_captcha =
      src.contains("verifycenter") || src.contains("rmc.bytedance.com") || src.contains("captcha");
    if !is_captcha {
      continue;
    }
    if let Some((_, _, width, height)) = dom_box_center(session, node_id).await {
      if width > 10.0 && height > 10.0 {
        return true;
      }
    }
  }
  false
}

async fn click_at(session: &mut WayfernSession, x: f64, y: f64) -> Result<(), String> {
  for (kind, button_count) in [("mousePressed", 1i64), ("mouseReleased", 1)] {
    session
      .call(
        "Input.dispatchMouseEvent",
        json!({
          "type": kind,
          "x": x,
          "y": y,
          "button": "left",
          "clickCount": button_count,
        }),
      )
      .await
      .map_err(|e| format!("模拟点击失败: {e}"))?;
  }
  Ok(())
}

/// Rest the pointer over a point without clicking. Doubao only mounts a card's
/// toolbar while the pointer is inside it, so the 「下载」 control does not exist
/// until this has run.
async fn hover_at(session: &mut WayfernSession, x: f64, y: f64) -> Result<(), String> {
  session
    .call(
      "Input.dispatchMouseEvent",
      json!({ "type": "mouseMoved", "x": x, "y": y, "button": "none" }),
    )
    .await
    .map_err(|e| format!("模拟悬停失败: {e}"))
    .map(|_| ())
}

async fn press_enter(session: &mut WayfernSession) -> Result<(), String> {
  session
    .call(
      "Input.dispatchKeyEvent",
      json!({
        "type": "rawKeyDown",
        "key": "Enter",
        "code": "Enter",
        "windowsVirtualKeyCode": 13,
        "nativeVirtualKeyCode": 13,
        "text": "\r",
      }),
    )
    .await
    .map_err(|e| format!("模拟回车失败: {e}"))?;
  session
    .call(
      "Input.dispatchKeyEvent",
      json!({
        "type": "keyUp",
        "key": "Enter",
        "code": "Enter",
        "windowsVirtualKeyCode": 13,
        "nativeVirtualKeyCode": 13,
      }),
    )
    .await
    .map_err(|e| format!("模拟回车失败: {e}"))?;
  Ok(())
}

/// Where the chat input sits, in viewport pixels. Under the paid gate the JS
/// probe is refused, so fall back to the contenteditable's DOM box.
async fn composer_point(session: &mut WayfernSession) -> Option<(f64, f64)> {
  match evaluate(session, INPUT_BOX_POINT_JS, false).await {
    Ok(point) => Some((
      point.get("x").and_then(Value::as_f64)?,
      point.get("y").and_then(Value::as_f64)?,
    )),
    Err(e) if is_gate_error(&e) => {
      let root = dom_doc_root(session).await.ok()?;
      let node = dom_query(session, root, "[contenteditable=true]").await?;
      let (x, y, _, _) = dom_box_center(session, node).await?;
      Some((x, y))
    }
    Err(_) => None,
  }
}

/// Click the chat input, paste a whole block of text, then press Enter.
/// Paste-style insertion is fast and never splits on embedded newlines.
async fn ui_type_text(
  session: &mut WayfernSession,
  text: &str,
  cancel: &AtomicBool,
) -> Result<(), DoubaoExecutorError> {
  check_cancel(cancel)?;
  // 「安全确认」弹窗会盖住输入框，先点掉再定位
  accept_safety_consent(session).await;
  let (x, y) = composer_point(session)
    .await
    .ok_or_else(|| DoubaoExecutorError::Message("找不到豆包输入框".into()))?;
  click_at(session, x, y)
    .await
    .map_err(DoubaoExecutorError::Message)?;
  session
    .call("Input.insertText", json!({ "text": text }))
    .await
    .map_err(|e| DoubaoExecutorError::Message(format!("输入文本失败: {e}")))?;
  tokio::time::sleep(Duration::from_millis(500)).await;
  press_enter(session)
    .await
    .map_err(DoubaoExecutorError::Message)?;
  Ok(())
}

fn load_image_base64(path: &Path) -> Result<(String, String, String), DoubaoExecutorError> {
  let data = std::fs::read(path)
    .map_err(|e| DoubaoExecutorError::Message(format!("图片不存在：{}（{e}）", path.display())))?;
  let mime = match path
    .extension()
    .and_then(|ext| ext.to_str())
    .map(|ext| ext.to_ascii_lowercase())
    .as_deref()
  {
    Some("jpg") | Some("jpeg") => "image/jpeg",
    Some("webp") => "image/webp",
    Some("apng") | Some("png") => "image/png",
    _ => "image/png",
  };
  Ok((
    path
      .file_name()
      .and_then(|n| n.to_str())
      .unwrap_or("image.png")
      .to_string(),
    mime.to_string(),
    base64::engine::general_purpose::STANDARD.encode(data),
  ))
}

/// The doubao SSR payload embeds the server's own verdict on this session:
/// `account_info_error` / error_code 13 means 「会话过期，请重新登录」. Cookie
/// presence alone lies — a logged-out jar keeps a stale `sessionid` — so the
/// page text is the authoritative check, and it needs no Runtime access.
pub(crate) async fn ssr_session_expired(session: &mut WayfernSession) -> bool {
  let Ok(reply) = session
    .call("DOM.getDocument", json!({ "depth": -1, "pierce": false }))
    .await
  else {
    return false;
  };
  let Some(root) = reply.get("root") else {
    return false;
  };
  let mut text = String::new();
  if !collect_script_text(root, &mut text) {
    return false;
  }
  ssr_reports_expired(&text)
}

/// The two spellings of the server's "session expired" verdict in the SSR blob.
fn ssr_reports_expired(script_text: &str) -> bool {
  script_text.contains(r#""error_code":13"#) || script_text.contains("account_info_error")
}

/// Collect the source text of every `<script>` node; returns whether the
/// router data blob (the only one we care about) showed up.
fn collect_script_text(node: &Value, out: &mut String) -> bool {
  if node.get("nodeName").and_then(Value::as_str) == Some("SCRIPT") {
    append_text_nodes(node, out);
  }
  let mut found = out.contains("_ROUTER_DATA");
  if let Some(children) = node.get("children").and_then(Value::as_array) {
    for child in children {
      found |= collect_script_text(child, out);
    }
  }
  found
}

/// Append `nodeValue` of this node's text descendants.
fn append_text_nodes(node: &Value, out: &mut String) {
  if let Some(value) = node.get("nodeValue").and_then(Value::as_str) {
    out.push_str(value);
    out.push('\n');
  }
  if let Some(children) = node.get("children").and_then(Value::as_array) {
    for child in children {
      append_text_nodes(child, out);
    }
  }
}

/// 豆包额度用尽时把结论直接写在页面上：「近 7 天免费额度已用完，预计 11 月 9 日
/// 10:31 恢复。开通订阅畅享更多额度。」取命中那条句子原样带回，账号上挂的就是
/// 豆包自己的话，也带着恢复时间。
fn quota_notice(page_text: &str) -> Option<String> {
  page_sentence_notice(page_text, &QUOTA_MARKERS)
}

/// 页面文案里的「等用户回复确认」提示（「确认生成」这类）。
fn confirmation_cue(page_text: &str) -> Option<String> {
  page_sentence_notice(page_text, &crate::doubao_protocol::CONFIRMATION_MARKERS)
}

/// 按 markers 在页面文案里找第一条命中句。先按整行找（一个文本节点通常就是
/// 一句话），行内没有标点才抹平换行拼回去再找。
fn page_sentence_notice(page_text: &str, markers: &[&str]) -> Option<String> {
  let mut fragment = false;
  for line in page_text.lines() {
    let trimmed = line.trim();
    if !markers.iter().any(|marker| trimmed.contains(marker)) {
      continue;
    }
    // 一个节点里就是整句话（真实页面的常态）：直接用它，省得把上一个节点的
    // 文案（卡片标题之类）粘进提示里
    if trimmed.contains('。') || trimmed.contains('，') {
      return Some(sentence_around(trimmed, markers));
    }
    fragment = true;
  }
  if !fragment {
    return None;
  }
  // 句子被拆成几个节点：抹平换行再找一次
  let flat = page_text
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("");
  let at = markers
    .iter()
    .filter_map(|marker| flat.find(marker))
    .min()?;
  let head = flat[..at].rsplit(['。', '！', '？']).next().unwrap_or("");
  Some(sentence_around(
    &format!("{head}{}", flat[at..].chars().take(96).collect::<String>()),
    markers,
  ))
}

/// 只留命中那一句：往前到上一个句末符，往后到第一个句号。
fn sentence_around(text: &str, markers: &[&str]) -> String {
  let at = markers
    .iter()
    .filter_map(|marker| text.find(marker))
    .min()
    .unwrap_or(0);
  let head = text[..at].rsplit(['。', '！', '？']).next().unwrap_or("");
  let tail: String = text[at..].chars().take(96).collect();
  let tail = match tail.find('。') {
    Some(end) => &tail[..end + '。'.len_utf8()],
    None => &tail,
  };
  format!("{head}{tail}")
}

/// 页面最近一段文案。只看文档末尾，避免把同一会话里早先的消息当成这一次
/// 任务的结论。
async fn page_tail_text(session: &mut WayfernSession) -> Option<String> {
  let reply = session
    .call("DOM.getDocument", json!({ "depth": -1, "pierce": false }))
    .await
    .ok()?;
  let root = reply.get("root")?;
  let mut text = String::new();
  append_text_nodes(root, &mut text);
  let tail: String = text
    .chars()
    .rev()
    .take(4000)
    .collect::<Vec<_>>()
    .into_iter()
    .rev()
    .collect();
  Some(tail)
}

/// 页面文案里的额度结论。
async fn page_quota_notice(session: &mut WayfernSession) -> Option<String> {
  quota_notice(&page_tail_text(session).await?)
}

/// 页面文案里的「等用户回复确认」提示。
async fn page_confirmation_cue(session: &mut WayfernSession) -> Option<String> {
  confirmation_cue(&page_tail_text(session).await?)
}

async fn composer_text(session: &mut WayfernSession) -> Option<String> {
  match evaluate(session, INPUT_BOX_TEXT_JS, false).await {
    Ok(value) => return Some(value.as_str().unwrap_or_default().trim().to_string()),
    Err(error) if !is_gate_error(&error) => return None,
    Err(_) => {}
  }
  let root = dom_doc_root(session).await.ok()?;
  let composer = dom_query(session, root, "[contenteditable=\"true\"]").await?;
  let subtree = dom_subtree(session, composer).await?;
  let mut text = String::new();
  append_text_nodes(&subtree, &mut text);
  Some(text.trim().to_string())
}

async fn composer_is_empty(session: &mut WayfernSession) -> bool {
  composer_text(session)
    .await
    .is_some_and(|text| text.is_empty())
}

/// Sending the keystroke is not a receipt. Only an empty composer proves that
/// the confirmation left the editor; otherwise clear any residue and try again.
async fn ui_send_confirmation(
  session: &mut WayfernSession,
  cancel: &AtomicBool,
) -> Result<bool, DoubaoExecutorError> {
  ui_send_text(session, "开始生成视频", cancel).await
}

/// Sending the keystroke is not a receipt. Only an empty composer proves that
/// the message left the editor; otherwise clear any residue and try again.
async fn ui_send_text(
  session: &mut WayfernSession,
  text: &str,
  cancel: &AtomicBool,
) -> Result<bool, DoubaoExecutorError> {
  for _ in 0..3 {
    clear_composer(session).await;
    ui_type_text(session, text, cancel).await?;
    for _ in 0..10 {
      tokio::time::sleep(Duration::from_millis(500)).await;
      check_cancel(cancel)?;
      if composer_is_empty(session).await {
        return Ok(true);
      }
    }
    debug_log("回复仍在输入框，清理后重试发送");
  }
  Ok(false)
}

/// A new task is accepted when the editor clears or the page creates its chat.
/// If the editor cannot be inspected, wait for the URL instead of risking a
/// duplicate generation by blindly pressing Enter again.
async fn ui_send_task(
  session: &mut WayfernSession,
  text: &str,
  may_clear_composer: bool,
  cancel: &AtomicBool,
  update: &(dyn Fn(DoubaoProgress) + Send + Sync),
) -> Result<String, DoubaoExecutorError> {
  if may_clear_composer {
    clear_composer(session).await;
  }
  let mut can_read_composer = true;
  for attempt in 1..=3 {
    if attempt > 1 {
      // Ctrl+A would remove uploaded images and the video-skill mention too.
      // Keep that message intact and wait for its URL rather than submitting it twice.
      if !may_clear_composer {
        return wait_for_ui_conversation(session, cancel, update).await;
      }
      clear_composer(session).await;
    }
    ui_type_text(session, text, cancel).await?;
    for _ in 0..10 {
      tokio::time::sleep(Duration::from_millis(500)).await;
      check_cancel(cancel)?;
      if let Some(id) = conv_url_conversation_id(&current_url(session).await) {
        return Ok(id);
      }
      match composer_text(session).await {
        Some(remaining) if remaining.is_empty() => {
          return wait_for_ui_conversation(session, cancel, update).await;
        }
        Some(_) => {}
        None => can_read_composer = false,
      }
    }
    if !can_read_composer {
      return wait_for_ui_conversation(session, cancel, update).await;
    }
    if let Some(notice) = page_quota_notice(session).await {
      return Err(DoubaoExecutorError::RateLimited(notice));
    }
    debug_log(&format!("任务提交第 {attempt}/3 次未收到回执，清理后重试"));
    update(DoubaoProgress {
      status: Some("starting".into()),
      error_message: Some(format!("任务尚未提交成功，正在重试（{attempt}/3）...")),
      ..Default::default()
    });
  }
  Err(DoubaoExecutorError::Message(
    "任务连续 3 次未提交成功，请检查浏览器输入框和发送按钮".into(),
  ))
}

/// 排障日志通道：本应用把 `log::` 整体设为 Off，只有 bwbrowser_cloud 的调试日志
/// 会落盘（云端「同步调试日志」开关控制），豆包模块的关键判断走这里才有迹可循。
pub(crate) fn debug_log(message: &str) {
  crate::bwbrowser_cloud::log_bwbrowser("doubao", message);
}

/// True when the subtree carries `needle` anywhere in its text nodes.
fn subtree_contains_text(node: &Value, needle: &str) -> bool {
  if node
    .get("nodeValue")
    .and_then(Value::as_str)
    .map(|value| value.contains(needle))
    .unwrap_or(false)
  {
    return true;
  }
  node
    .get("children")
    .and_then(Value::as_array)
    .map(|children| {
      children
        .iter()
        .any(|child| subtree_contains_text(child, needle))
    })
    .unwrap_or(false)
}

/// Doubao 首次提交会弹「安全确认」素材授权弹窗，挡住输入框和提交：找到它的
/// 「确认」按钮并按下去。DOM + Input 域实现，门禁内外都可用；只认标题带
/// 「安全确认」的弹窗，页面上其他「确认」文本不会被误点。
async fn accept_safety_consent(session: &mut WayfernSession) -> bool {
  let Ok(root_id) = dom_doc_root(session).await else {
    return false;
  };
  let Some(dialog) = dom_query(session, root_id, "[role=dialog][data-state=open]").await else {
    return false;
  };
  let Ok(reply) = session
    .call("DOM.describeNode", json!({ "nodeId": dialog, "depth": -1 }))
    .await
  else {
    return false;
  };
  let Some(node) = reply.get("node") else {
    return false;
  };
  if !subtree_contains_text(node, "安全确认") {
    return false;
  }
  let mut buttons = Vec::new();
  collect_leaf_matches(node, "确认", &mut buttons);
  // 弹窗挂在 body 末尾，取最后一个匹配（与注入脚本的 last-match 规则一致）
  for node_id in buttons.iter().rev() {
    if let Some((x, y, _, _)) = dom_box_center(session, *node_id).await {
      if click_at(session, x, y).await.is_ok() {
        log::info!("[doubao] 已自动点击豆包「安全确认」授权弹窗的确认");
        return true;
      }
    }
  }
  false
}

/// Best-effort close of an open promo dialog through the DOM + Input domains.
async fn dom_close_promo(session: &mut WayfernSession) {
  let Ok(root) = dom_doc_root(session).await else {
    return;
  };
  let Some(dialog) = dom_query(session, root, "[role=dialog][data-state=open]").await else {
    return;
  };
  let Some(close) = dom_query(
    session,
    dialog,
    "button[aria-label*=lose], [data-slot=dialog-close]",
  )
  .await
  else {
    return;
  };
  if let Some((x, y, _, _)) = dom_box_center(session, close).await {
    let _ = click_at(session, x, y).await;
  }
}

/// `(label, x, y)` of the lowest leaf wearing one of `labels` under `min_y`:
/// the page script first, the DOM tree when the automation gate refuses it.
async fn find_leaf(
  session: &mut WayfernSession,
  labels: &[&str],
  min_y: f64,
) -> Option<(String, f64, f64)> {
  let expression = format!(
    "({FIND_LEAVES_JS})({})",
    json!({ "labels": labels, "minY": min_y })
  );
  match evaluate(session, &expression, true).await {
    Ok(v) => v
      .as_array()
      .map(|hits| {
        hits
          .iter()
          .filter_map(|hit| {
            Some((
              hit.get("label")?.as_str()?.to_string(),
              hit.get("x")?.as_f64()?,
              hit.get("y")?.as_f64()?,
            ))
          })
          .next()
      })
      .unwrap_or(None),
    Err(e) if !is_gate_error(&e) => None,
    Err(_) => dom_find_leaf(session, labels, min_y).await,
  }
}

/// `(label, x, y)` of an entry inside the open skill popup.
async fn find_popup_leaf(
  session: &mut WayfernSession,
  labels: &[&str],
) -> Option<(String, f64, f64)> {
  let expression = format!("({FIND_POPUP_LEAF_JS})({})", json!({ "labels": labels }));
  match evaluate(session, &expression, true).await {
    Ok(v) => {
      let obj = v.as_object()?;
      Some((
        obj.get("label")?.as_str()?.to_string(),
        obj.get("x")?.as_f64()?,
        obj.get("y")?.as_f64()?,
      ))
    }
    Err(e) if !is_gate_error(&e) => None,
    Err(_) => dom_popup_leaf(session, labels).await,
  }
}

/// Empty the composer. An accidental click on the empty-task page's 「为你推荐」
/// card drops example copy into the input, and that copy would ride along into
/// the real prompt.
async fn clear_composer(session: &mut WayfernSession) {
  let Ok(root) = dom_doc_root(session).await else {
    return;
  };
  let Some(node) = dom_query(session, root, "[contenteditable=\"true\"]").await else {
    return;
  };
  let Some((x, y, _, _)) = dom_box_center(session, node).await else {
    return;
  };
  if click_at(session, x, y).await.is_err() {
    return;
  }
  for (key, code, vk, modifiers) in [("a", "KeyA", 65, 2), ("Backspace", "Backspace", 8, 0)] {
    for event_type in ["keyDown", "keyUp"] {
      let _ = session
        .call(
          "Input.dispatchKeyEvent",
          json!({
            "type": event_type,
            "key": key,
            "code": code,
            "windowsVirtualKeyCode": vk,
            "nativeVirtualKeyCode": vk,
            "modifiers": modifiers,
          }),
        )
        .await;
    }
  }
  tokio::time::sleep(Duration::from_millis(200)).await;
}

/// Pick a skill row out of the open 技能 drawer. The drawer is a scrolling
/// list, not a Radix menu: every row is in the DOM but most sit clipped below
/// the fold, so the target is pulled into view before the click. The composer
/// chip is the verdict — it also rejects the 「为你推荐」 card that carries the
/// same name on the empty-task page.
async fn pick_skill_row(session: &mut WayfernSession, labels: &[&str]) -> Option<String> {
  for label in labels {
    let mut tried = 0usize;
    loop {
      let Ok(doc) = session
        .call("DOM.getDocument", json!({ "depth": -1, "pierce": true }))
        .await
      else {
        return None;
      };
      let Some(root) = doc.get("root") else {
        break;
      };
      let mut nodes: Vec<i64> = collect_leaf_labels(root, &[*label])
        .into_iter()
        .map(|(_, node_id)| node_id)
        .collect();
      // Doubao paints the live list last, so try the bottom-most match first.
      nodes.reverse();
      let Some(node_id) = nodes.into_iter().nth(tried) else {
        break;
      };
      tried += 1;
      dom_scroll_into_view(session, node_id).await;
      tokio::time::sleep(Duration::from_millis(400)).await;
      let Some((x, y, _, _)) = dom_box_center(session, node_id).await else {
        continue;
      };
      if click_at(session, x, y).await.is_err() {
        continue;
      }
      tokio::time::sleep(Duration::from_millis(800)).await;
      if dom_skill_active(session, &[*label]).await {
        return Some(label.to_string());
      }
      clear_composer(session).await;
    }
  }
  None
}

/// Click the first node matching `selector` that has a box.
async fn click_selector(session: &mut WayfernSession, selector: &str) -> bool {
  let Ok(root) = dom_doc_root(session).await else {
    return false;
  };
  let Some(node) = dom_query(session, root, selector).await else {
    return false;
  };
  dom_scroll_into_view(session, node).await;
  let Some((x, y, _, _)) = dom_box_center(session, node).await else {
    return false;
  };
  click_at(session, x, y).await.is_ok()
}

/// The new composer keeps no apng input: the attached skill shows up as a
/// mention chip inside the contenteditable. Either signal means video mode.
async fn skill_active(session: &mut WayfernSession) -> bool {
  let expression = format!(
    "({SKILL_ACTIVE_JS})({})",
    json!({ "labels": VIDEO_SKILL_LABELS })
  );
  match evaluate(session, &expression, true).await {
    Ok(v) => v.as_bool().unwrap_or(false),
    Err(e) if is_gate_error(&e) => dom_skill_active(session, &VIDEO_SKILL_LABELS).await,
    Err(_) => false,
  }
}

/// The file input accepting .apng, or an attached 创意视频 skill, marks video
/// mode as active.
async fn video_mode_ready(session: &mut WayfernSession) -> bool {
  let apng = match evaluate(session, VIDEO_MODE_READY_JS, false).await {
    Ok(v) => v.as_bool().unwrap_or(false),
    Err(e) if !is_gate_error(&e) => false,
    Err(_) => {
      let ready = async {
        let root = dom_doc_root(session).await.ok()?;
        let node = dom_query(session, root, "input[type=file]").await?;
        let attributes = dom_node_attributes(session, node).await;
        Some(attribute_value(&attributes, "accept")?.contains(".apng"))
      }
      .await
      .unwrap_or(false);
      ready
    }
  };
  apng || skill_active(session).await
}

/// Wait for the mode signal after a click attempt.
async fn wait_video_mode(
  session: &mut WayfernSession,
  cancel: &AtomicBool,
  rounds: u32,
) -> Result<bool, DoubaoExecutorError> {
  for _ in 0..rounds {
    check_cancel(cancel)?;
    tokio::time::sleep(Duration::from_millis(700)).await;
    if video_mode_ready(session).await {
      return Ok(true);
    }
  }
  Ok(false)
}

/// Attach the video skill to the composer. 豆包工作 has no 「视频生成」 button
/// any more: the toolbar carries 「更多技能」 (`general-agent-skill-button`),
/// the popup lists the skills, and picking 「创意视频」 leaves a mention chip in
/// the input. Older pages still expose 「视频生成」 on the toolbar or behind
/// 「更多」, so those run as fallbacks. Every click is followed by the mode
/// signal — a wrong click cannot slip through as "ready".
async fn switch_to_video_mode(
  session: &mut WayfernSession,
  cancel: &AtomicBool,
) -> Result<(), DoubaoExecutorError> {
  accept_safety_consent(session).await;
  if evaluate(session, CLOSE_PROMO_JS, false).await.is_err() {
    dom_close_promo(session).await;
  }
  tokio::time::sleep(Duration::from_millis(500)).await;
  if wait_video_mode(session, cancel, 2).await? {
    return Ok(());
  }

  // 1) 新版：「更多技能」打开技能抽屉，从抽屉里选视频技能。抽屉是可滚动列表，
  //    目标行常被裁在折叠线以下，所以先滚进可视区再点。
  let mut trail = String::new();
  if click_selector(session, SKILL_BUTTON_SELECTOR).await {
    trail.push_str("技能按钮(testid)");
  } else if let Some((_, x, y)) = find_leaf(session, &["更多技能"], 0.0).await {
    let _ = click_at(session, x, y).await;
    trail.push_str("技能按钮(文案)");
  } else {
    trail.push_str("技能按钮(缺失)");
  }
  tokio::time::sleep(Duration::from_millis(1200)).await;
  if let Some(label) = pick_skill_row(session, &VIDEO_SKILL_LABELS).await {
    trail.push_str(&format!("抽屉「{label}」"));
    if wait_video_mode(session, cancel, 8).await? {
      return Ok(());
    }
  }

  // 2) 旧版：工具栏直接有「视频生成」
  if let Some((label, x, y)) = find_leaf(session, &["视频生成"], 0.0).await {
    let _ = click_at(session, x, y).await;
    trail.push_str(&format!("工具栏「{label}」"));
    if wait_video_mode(session, cancel, 6).await? {
      return Ok(());
    }
  }
  // 3) 旧版：收在输入框工具栏的「更多」弹出菜单里
  if let Some((_, x, y)) = find_leaf(session, &["更多"], 400.0).await {
    let _ = click_at(session, x, y).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    if let Some((label, x, y)) = find_popup_leaf(session, &VIDEO_SKILL_LABELS)
      .await
      .or(find_leaf(session, &["视频生成"], 400.0).await)
    {
      let _ = click_at(session, x, y).await;
      trail.push_str(&format!("「更多」→「{label}」"));
      if wait_video_mode(session, cancel, 8).await? {
        return Ok(());
      }
    }
  }

  debug_log(&format!(
    "切换视频生成模式失败：尝试={} 就绪=否 弹层={} 文件框={} 技能标记=否",
    trail,
    find_popup_leaf(session, &VIDEO_SKILL_LABELS)
      .await
      .is_some(),
    video_file_input(session).await.is_some(),
  ));
  Err(DoubaoExecutorError::Message(
    "未能切换到视频生成模式（页面可能是改版后的豆包，或登录态已失效），请重试或更换账号".into(),
  ))
}

/// The composer's file input: the old video mode keeps one with `.apng`, the
/// new one only mounts it after 「上传文件」 is opened.
async fn video_file_input(session: &mut WayfernSession) -> Option<i64> {
  let root = dom_doc_root(session).await.ok()?;
  for selector in [
    "input[type=file][accept*=apng]",
    "input[type=file][accept*=image]",
    "input[type=file]",
  ] {
    if let Some(node) = dom_query(session, root, selector).await {
      return Some(node);
    }
  }
  None
}

/// Make sure an `input[type=file]` exists so `DOM.setFileInputFiles` has a
/// target: the new Doubao only mounts one after 「+」→「上传文件或图片」.
async fn ensure_file_input(session: &mut WayfernSession) -> Option<i64> {
  if let Some(node) = video_file_input(session).await {
    return Some(node);
  }
  // 「+」是开关式的：上一次失败留下的菜单还开着，再点一次就把它关了，
  // 所以先看条目在不在，缺了才点按钮。
  if find_upload_entry(session).await.is_none() {
    let _ = click_selector(session, UPLOAD_BUTTON_SELECTOR).await;
  }
  for _ in 0..12 {
    tokio::time::sleep(Duration::from_millis(400)).await;
    if let Some(node) = video_file_input(session).await {
      return Some(node);
    }
    if let Some((_, x, y)) = find_upload_entry(session).await {
      let _ = click_at(session, x, y).await;
    }
  }
  None
}

/// 菜单里的上传条目。Radix 那层 `[role=menu]` 容器实测拿不到盒子（宽度 0），
/// 所以按弹层找不到就整页按文案再找一次。
async fn find_upload_entry(session: &mut WayfernSession) -> Option<(String, f64, f64)> {
  if let Some(hit) = find_popup_leaf(session, &UPLOAD_ENTRY_LABELS).await {
    return Some(hit);
  }
  find_leaf(session, &UPLOAD_ENTRY_LABELS, 0.0).await
}

/// 解码素材图并写进系统剪贴板。arboard 的句柄不是 Send，整段放到 blocking 线程；
/// 长边超过 4096 先等比缩，免得一张超大图把几百兆 RGBA 塞进剪贴板。
async fn put_image_on_clipboard(path: &Path) -> Result<(), DoubaoExecutorError> {
  let path = path.to_path_buf();
  tokio::task::spawn_blocking(move || -> Result<(), String> {
    let bytes =
      std::fs::read(&path).map_err(|e| format!("图片读取失败：{}（{e}）", path.display()))?;
    // 云端素材的扩展名可能仍是原上传时写错的 .jpeg；按文件头识别，别把 PNG 当 JPEG 解码。
    let decoded = image::load_from_memory(&bytes)
      .map_err(|e| format!("图片解码失败：{}（{e}）", path.display()))?;
    let fitted = if decoded.width().max(decoded.height()) > 4096 {
      decoded.resize(4096, 4096, image::imageops::FilterType::Triangle)
    } else {
      decoded
    };
    let rgba = fitted.to_rgba8();
    let image = arboard::ImageData {
      width: rgba.width() as usize,
      height: rgba.height() as usize,
      bytes: rgba.into_raw().into(),
    };
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("系统剪贴板打不开：{e}"))?;
    clipboard
      .set_image(image)
      .map_err(|e| format!("素材图写入剪贴板失败：{e}"))?;
    Ok(())
  })
  .await
  .map_err(|e| DoubaoExecutorError::Message(format!("剪贴板任务失败: {e}")))?
  .map_err(DoubaoExecutorError::Message)
}

async fn paste_key(
  session: &mut WayfernSession,
  event_type: &str,
  command: bool,
) -> Result<(), String> {
  let mut payload = json!({
    "type": event_type,
    "key": "v",
    "code": "KeyV",
    "windowsVirtualKeyCode": 86,
    "nativeVirtualKeyCode": 86,
    "modifiers": 2,
  });
  if command {
    payload["commands"] = json!(["Paste"]);
  }
  session
    .call("Input.dispatchKeyEvent", payload)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// 让豆包执行一次粘贴。CDP 的合成按键只进渲染器，浏览器级 Ctrl+V 快捷键不一定触发，
/// 所以先按 `commands:["Paste"]` 直接下粘贴命令；内核不认这个参数才退回普通 Ctrl+V。
/// 两条路只走一条，否则会贴两遍。
async fn send_paste(session: &mut WayfernSession) -> Result<(), DoubaoExecutorError> {
  if paste_key(session, "keyDown", true).await.is_ok() {
    let _ = paste_key(session, "keyUp", false).await;
    return Ok(());
  }
  for event_type in ["keyDown", "keyUp"] {
    paste_key(session, event_type, false)
      .await
      .map_err(DoubaoExecutorError::Message)?;
  }
  Ok(())
}

/// 等页面把 `min` 张图传出去：至少看到 `min` 次上传请求，并且请求安静 5 秒才算完。
/// 豆包的前端自己签名上传，所以这条请求就是「图真的进了输入框」的唯一可信信号。
async fn wait_page_uploads(
  session: &mut WayfernSession,
  cancel: &AtomicBool,
  min: u32,
  timeout: Duration,
) -> Result<u32, DoubaoExecutorError> {
  let deadline = tokio::time::Instant::now() + timeout;
  let mut count: u32 = 0;
  let mut last_event = tokio::time::Instant::now();
  let mut failed = String::new();
  while tokio::time::Instant::now() < deadline {
    check_cancel(cancel)?;
    match session
      .await_any_event(&["Network.responseReceived"], Duration::from_secs(1))
      .await
    {
      Ok(Some((_method, params))) => {
        let url = params
          .pointer("/response/url")
          .and_then(Value::as_str)
          .unwrap_or_default();
        if UPLOAD_MARKERS.iter().any(|marker| url.contains(marker)) {
          let status = params
            .pointer("/response/status")
            .and_then(Value::as_u64)
            .unwrap_or(0);
          if status >= 400 {
            failed = format!("HTTP {status}");
          }
          count += 1;
          last_event = tokio::time::Instant::now();
        }
      }
      Ok(None) => {
        if !failed.is_empty() {
          return Err(DoubaoExecutorError::Message(format!(
            "页面图片上传失败：{failed}"
          )));
        }
        if count >= min && last_event.elapsed() > Duration::from_secs(5) {
          return Ok(count);
        }
      }
      Err(e) => {
        return Err(DoubaoExecutorError::Browser(format!(
          "监听上传请求失败: {e}"
        )))
      }
    }
  }
  Err(DoubaoExecutorError::Message(
    "没有等到豆包的图片上传请求".into(),
  ))
}

/// 传输已经断开时继续点上传入口既不会恢复连接，也会把真实错误换成入口错误。
fn can_retry_upload_with_file_input(error: &DoubaoExecutorError) -> bool {
  !matches!(error, DoubaoExecutorError::Browser(_))
}

/// 按人工的做法贴图：素材图进剪贴板 → 点输入框 → 粘贴，一张一张来。
/// 任何一张没等到上传请求就整体失败，交给上层退回文件框链路。
async fn attach_images_via_paste(
  session: &mut WayfernSession,
  paths: &[PathBuf],
  cancel: &AtomicBool,
) -> Result<(), DoubaoExecutorError> {
  for (index, path) in paths.iter().enumerate() {
    check_cancel(cancel)?;
    put_image_on_clipboard(path).await?;
    let (x, y) = composer_point(session)
      .await
      .ok_or_else(|| DoubaoExecutorError::Message("找不到豆包输入框".into()))?;
    click_at(session, x, y)
      .await
      .map_err(DoubaoExecutorError::Message)?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    send_paste(session).await?;
    wait_page_uploads(session, cancel, 1, Duration::from_secs(25))
      .await
      .map_err(|e| {
        DoubaoExecutorError::Message(format!("粘贴第 {} 张素材图没有生效：{e}", index + 1))
      })?;
  }
  Ok(())
}

/// 把本地素材图送进豆包输入框：先粘贴，粘不上再去点「+」挂文件框。
async fn upload_images_via_page(
  session: &mut WayfernSession,
  paths: &[PathBuf],
  cancel: &AtomicBool,
) -> Result<(), DoubaoExecutorError> {
  session
    .call("Network.enable", json!({}))
    .await
    .map_err(|e| DoubaoExecutorError::Message(format!("Network.enable 失败: {e}")))?;
  match attach_images_via_paste(session, paths, cancel).await {
    Ok(()) => return Ok(()),
    Err(e) if !can_retry_upload_with_file_input(&e) => return Err(e),
    Err(e) => debug_log(&format!("粘贴素材图没生效，退回上传入口：{e}")),
  }
  upload_images_via_file_input(session, paths, cancel).await
}

/// 兜底链路：豆包新版只在「+」→「上传文件或图片」之后才挂出 `input[type=file]`。
async fn upload_images_via_file_input(
  session: &mut WayfernSession,
  paths: &[PathBuf],
  cancel: &AtomicBool,
) -> Result<(), DoubaoExecutorError> {
  let node_id = ensure_file_input(session)
    .await
    .ok_or_else(|| DoubaoExecutorError::Message("找不到图片上传入口，请在大窗口重试".into()))?;

  let files: Vec<String> = paths
    .iter()
    .map(|p| p.to_string_lossy().to_string())
    .collect();
  session
    .call(
      "DOM.setFileInputFiles",
      json!({ "files": files, "nodeId": node_id }),
    )
    .await
    .map_err(|e| DoubaoExecutorError::Message(format!("设置上传文件失败: {e}")))?;

  wait_page_uploads(session, cancel, 1, Duration::from_secs(180))
    .await
    .map(|_| ())
}

/// Protocol upload path: inject each image through the SigV4 upload script.
async fn upload_images_via_protocol(
  session: &mut WayfernSession,
  paths: &[PathBuf],
  cancel: &AtomicBool,
  update: &(dyn Fn(DoubaoProgress) + Send + Sync),
) -> Result<Vec<Value>, DoubaoExecutorError> {
  let mut uploaded = Vec::new();
  let total = paths.len();
  for (index, path) in paths.iter().enumerate() {
    check_cancel(cancel)?;
    let (name, mime, b64) = load_image_base64(path)?;
    update(DoubaoProgress {
      status: Some("starting".into()),
      error_message: Some(format!("正在上传图片 {}/{total}", index + 1)),
      ..Default::default()
    });
    let image = call_script(
      session,
      UPLOAD_IMAGE_SCRIPT,
      &json!({ "name": name, "mime": mime, "base64Data": b64 }),
    )
    .await
    .map_err(DoubaoExecutorError::Message)?;
    uploaded.push(image);
  }
  Ok(uploaded)
}

/// 参数确认可能先于会话 URL 出现，也可能生成 URL 后才完整显示。提交等待和
/// 结果轮询共用这套检查，不再要求先拿到 conversation id 才开始看确认文案。
async fn auto_confirm_generation(
  session: &mut WayfernSession,
  cancel: &AtomicBool,
  update: &(dyn Fn(DoubaoProgress) + Send + Sync),
  last_confirmed_cue: &mut Option<String>,
) -> Result<bool, DoubaoExecutorError> {
  let Some(cue) = page_confirmation_cue(session).await else {
    return Ok(false);
  };
  if last_confirmed_cue.as_deref() == Some(cue.as_str()) {
    return Ok(false);
  }
  tokio::time::sleep(Duration::from_secs(2)).await;
  let Some(current_cue) = page_confirmation_cue(session).await else {
    return Ok(false);
  };
  if current_cue != cue {
    return Ok(false);
  }
  debug_log(&format!("页面出现确认提示，自动回复：{cue}"));
  update(DoubaoProgress {
    status: Some("starting".into()),
    error_message: Some("豆包要求确认生成，自动回复确认中...".into()),
    ..Default::default()
  });
  match ui_send_confirmation(session, cancel).await {
    Ok(true) => {
      *last_confirmed_cue = Some(cue);
      update(DoubaoProgress {
        status: Some("starting".into()),
        error_message: Some("已发送确认，等待豆包开始生成...".into()),
        ..Default::default()
      });
      Ok(true)
    }
    Ok(false) => {
      debug_log("确认回复没有发送回执，下一轮继续尝试");
      update(DoubaoProgress {
        status: Some("starting".into()),
        error_message: Some("确认回复尚未发送成功，正在重试...".into()),
        ..Default::default()
      });
      Ok(false)
    }
    Err(err) if matches!(&err, DoubaoExecutorError::Message(m) if is_gate_error(m)) => {
      update(DoubaoProgress {
        status: Some("verifying".into()),
        error_message: Some(
          "豆包要求确认生成，请在该账号浏览器窗口输入「开始生成视频」并发送；处理期间不计入生成超时".into(),
        ),
        ..Default::default()
      });
      Ok(false)
    }
    Err(err) => Err(err),
  }
}

/// Wait for the UI submit to land: the URL gains /chat/<conversation id>.
async fn wait_for_ui_conversation(
  session: &mut WayfernSession,
  cancel: &AtomicBool,
  update: &(dyn Fn(DoubaoProgress) + Send + Sync),
) -> Result<String, DoubaoExecutorError> {
  let mut confirmed_cue = None;
  for attempt in 0..60 {
    check_cancel(cancel)?;
    // 「安全确认」弹窗会盖住输入框，头几轮各点一次，别让它挡住会话创建
    if matches!(attempt, 1 | 4) {
      accept_safety_consent(session).await;
    }
    let url = current_url(session).await;
    if let Some(id) = conv_url_conversation_id(&url) {
      if confirmed_cue.is_none() {
        auto_confirm_generation(session, cancel, update, &mut confirmed_cue).await?;
      }
      return Ok(id);
    }
    if confirmed_cue.is_none() {
      auto_confirm_generation(session, cancel, update, &mut confirmed_cue).await?;
    }
    // 提交后被额度拦下：豆包不建会话，只在页面上写「额度已用完」
    if attempt % 3 == 1 {
      if let Some(notice) = page_quota_notice(session).await {
        debug_log(&format!("提交后豆包直接给出额度提示，换号：{notice}"));
        return Err(DoubaoExecutorError::RateLimited(notice));
      }
    }
    if captcha_visible(session).await {
      update(DoubaoProgress {
        status: Some("verifying".into()),
        error_message: Some("请在浏览器窗口完成人机验证".into()),
        ..Default::default()
      });
      for _ in 0..100 {
        check_cancel(cancel)?;
        if !captcha_visible(session).await {
          break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
      }
      // 过码后消息可能未自动发出：输入框仍有文字则补一次回车。
      // 门禁下读不到输入框文字，一律补按（空输入框回车无害）。
      match evaluate(session, INPUT_BOX_TEXT_JS, false).await {
        Ok(remain) => {
          if remain.as_str().map(|s| !s.is_empty()).unwrap_or(false) {
            let _ = press_enter(session).await;
          }
        }
        Err(e) if is_gate_error(&e) => {
          let _ = press_enter(session).await;
        }
        Err(_) => {}
      }
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
  }
  Err(DoubaoExecutorError::Message(
    "未能创建会话，请在浏览器窗口检查登录状态".into(),
  ))
}

fn conv_url_conversation_id(url: &str) -> Option<String> {
  url
    .split("/chat/")
    .nth(1)
    .and_then(|rest| rest.split(['?', '/', '&']).next())
    .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
    .map(str::to_string)
}

/// Read a captured network response body (base64 or plain) as JSON.
async fn response_body_json(
  session: &mut WayfernSession,
  request_id: &str,
) -> Result<Value, String> {
  let body = session
    .call(
      "Network.getResponseBody",
      json!({ "requestId": request_id }),
    )
    .await
    .map_err(|e| format!("读取豆包链路响应失败: {e}"))?;
  let raw = if body
    .get("base64Encoded")
    .and_then(Value::as_bool)
    .unwrap_or(false)
  {
    let encoded = body.get("body").and_then(Value::as_str).unwrap_or_default();
    let bytes = base64::engine::general_purpose::STANDARD
      .decode(encoded)
      .map_err(|e| format!("豆包链路响应解码失败: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("豆包链路响应解码失败: {e}"))?
  } else {
    body
      .get("body")
      .and_then(Value::as_str)
      .unwrap_or_default()
      .to_string()
  };
  serde_json::from_str(&raw).map_err(|e| format!("豆包链路响应解析失败: {e}"))
}

/// Click the download control on the finished-video card. Doubao has shipped it
/// both as a text button and as a labelled icon (`aria-label` / `title`), so
/// hover the newest card, try the last visible 「下载」 leaf, then the attributes.
async fn click_page_download(session: &mut WayfernSession) -> Option<String> {
  let root = dom_doc_root(session).await.ok()?;
  // 豆包卡片的操作工具条只在指针悬停时挂出来；先把鼠标停在最新那条视频上。
  if let Some((x, y)) = dom_last_box_center(session, root, "video").await {
    if hover_at(session, x, y).await.is_ok() {
      tokio::time::sleep(Duration::from_millis(400)).await;
    }
  }
  // 文本「下载」取的是最后一个可见匹配，正好是刚生成的那张卡片；属性选择器
  // 命中的是文档里第一个，可能是左侧栏的下载中心，只当兜底。
  if let Some((x, y)) = dom_find_leaf_point(session, "下载", 0.0).await {
    if click_at(session, x, y).await.is_ok() {
      return Some("文本「下载」".to_string());
    }
  }
  for selector in ["[aria-label*=\"下载\"]", "[title*=\"下载\"]"] {
    let Some(node) = dom_query(session, root, selector).await else {
      continue;
    };
    dom_scroll_into_view(session, node).await;
    let Some((x, y, _, _)) = dom_box_center(session, node).await else {
      continue;
    };
    if click_at(session, x, y).await.is_ok() {
      return Some(selector.to_string());
    }
  }
  None
}

/// 门禁下无水印原链的取法：注入脚本发不了 `get_download_info`，但豆包卡片上的
/// 「下载」按钮发的就是同一个请求。让页面自己发，我们只从 Network 事件里读响应体。
async fn original_url_via_page_download(
  session: &mut WayfernSession,
  cancel: &AtomicBool,
) -> Option<String> {
  if session.call("Network.enable", json!({})).await.is_err() {
    return None;
  }
  // 后台标签页可能已经不重绘卡片的悬停工具条，切到前台再点。
  let _ = session.call("Page.bringToFront", json!({})).await;
  let hit = click_page_download(session).await?;
  debug_log(&format!(
    "已点击豆包卡片「下载」（{hit}），正在从页面请求里取无水印原链"
  ));
  let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
  while tokio::time::Instant::now() < deadline {
    check_cancel(cancel).ok()?;
    let Ok(Some((_, params))) = session
      .await_any_event(&["Network.responseReceived"], Duration::from_secs(2))
      .await
    else {
      continue;
    };
    let url = params
      .pointer("/response/url")
      .and_then(Value::as_str)
      .unwrap_or_default();
    if !url.contains("/samantha/aispace/get_download_info") {
      continue;
    }
    if params.pointer("/response/status").and_then(Value::as_u64) != Some(200) {
      continue;
    }
    let Some(request_id) = params
      .pointer("/response/requestId")
      .or_else(|| params.get("requestId"))
      .and_then(Value::as_str)
    else {
      continue;
    };
    let Ok(reply) = response_body_json(session, request_id).await else {
      continue;
    };
    if let Some(info) = crate::doubao_protocol::parse_download_info(&reply["data"]) {
      if !info.result_url.is_empty() {
        debug_log("已从页面「下载」请求取到无水印原链");
        return Some(info.result_url);
      }
    }
  }
  debug_log("页面「下载」30 秒内没有返回下载信息，保留带水印链接；可在账号窗口人工点一次「下载」");
  None
}

/// Resolve the watermark-free original download URL through the aispace
/// endpoints; falls back to the chain's download_url on any miss.
async fn resolve_original_download(
  session: &mut WayfernSession,
  ack: &crate::doubao_protocol::SseAck,
  result: &crate::doubao_protocol::CreationResult,
  cancel: &AtomicBool,
) -> DoubaoRunResult {
  let mut merged = DoubaoRunResult {
    conversation_id: ack.conversation_id.clone(),
    section_id: ack.section_id.clone(),
    question_id: ack.question_id.clone(),
    remote_task_id: result.remote_task_id.clone(),
    vid: result.vid.clone(),
    fallback_result_url: result.fallback_result_url.clone(),
    cover_url: result.cover_url.clone(),
    result_url: result.fallback_result_url.clone(),
    ..Default::default()
  };
  let homepage = call_fetch_script(
    session,
    AISPACE_SCRIPT,
    &json!({ "endpoint": "/samantha/aispace/homepage", "body": {} }),
    "创作空间",
  )
  .await;
  let Ok(homepage) = homepage else {
    // 脚本请求被门禁拒绝时走页面「下载」，那条路不需要 Runtime
    if let Some(url) = original_url_via_page_download(session, cancel).await {
      merged.result_url = url;
    }
    return merged;
  };
  let Some(folder_id) = crate::doubao_protocol::find_creation_directory(&homepage["data"]) else {
    return merged;
  };
  if result.vid.is_empty() {
    return merged;
  }

  let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
  let mut node_id: Option<String> = None;
  while tokio::time::Instant::now() < deadline {
    check_cancel(cancel).ok();
    if let Ok(nodes) = call_fetch_script(
      session,
      AISPACE_SCRIPT,
      &json!({
        "endpoint": "/samantha/aispace/node_info",
        "body": {
          "node_id": folder_id,
          "need_full_path": true,
          "size": 50,
          "sort_param": { "need_sort_config": true, "sort_order": 1, "sort_type": 0 },
        },
      }),
      "创作节点",
    )
    .await
    {
      node_id = crate::doubao_protocol::find_video_node(&nodes["data"], &result.vid);
    }
    if node_id.is_some() {
      break;
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
  }
  let Some(node_id) = node_id else {
    return merged;
  };
  if let Ok(download) = call_fetch_script(
    session,
    AISPACE_SCRIPT,
    &json!({
      "endpoint": "/samantha/aispace/get_download_info",
      "body": { "requests": [{ "node_id": node_id }] },
    }),
    "下载信息",
  )
  .await
  {
    if let Some(info) = crate::doubao_protocol::parse_download_info(&download["data"]) {
      merged.result_url = info.result_url;
      merged.backup_result_url = info.backup_result_url;
    }
  }
  merged
}

/// Gate mode: the page polls /im/chain/single by itself; listen for those
/// responses and read the body, instead of issuing our own fetch.
async fn poll_chain_passive(session: &mut WayfernSession) -> Result<Option<Value>, String> {
  match session
    .await_any_event(&["Network.responseReceived"], Duration::from_secs(1))
    .await
  {
    Ok(Some((_method, params))) => {
      let url = params
        .pointer("/response/url")
        .and_then(Value::as_str)
        .unwrap_or_default();
      if !url.contains("/im/chain/single") {
        return Ok(None);
      }
      let status = params
        .pointer("/response/status")
        .and_then(Value::as_u64)
        .unwrap_or(0);
      if status != 200 {
        return Ok(None);
      }
      let request_id = params
        .pointer("/response/requestId")
        .or_else(|| params.get("requestId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
      if request_id.is_empty() {
        return Ok(None);
      }
      response_body_json(session, &request_id).await.map(Some)
    }
    Ok(None) => Ok(None),
    Err(e) => Err(format!("监听豆包链路响应失败: {e}")),
  }
}

/// 单个任务允许的人工介入总时长：过验证、手点「确认」都从这里扣，扣完仍卡住
/// 就按验证失败交给调度器重试，避免一个死号把任务无限挂住。
const HUMAN_WAIT_BUDGET: Duration = Duration::from_secs(900);

/// Poll the conversation chain until the video is finished, auto-confirming
/// Doubao's parameter-confirmation step on the way.
async fn poll_result(
  session: &mut WayfernSession,
  ack: &crate::doubao_protocol::SseAck,
  ui_submit: bool,
  fingerprint: &str,
  params: &DoubaoRunParams,
  cancel: &AtomicBool,
  update: &(dyn Fn(DoubaoProgress) + Send + Sync),
) -> Result<DoubaoRunResult, DoubaoExecutorError> {
  let effective_timeout = if params.timeout_secs > 0 {
    params.timeout_secs
  } else {
    // 未指定时按时长放大：15s 视频生成时间远超 5s，固定 7 分钟不够
    420u64.max(u64::from(params.duration) * 60)
  };
  let poll_interval = Duration::from_secs(params.poll_interval_secs.max(1));
  let mut deadline = tokio::time::Instant::now() + Duration::from_secs(effective_timeout);
  let mut confirmed = false;
  let mut passive = false;
  let mut human_budget = HUMAN_WAIT_BUDGET;
  let mut last_chain = tokio::time::Instant::now();
  let mut last_nudge = tokio::time::Instant::now();
  let mut confirmed_cue = None;
  let mut round = 0u32;
  let mut ack = ack.clone();

  update(DoubaoProgress {
    status: Some("generating".into()),
    error_message: None,
    conversation_id: Some(ack.conversation_id.clone()),
    section_id: Some(ack.section_id.clone()),
    question_id: Some(ack.question_id.clone()),
  });

  while tokio::time::Instant::now() < deadline {
    check_cancel(cancel)?;
    round += 1;
    // 额度用尽是豆包在页面上写的一句话，不是接口错误：每 4 轮（约 20 秒）读一次，
    // 命中就按额度受限交给调度器换号。全量 DOM 查询不该每轮都做。
    if round % 4 == 0 {
      if let Some(notice) = page_quota_notice(session).await {
        debug_log(&format!("监听中豆包给出额度提示，换号：{notice}"));
        return Err(DoubaoExecutorError::RateLimited(notice));
      }
    }
    // 门禁下没法替用户过码：人工处理验证期间冻结生成超时，处理完继续监听。
    if passive && round % 4 == 0 && captcha_visible(session).await {
      update(DoubaoProgress {
        status: Some("verifying".into()),
        error_message: Some(
          "浏览器窗口出现人机验证，请在该账号窗口手动完成；处理期间不计入生成超时".into(),
        ),
        ..Default::default()
      });
      let started = tokio::time::Instant::now();
      loop {
        check_cancel(cancel)?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        if !captcha_visible(session).await {
          break;
        }
        if started.elapsed() >= human_budget {
          debug_log("人机验证超时仍未处理，按验证失败交给调度器重试");
          return Err(DoubaoExecutorError::VerifyFailed);
        }
      }
      let waited = started.elapsed();
      human_budget = human_budget.saturating_sub(waited);
      deadline += waited;
      last_chain = tokio::time::Instant::now();
      update(DoubaoProgress {
        status: Some("generating".into()),
        error_message: Some("验证已处理，继续监听豆包结果...".into()),
        ..Default::default()
      });
      continue;
    }
    // 不依赖网络监听何时返回：每一轮都先检查页面是否正在等参数确认。
    if !confirmed
      && round % 2 == 0
      && auto_confirm_generation(session, cancel, update, &mut confirmed_cue).await?
    {
      confirmed = true;
      last_chain = tokio::time::Instant::now();
      tokio::time::sleep(Duration::from_secs(3)).await;
      continue;
    }
    let data: Value = if passive {
      match poll_chain_passive(session).await {
        Ok(Some(data)) => {
          last_chain = tokio::time::Instant::now();
          data
        }
        Ok(None) => {
          // 被动监听靠页面自己的轮询定时器，窗口在后台会被浏览器限流；
          // 90s 收不到任何链路响应就把它顶到前台，并告诉人工该看一眼了。
          if last_chain.elapsed() >= Duration::from_secs(90)
            && last_nudge.elapsed() >= Duration::from_secs(60)
          {
            last_nudge = tokio::time::Instant::now();
            let _ = session.call("Page.bringToFront", json!({})).await;
            update(DoubaoProgress {
              error_message: Some(
                "被动监听已 90 秒没有收到豆包结果：已把该账号窗口切到前台，请在窗口确认豆包正在生成（需要时手动刷新页面或处理弹窗）".into(),
              ),
              ..Default::default()
            });
          }
          tokio::time::sleep(poll_interval).await;
          continue;
        }
        Err(e) => {
          debug_log(&format!("本次被动链路读取未完成，等待页面下一次轮询：{e}"));
          tokio::time::sleep(poll_interval).await;
          continue;
        }
      }
    } else {
      match call_fetch_script(
        session,
        CHAIN_SCRIPT,
        &json!({ "conversationId": ack.conversation_id }),
        "豆包结果",
      )
      .await
      {
        Ok(chain) => chain["data"].clone(),
        // 门禁：不注入脚本，改为监听页面自身的 /im/chain/single 轮询
        Err(e) if is_gate_error(&e) => {
          session
            .call("Network.enable", json!({}))
            .await
            .map_err(|e| DoubaoExecutorError::Message(format!("Network.enable 失败: {e}")))?;
          update(DoubaoProgress {
            error_message: Some("已切换为被动监听，正在等待豆包生成结果...".into()),
            ..Default::default()
          });
          passive = true;
          continue;
        }
        Err(e) => return Err(DoubaoExecutorError::Message(e)),
      }
    };
    if let Some(result) = crate::doubao_protocol::parse_creation_result(&data) {
      update(DoubaoProgress {
        status: Some("resolving".into()),
        ..Default::default()
      });
      return Ok(resolve_original_download(session, &ack, &result, cancel).await);
    }
    // 豆包新流程：先返回参数确认消息，需回复"确认"后才真正生成
    if !confirmed && crate::doubao_protocol::is_awaiting_confirmation(&data) {
      update(DoubaoProgress {
        error_message: Some("豆包要求确认参数，自动回复确认中...".into()),
        ..Default::default()
      });
      if ui_submit || passive {
        // 被动模式下注入的脚本请求发不出去，改为在输入框回复「确认」
        match ui_send_confirmation(session, cancel).await {
          Ok(sent) => confirmed = sent,
          Err(err) if matches!(&err, DoubaoExecutorError::Message(m) if is_gate_error(m)) => {
            debug_log(&format!("自动回复确认失败（门禁）：{err}"));
            update(DoubaoProgress {
              status: Some("verifying".into()),
              error_message: Some(
                "豆包要求确认参数，自动回复被页面脚本限制挡住：请在该账号浏览器窗口输入「开始生成视频」并发送；处理期间不计入生成超时".into(),
              ),
              ..Default::default()
            });
            // 人工点确认不该占用生成预算
            deadline += HUMAN_WAIT_BUDGET;
            last_chain = tokio::time::Instant::now();
          }
          Err(err) => return Err(err),
        }
      } else {
        let payload =
          crate::doubao_protocol::build_confirm_payload(&ack.conversation_id, fingerprint);
        let confirm_resp = call_fetch_script(
          session,
          COMPLETION_SCRIPT,
          &json!({ "payload": payload }),
          "确认回复",
        )
        .await
        .map_err(DoubaoExecutorError::Message)?;
        if let Ok(confirm_ack) =
          crate::doubao_protocol::parse_sse_ack(confirm_resp["text"].as_str().unwrap_or(""))
        {
          ack = confirm_ack;
        }
        confirmed = true;
      }
      tokio::time::sleep(Duration::from_secs(3)).await;
      continue;
    }
    tokio::time::sleep(poll_interval).await;
  }
  Err(DoubaoExecutorError::Timeout)
}

/// Run one video-generation task against the profile's doubao tab.
pub async fn run_task(
  profile: &BrowserProfile,
  params: &DoubaoRunParams,
  cancel: &AtomicBool,
  update: &(dyn Fn(DoubaoProgress) + Send + Sync),
) -> Result<DoubaoRunResult, DoubaoExecutorError> {
  let mut session = open_chat_session(profile, cancel).await?;
  assert_logged_in(&mut session, cancel).await?;

  // (ui path, ready ack) — the protocol path already carries an SSE ack, the
  // UI path builds one from the page URL after the message is sent.
  if params.mode == "i2v" {
    if params.image_paths.is_empty() {
      return Err(DoubaoExecutorError::Message("图生视频缺少本地图片".into()));
    }
    if params.image_paths.len() > crate::doubao_manager::MAX_I2V_IMAGES {
      return Err(DoubaoExecutorError::Message(
        "图生视频最多支持 9 张图片".into(),
      ));
    }
    for path in &params.image_paths {
      if !path.is_file() {
        return Err(DoubaoExecutorError::Message(format!(
          "图片不存在：{}",
          path.display()
        )));
      }
    }
    switch_to_video_mode(&mut session, cancel).await?;
    update(DoubaoProgress {
      status: Some("starting".into()),
      error_message: Some("正在通过浏览器上传图片...".into()),
      ..Default::default()
    });
    // 素材图一律先走页面：粘贴按一张张来，不需要输入框支持 multiple。
    // 只有「Runtime 可用 + 多图」才退回协议上传（注入脚本签名提交，缺前端签名会触发 verify 风控）
    if params.image_paths.len() == 1 || runtime_gated(&mut session).await {
      // UI 路径：豆包前端自己上传（fetch 协议提交缺前端签名会触发 verify 风控）
      upload_images_via_page(&mut session, &params.image_paths, cancel).await?;
      tokio::time::sleep(Duration::from_secs(2)).await;
      update(DoubaoProgress {
        status: Some("starting".into()),
        error_message: Some("正在浏览器输入框输入任务...".into()),
        ..Default::default()
      });
      let conversation_id =
        ui_send_task(&mut session, &params.prompt, false, cancel, update).await?;
      return poll_result(
        &mut session,
        &crate::doubao_protocol::SseAck {
          conversation_id,
          section_id: String::new(),
          question_id: String::new(),
        },
        true,
        "",
        params,
        cancel,
        update,
      )
      .await;
    }
    // 多图且 Runtime 可用：协议上传 + 接口提交
    let mut fingerprint = read_fingerprint(&mut session, cancel).await?;
    let images =
      upload_images_via_protocol(&mut session, &params.image_paths, cancel, update).await?;
    let payload =
      crate::doubao_protocol::build_completion_payload(crate::doubao_protocol::CompletionParams {
        prompt: &params.prompt,
        model: &params.model,
        ratio: &params.ratio,
        duration: params.duration,
        fingerprint: &fingerprint,
        mode: &params.mode,
        images: &images
          .iter()
          .map(|image| crate::doubao_protocol::DoubaoImageRef {
            uri: image
              .get("uri")
              .and_then(Value::as_str)
              .unwrap_or_default()
              .to_string(),
            identifier: image
              .get("identifier")
              .and_then(Value::as_str)
              .unwrap_or_default()
              .to_string(),
            name: image
              .get("name")
              .and_then(Value::as_str)
              .map(str::to_string),
            width: image.get("width").and_then(Value::as_i64),
            height: image.get("height").and_then(Value::as_i64),
            url: image.get("url").and_then(Value::as_str).map(str::to_string),
          })
          .collect::<Vec<_>>(),
        conversation_id: None,
        need_create_conversation: true,
      })
      .map_err(DoubaoExecutorError::Message)?;
    let local_id = payload
      .pointer("/client_meta/local_conversation_id")
      .and_then(Value::as_str)
      .unwrap_or_default()
      .to_string();
    let _ = evaluate(
      &mut session,
      &format!("history.replaceState({{}}, '', '/chat/{local_id}')"),
      false,
    )
    .await;
    let ack = send_completion_with_verify(
      &mut session,
      payload,
      params,
      &mut fingerprint,
      cancel,
      update,
    )
    .await?;
    return poll_result(
      &mut session,
      &ack,
      false,
      &fingerprint,
      params,
      cancel,
      update,
    )
    .await;
  }

  // t2v：先尝试挂上视频技能。豆包工作里「生成视频：xxx」只是普通聊天文字，
  // 不带技能时不会真正进入视频生成流程。挂不上就退回旧的发文字方式，
  // 让还能靠文字触发视频的旧页面继续可用。
  let skill_attached = switch_to_video_mode(&mut session, cancel).await.is_ok();
  update(DoubaoProgress {
    status: Some("starting".into()),
    error_message: Some("正在浏览器输入框输入任务...".into()),
    ..Default::default()
  });
  let text = if skill_attached {
    format!("{}，{}", params.prompt, params.ratio)
  } else {
    format!("生成视频：{}，{}", params.prompt, params.ratio)
  };
  let conversation_id = ui_send_task(&mut session, &text, !skill_attached, cancel, update).await?;
  poll_result(
    &mut session,
    &crate::doubao_protocol::SseAck {
      conversation_id,
      section_id: String::new(),
      question_id: String::new(),
    },
    true,
    "",
    params,
    cancel,
    update,
  )
  .await
}

/// Protocol submission with the verify-captcha retry loop (protocol path only).
#[allow(clippy::too_many_arguments)]
async fn send_completion_with_verify(
  session: &mut WayfernSession,
  payload: Value,
  params: &DoubaoRunParams,
  fingerprint: &mut String,
  cancel: &AtomicBool,
  update: &(dyn Fn(DoubaoProgress) + Send + Sync),
) -> Result<crate::doubao_protocol::SseAck, DoubaoExecutorError> {
  let mut payload = payload;
  for attempt in 0..4u32 {
    check_cancel(cancel)?;
    let response = call_fetch_script(
      session,
      COMPLETION_SCRIPT,
      &json!({ "payload": payload }),
      "豆包提交",
    )
    .await
    .map_err(DoubaoExecutorError::Message)?;
    let text = response["text"].as_str().unwrap_or("");
    match crate::doubao_protocol::parse_sse_ack(text) {
      Ok(ack) => return Ok(ack),
      Err(crate::doubao_protocol::DoubaoProtocolError::VerifyRequired(_)) => {
        if attempt >= 3 {
          return Err(DoubaoExecutorError::VerifyFailed);
        }
        session
          .call("Page.navigate", json!({ "url": DOUBAO_CHAT_URL }))
          .await
          .map_err(|e| DoubaoExecutorError::Message(format!("刷新豆包页面失败: {e}")))?;
        tokio::time::sleep(Duration::from_secs(5)).await;
        update(DoubaoProgress {
          status: Some("verifying".into()),
          error_message: Some("请在浏览器窗口完成人机验证".into()),
          ..Default::default()
        });
        // 码"出现过再消失"才算过码完成；一直不弹(刷新不弹码)最多等60s继续重试
        let mut seen_captcha = false;
        for check in 0..100u32 {
          check_cancel(cancel)?;
          let visible = captcha_visible(session).await;
          if visible {
            seen_captcha = true;
          }
          if seen_captcha && !visible {
            break;
          }
          if !seen_captcha && check >= 20 {
            break;
          }
          tokio::time::sleep(Duration::from_secs(3)).await;
        }
        *fingerprint = read_fingerprint(session, cancel).await?;
        let images: Vec<crate::doubao_protocol::DoubaoImageRef> = payload["messages"]
          .as_array()
          .and_then(|messages| messages.first())
          .and_then(|first| first.pointer("/content_block/0/content/attachment_block/attachments"))
          .and_then(Value::as_array)
          .map(|attachments| {
            attachments
              .iter()
              .map(|a| crate::doubao_protocol::DoubaoImageRef {
                uri: a
                  .pointer("/image/uri")
                  .and_then(Value::as_str)
                  .unwrap_or_default()
                  .to_string(),
                identifier: a
                  .get("identifier")
                  .and_then(Value::as_str)
                  .unwrap_or_default()
                  .to_string(),
                name: a
                  .pointer("/image/name")
                  .and_then(Value::as_str)
                  .map(str::to_string),
                width: a.pointer("/image/image_ori/width").and_then(Value::as_i64),
                height: a.pointer("/image/image_ori/height").and_then(Value::as_i64),
                url: a
                  .pointer("/image/image_ori/url")
                  .and_then(Value::as_str)
                  .map(str::to_string),
              })
              .collect()
          })
          .unwrap_or_default();
        payload = crate::doubao_protocol::build_completion_payload(
          crate::doubao_protocol::CompletionParams {
            prompt: &params.prompt,
            model: &params.model,
            ratio: &params.ratio,
            duration: params.duration,
            fingerprint,
            mode: &params.mode,
            images: &images,
            conversation_id: None,
            need_create_conversation: true,
          },
        )
        .map_err(DoubaoExecutorError::Message)?;
      }
      Err(crate::doubao_protocol::DoubaoProtocolError::RateLimited(m)) => {
        return Err(DoubaoExecutorError::RateLimited(m));
      }
      Err(crate::doubao_protocol::DoubaoProtocolError::TooManyRequests(m)) => {
        return Err(DoubaoExecutorError::TooManyRequests(m));
      }
      Err(e) => return Err(DoubaoExecutorError::Message(e.to_string())),
    }
  }
  Err(DoubaoExecutorError::VerifyFailed)
}

/// Re-attach to an existing conversation and keep polling until it finishes.
/// Used after an app restart or tab close mid-generation; the doubao server
/// keeps generating regardless of our browser session.
pub async fn resume_poll(
  profile: &BrowserProfile,
  conversation_id: &str,
  section_id: &str,
  question_id: &str,
  params: &DoubaoRunParams,
  cancel: &AtomicBool,
  update: &(dyn Fn(DoubaoProgress) + Send + Sync),
) -> Result<DoubaoRunResult, DoubaoExecutorError> {
  if conversation_id.is_empty() {
    return Err(DoubaoExecutorError::Message("缺少可续听的豆包会话".into()));
  }
  let url = format!("{DOUBAO_CHAT_URL}{conversation_id}");
  let mut session = open_chat_session_at(profile, &url, cancel).await?;
  assert_logged_in(&mut session, cancel).await?;
  poll_result(
    &mut session,
    &crate::doubao_protocol::SseAck {
      conversation_id: conversation_id.to_string(),
      section_id: section_id.to_string(),
      question_id: question_id.to_string(),
    },
    false,
    "",
    params,
    cancel,
    update,
  )
  .await
}

/// One video pulled from the account's creation space (watermark-free link).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PulledVideo {
  pub vid: String,
  pub node_id: String,
  pub name: String,
  pub result_url: String,
  pub backup_result_url: String,
}

/// Scan the account's creation space for finished videos with direct links.
pub async fn pull_videos(
  profile: &BrowserProfile,
  limit: u32,
  cancel: &AtomicBool,
) -> Result<Vec<PulledVideo>, DoubaoExecutorError> {
  let mut session = open_chat_session(profile, cancel).await?;
  assert_logged_in(&mut session, cancel).await?;
  let homepage = call_fetch_script(
    &mut session,
    AISPACE_SCRIPT,
    &json!({ "endpoint": "/samantha/aispace/homepage", "body": {} }),
    "创作空间",
  )
  .await
  .map_err(|e| {
    if is_gate_error(&e) {
      DoubaoExecutorError::Message(
        "浏览器内核限制了页面脚本执行（免费版自动化门禁），无法拉取创作空间视频".into(),
      )
    } else {
      DoubaoExecutorError::Message(e)
    }
  })?;
  let folder_id = crate::doubao_protocol::find_creation_directory(&homepage["data"])
    .ok_or_else(|| DoubaoExecutorError::Message("未找到创作目录（我的创作）".to_string()))?;
  let nodes = call_fetch_script(
    &mut session,
    AISPACE_SCRIPT,
    &json!({
      "endpoint": "/samantha/aispace/node_info",
      "body": {
        "node_id": folder_id,
        "need_full_path": true,
        "size": limit,
        "sort_param": { "need_sort_config": true, "sort_order": 1, "sort_type": 0 },
      },
    }),
    "创作节点",
  )
  .await
  .map_err(DoubaoExecutorError::Message)?;
  let mut videos = Vec::new();
  for node in crate::doubao_protocol::parse_creation_nodes(&nodes["data"]) {
    if let Ok(download) = call_fetch_script(
      &mut session,
      AISPACE_SCRIPT,
      &json!({
        "endpoint": "/samantha/aispace/get_download_info",
        "body": { "requests": [{ "node_id": node.node_id }] },
      }),
      "下载信息",
    )
    .await
    {
      if let Some(info) = crate::doubao_protocol::parse_download_info(&download["data"]) {
        videos.push(PulledVideo {
          vid: node.vid,
          node_id: node.node_id,
          name: node.name,
          result_url: info.result_url,
          backup_result_url: info.backup_result_url,
        });
      }
    }
  }
  Ok(videos)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn script_node(text: &str) -> Value {
    json!({
      "nodeName": "SCRIPT",
      "nodeType": 1,
      "children": [{ "nodeType": 3, "nodeValue": text }]
    })
  }

  #[test]
  fn test_ssr_reports_expired_markers() {
    assert!(ssr_reports_expired(
      r#"{"data":{"error_code":13,"name":"account_info_error"}}"#
    ));
    assert!(!ssr_reports_expired(
      r#"{"data":{"user_id":123,"session_key":"x"}}"#
    ));
  }

  #[test]
  fn test_collect_script_text_reads_router_data() {
    let tree = json!({
      "nodeName": "#document",
      "nodeType": 1,
      "children": [
        script_node("window._ROUTER_DATA = {\"loaderData\":{\"chat_layout\":{\"accountInfo\":{\"data\":{\"name\":\"account_info_error\"}}}}};")
      ]
    });
    let mut text = String::new();
    assert!(collect_script_text(&tree, &mut text));
    assert!(ssr_reports_expired(&text));
  }

  #[test]
  fn test_collect_script_text_without_router_data() {
    let tree =
      json!({ "nodeName": "#document", "nodeType": 1, "children": [script_node("var a = 1;")] });
    let mut text = String::new();
    assert!(!collect_script_text(&tree, &mut text));
  }

  #[test]
  fn test_quota_notice_keeps_only_the_verdict_sentence() {
    let page = "视频生成\n近 7 天免费额度已用完，预计 11 月 9 日 10:31 恢复。开通订阅畅享更多额度。\n开通豆包订阅";
    assert_eq!(
      quota_notice(page),
      Some("近 7 天免费额度已用完，预计 11 月 9 日 10:31 恢复。".into())
    );
    // 前半句里出现过句号，前面那段别跟着抄进提示
    let dirty = "上一条回答结束了。近 7 天免费额度已用完，预计 11 月 9 日 10:31 恢复。";
    assert_eq!(
      quota_notice(dirty),
      Some("近 7 天免费额度已用完，预计 11 月 9 日 10:31 恢复。".into())
    );
    // 节点把句子拆开、且没有哪一行是完整的话：抹平换行拼回来
    let split = "近 7 天免费额度已用完\n预计 11 月 9 日 10:31 恢复。";
    assert_eq!(
      quota_notice(split),
      Some("近 7 天免费额度已用完预计 11 月 9 日 10:31 恢复。".into())
    );
  }

  #[test]
  fn test_quota_notice_ignores_the_permanent_upsell() {
    assert_eq!(quota_notice("开通豆包订阅，畅享更多额度。"), None);
    assert_eq!(quota_notice("视频生成中，请稍候。"), None);
  }

  #[test]
  fn test_confirmation_cue_matches_the_new_copy() {
    assert_eq!(
      confirmation_cue("图片已就绪。点击确认生成。"),
      Some("点击确认生成。".into())
    );
    assert_eq!(
      confirmation_cue("确认后我再开始生成视频。"),
      Some("确认后我再开始生成视频。".into())
    );
    assert_eq!(confirmation_cue("正在生成视频，请稍候。"), None);
  }

  #[test]
  fn test_broken_upload_listener_does_not_trigger_file_input_fallback() {
    let error = DoubaoExecutorError::Browser(
      "监听上传请求失败: CDP WebSocket error: Message too long".into(),
    );
    assert!(!can_retry_upload_with_file_input(&error));
    assert!(can_retry_upload_with_file_input(
      &DoubaoExecutorError::Message("粘贴第 1 张素材图没有生效：没有等到豆包的图片上传请求".into())
    ));
  }
}
