// 视频嗅探下载 - MAIN world hook（document_start 注入，先于页面脚本）
// 在页面世界里 hook fetch / XMLHttpRequest，抓取视频类请求的 URL，
// 通过 window.postMessage 转发给 content script（isolated world）。
// 为什么不靠 background 的 webRequest：MV3 service worker 30s 就休眠，
// 休眠期间的请求事件全部丢失；页面内 hook 与页面同生命周期，永不丢。
(() => {
  if (window.__bwvsHooked) return;
  window.__bwvsHooked = true;

  const EXT_RE =
    /\.(mp4|m3u8|mpd|flv|webm|mov|m4v|avi|mkv|ts|3gp|ogv)(\?|#|$)/i;
  // 分片/片段不是完整视频：.ts（HLS 段）/ .m4s（DASH 段）下下来只有几秒钟，
  // 完整 HLS 用 .m3u8（yt-dlp 会自动合并全部分段），必须排除。
  const IGNORE_RE = /\.(ts|m4s)(\?|#|$)/i;
  const MIME_RE =
    /^(video\/|application\/(vnd\.apple\.mpegurl|x-mpegurl|dash\+xml)|audio\/mpegurl)/i;

  // 同 URL 只报一次；超上限丢最旧，防极端页面撑爆内存
  const seen = new Map();
  const MAX = 200;
  const report = (url, mime) => {
    if (!url || typeof url !== "string") return;
    if (!/^https?:\/\//i.test(url)) return;
    if (IGNORE_RE.test(url)) return; // 分片不是完整视频
    if (!EXT_RE.test(url) && !(mime && MIME_RE.test(mime))) return;
    if (seen.has(url)) return;
    seen.set(url, Date.now());
    if (seen.size > MAX) {
      let oldest = null;
      let oldestTs = Infinity;
      for (const [u, ts] of seen) {
        if (ts < oldestTs) {
          oldestTs = ts;
          oldest = u;
        }
      }
      if (oldest) seen.delete(oldest);
    }
    try {
      window.postMessage({ type: "bwvs-media", url }, "*");
    } catch (e) {
      /* ignore */
    }
  };

  // ---- hook fetch
  const origFetch = window.fetch;
  if (origFetch) {
    window.fetch = function (...args) {
      const p = origFetch.apply(this, args);
      p.then((res) => {
        try {
          const reqUrl =
            typeof args[0] === "string"
              ? args[0]
              : args[0] && args[0].url
                ? args[0].url
                : res.url || "";
          report(
            reqUrl,
            (res.headers && res.headers.get("content-type")) || "",
          );
        } catch (e) {
          /* ignore */
        }
      }).catch(() => {});
      return p;
    };
  }

  // ---- hook XMLHttpRequest
  const origOpen = XMLHttpRequest.prototype.open;
  const origSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    try {
      this.__bwvsUrl = typeof url === "string" ? url : String(url || "");
    } catch (e) {
      /* ignore */
    }
    return origOpen.call(this, method, url, ...rest);
  };
  XMLHttpRequest.prototype.send = function (...args) {
    try {
      this.addEventListener("load", () => {
        try {
          report(this.__bwvsUrl, this.getResponseHeader("content-type") || "");
        } catch (e) {
          /* ignore */
        }
      });
    } catch (e) {
      /* ignore */
    }
    return origSend.apply(this, args);
  };
})();
