// 视频嗅探下载 - background service worker
// 职责：
//   1. chrome.webRequest 观察所有 tab 的媒体类请求（video/*.m3u8/mpd 等），
//      按 tabId 缓存（存 chrome.storage.session，SW 休眠不丢）
//   2. 响应 content script 的 SNAPSHOT（取本 tab 嗅探结果）与 DOWNLOAD
//      （POST 到 BwBrowser 本地 REST 免 token 窄端点，直接入队视频下载）

const VIDEO_EXT =
  /\.(mp4|m3u8|mpd|flv|webm|mov|m4v|avi|mkv|ts|3gp|ogv)(\?|#|$)/i;
// 分片/片段不是完整视频（.ts=HLS 段 / .m4s=DASH 段），只保留 .m3u8 交给 yt-dlp
const SEGMENT_RE = /\.(ts|m4s)(\?|#|$)/i;
const VIDEO_MIME =
  /^(video\/|application\/(vnd\.apple\.mpegurl|x-mpegurl|dash\+xml)|audio\/mpegurl)/i;

// 常驻「视频入队」mini server 端口（主程序启动即监听，不依赖 API 开关）；
// 10108 起为旧的完整 API server 端口（api_enabled 开启时）作为兜底
const API_PORTS = [10118, 10119, 10120, 10108, 10000, 10001, 10002];

function tabKey(tabId) {
  return `sniff_${tabId}`;
}

function isVideoRequest(url, contentType) {
  if (SEGMENT_RE.test(url)) return false;
  if (VIDEO_EXT.test(url)) return true;
  if (contentType && VIDEO_MIME.test(contentType)) return true;
  return false;
}

async function rememberVideo(tabId, url) {
  try {
    const key = tabKey(tabId);
    const existing = (await chrome.storage.session.get(key))[key] || {};
    // url -> { url, ts }，幂等去重
    existing[url] = { url, ts: Date.now() };
    // 每个 tab 最多记 80 条，防极端页面撑爆 session storage
    const entries = Object.values(existing)
      .sort((a, b) => b.ts - a.ts)
      .slice(0, 80);
    await chrome.storage.session.set({
      [key]: Object.fromEntries(entries.map((e) => [e.url, e])),
    });
  } catch (e) {
    // storage 满或不可用：静默放弃本条
  }
}

chrome.webRequest.onCompleted.addListener(
  (details) => {
    if (details.tabId < 0) return; // 非标签页请求（SW 自身等）
    const contentType =
      details.responseHeaders?.find(
        (h) => h.name.toLowerCase() === "content-type",
      )?.value || "";
    if (!isVideoRequest(details.url, contentType)) return;
    rememberVideo(details.tabId, details.url);
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest"] },
  ["responseHeaders"],
);

// tab 关闭 / 新导航时清理缓存
chrome.tabs.onRemoved.addListener((tabId) => {
  chrome.storage.session.remove(tabKey(tabId));
});
chrome.tabs.onUpdated.addListener((tabId, info) => {
  if (info.status === "loading") {
    chrome.storage.session.remove(tabKey(tabId));
  }
});

async function enqueueUrl(url) {
  for (const port of API_PORTS) {
    try {
      const res = await fetch(
        `http://127.0.0.1:${port}/v1/video-download/enqueue`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ url }),
        },
      );
      if (res.ok) {
        const data = await res.json().catch(() => ({}));
        return { ok: true, taskId: data.taskId || null, port };
      }
    } catch (e) {
      // 端口不通，试下一个
    }
  }
  return {
    ok: false,
    error: "无法连接 BwBrowser 主程序（请确认主程序已启动）",
  };
}

// 轮询一次任务进度（常驻 mini server 的免 token 只读端点）
async function pollTask(taskId, port) {
  for (const p of port
    ? [port, ...API_PORTS.filter((x) => x !== port)]
    : API_PORTS) {
    try {
      const res = await fetch(
        `http://127.0.0.1:${p}/v1/video-download/task/${encodeURIComponent(taskId)}`,
      );
      if (res.ok) {
        const data = await res.json().catch(() => ({}));
        if (data.found) return { ok: true, ...data };
        return { ok: false, error: "任务不存在" };
      }
    } catch (e) {
      // 试下一个端口
    }
  }
  return { ok: false, error: "无法查询进度" };
}

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (msg?.type === "SNAPSHOT") {
    // content script 无 sender.tab（扩展页调用时才可能），正常页面一定有
    const tabId = sender.tab?.id ?? msg.tabId;
    if (tabId === undefined) {
      sendResponse({ videos: [] });
      return;
    }
    chrome.storage.session
      .get(tabKey(tabId))
      .then((data) => {
        const entry = data[tabKey(tabId)] || {};
        const list = Object.values(entry)
          .sort((a, b) => b.ts - a.ts)
          .slice(0, 50);
        sendResponse({ videos: list });
      })
      .catch(() => sendResponse({ videos: [] }));
    return true; // async sendResponse
  }
  if (msg?.type === "DOWNLOAD") {
    enqueueUrl(String(msg.url || "")).then(sendResponse);
    return true; // async sendResponse
  }
  if (msg?.type === "POLL") {
    pollTask(String(msg.taskId || ""), msg.port).then(sendResponse);
    return true; // async sendResponse
  }
});
