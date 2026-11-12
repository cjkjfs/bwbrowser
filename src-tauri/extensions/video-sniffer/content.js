// 视频嗅探下载 - content script（精简版）
// 右下角悬浮「识别视频」圆球。点击后在页面每个可见 <video> 播放器元素
// 的正确方向位置叠一个「下载」按钮（一视频一按钮，随元素位置/滚动跟随）。
// 若某播放器 src 是 blob:/data:（无 http 直链），则用 background 嗅探命中
// 的真实视频直链（m3u8/mp4…）作为该按钮的下载地址。
// 点击按钮 -> background POST 本地 REST -> 主程序视频下载页自动入队。

(() => {
  if (window.__bwVideoSnifferInjected) return;
  window.__bwVideoSnifferInjected = true;

  // ---------------------------------------------------- 页面世界 hook 注入
  // hook.js 必须在页面脚本之前跑（抓 fetch/XHR）。此 Chromium 对 manifest 的
  // world:"MAIN" 不生效，改用向页面注入 <script> 的传统方式：
  // src 指向扩展资源（不受页面 CSP 限制），失败再 inline 兜底。
  try {
    const s = document.createElement("script");
    s.src = chrome.runtime.getURL("hook.js");
    s.onload = () => s.remove();
    s.onerror = () => {
      // inline 兜底（严格 CSP 站点可能拦截 inline script）
      try {
        const s2 = document.createElement("script");
        s2.src = chrome.runtime.getURL("hook.js") + "?r=" + Date.now();
        (document.head || document.documentElement).appendChild(s2);
      } catch (e) {
        /* ignore */
      }
    };
    (document.head || document.documentElement).appendChild(s);
  } catch (e) {
    /* ignore */
  }

  // ------------------------------------------------------------ 悬浮球(Shadow)
  const host = document.createElement("div");
  host.style.cssText =
    "all:initial; position:fixed; right:12px; bottom:12px; z-index:2147483647;";
  const shadow = host.attachShadow({ mode: "closed" });

  const s2 = document.createElement("style");
  s2.textContent = `
    * { box-sizing: border-box; margin: 0; padding: 0; font-family: system-ui, sans-serif; }
    .fab {
      width: 44px; height: 44px; border-radius: 50%;
      background: linear-gradient(135deg, #6366f1, #8b5cf6);
      color: #fff; border: none; cursor: pointer;
      box-shadow: 0 4px 12px rgba(0,0,0,.25);
      font-size: 20px; line-height: 1; text-align: center;
      display: flex; align-items: center; justify-content: center;
    }
    .fab:hover { transform: scale(1.08); }
    .fab.active { background: linear-gradient(135deg, #ef4444, #f97316); }
    .fab-row { display: flex; align-items: center; gap: 8px; justify-content: flex-end; }
    .fab-cancel {
      height: 40px; padding: 0 14px; border-radius: 20px;
      background: rgba(40,40,50,.92); color: #fff; border: 1px solid rgba(255,255,255,.25);
      cursor: pointer; font-size: 13px; font-weight: 600;
      box-shadow: 0 4px 12px rgba(0,0,0,.25);
      display: flex; align-items: center; justify-content: center;
      white-space: nowrap;
    }
    .fab-cancel:hover { background: #ef4444; border-color: #ef4444; }
    .fab-cancel:disabled { opacity: .45; cursor: default; background: rgba(40,40,50,.6); }
    .status { text-align: center; font-size: 11px; color: #fff; opacity:.85; padding: 4px 0 0; }
    /* 兜底条：页面里没有对应播放器元素、但嗅探到的可下载直链 */
    .extra {
      position: fixed; right: 12px; bottom: 64px;
      max-width: 360px; max-height: 45vh; overflow-y: auto;
      background: rgba(20,20,30,.94); color: #fff;
      border-radius: 10px; padding: 8px 10px;
      box-shadow: 0 6px 24px rgba(0,0,0,.35);
      font-size: 12px; display: none; flex-direction: column; gap: 6px;
    }
    .extra.show { display: flex; }
    .extra-head {
      font-weight: 600; display: flex; align-items: center;
      justify-content: space-between; gap: 8px;
    }
    .extra-close {
      background: none; border: none; color: #aaa; cursor: pointer;
      font-size: 14px; line-height: 1; padding: 0;
    }
    .extra-row { display: flex; align-items: center; gap: 8px; }
    .extra-url {
      flex: 1; min-width: 0; color: #ccc;
      white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
      direction: rtl; text-align: left;
    }
    .extra-dl {
      flex-shrink: 0; border: none; cursor: pointer;
      background: #6366f1; color: #fff;
      font-size: 11px; padding: 3px 10px; border-radius: 5px;
    }
    .extra-dl:disabled { background: #555; color: #999; }
  `;
  const fab = document.createElement("button");
  fab.className = "fab";
  fab.title = "重新识别页面视频（下载按钮已自动显示）";
  fab.textContent = "▶";
  const status = document.createElement("div");
  status.className = "status";
  // 全局「取消」按钮：点击后让当前页面所有下载按钮消失，重新点识别球才恢复。
  const cancelAllBtn = document.createElement("button");
  cancelAllBtn.className = "fab-cancel";
  cancelAllBtn.textContent = "隐藏";
  cancelAllBtn.title = "取消所有视频的下载按钮（重新点 ▶ 识别可恢复）";
  cancelAllBtn.disabled = true; // 无按钮时置灰，有按钮时点亮
  const fabRow = document.createElement("div");
  fabRow.className = "fab-row";
  fabRow.appendChild(cancelAllBtn);
  fabRow.appendChild(fab);

  // 兜底条：嗅探到但没有对应播放器元素可贴的可下载直链（紧凑小条，非大面板）
  const extra = document.createElement("div");
  extra.className = "extra";
  const extraHead = document.createElement("div");
  extraHead.className = "extra-head";
  const extraTitle = document.createElement("span");
  const extraClose = document.createElement("button");
  extraClose.className = "extra-close";
  extraClose.textContent = "×";
  extraClose.title = "收起";
  extraHead.appendChild(extraTitle);
  extraHead.appendChild(extraClose);
  const extraBody = document.createElement("div");
  extra.appendChild(extraHead);
  extra.appendChild(extraBody);
  extraClose.addEventListener("click", () => extra.classList.remove("show"));

  shadow.appendChild(s2);
  shadow.appendChild(fabRow);
  shadow.appendChild(status);
  shadow.appendChild(extra);
  document.documentElement.appendChild(host);

  // ------------------------------------------------------------ 轻提示
  const dstyle = document.createElement("style");
  dstyle.textContent = `
    .bwvs-tip {
      position: fixed; left: 50%; bottom: 80px; transform: translateX(-50%);
      z-index: 2147483647; max-width: 80%;
      background: #111; color: #fff; padding: 8px 16px; border-radius: 8px;
      font-size: 13px; box-shadow: 0 4px 16px rgba(0,0,0,.3); pointer-events: none;
      opacity: 0; transition: opacity .25s;
    }
    .bwvs-tip.show { opacity: 1; }
    .bwvs-tip.ok { background: #059669; }
    .bwvs-tip.err { background: #dc2626; }
    /* 点击下载：一个圆点从按钮飞到视口右下角（下载托盘方位） */
    .bwvs-fly {
      position: fixed; z-index: 2147483647; pointer-events: none;
      width: 26px; height: 26px; border-radius: 50%;
      background: linear-gradient(135deg, #6366f1, #22c55e);
      box-shadow: 0 4px 14px rgba(99,102,241,.5);
      display: flex; align-items: center; justify-content: center;
      font-size: 14px; color: #fff; font-weight: 700;
      transform: translate(0,0); opacity: 1;
      transition: transform .8s cubic-bezier(.22,.9,.3,1.4), opacity .7s ease;
      will-change: transform, opacity;
    }
    .bwvs-fly::after {
      content: "↓"; line-height: 1;
    }
  `;
  document.head
    ? document.head.appendChild(dstyle)
    : document.addEventListener("DOMContentLoaded", () =>
        document.head.appendChild(dstyle),
      );

  let tipEl = null;
  const showTip = (text, ok) => {
    if (!tipEl) {
      tipEl = document.createElement("div");
      tipEl.className = "bwvs-tip";
      document.body.appendChild(tipEl);
    }
    tipEl.className = `bwvs-tip show ${ok ? "ok" : "err"}`;
    tipEl.textContent = text;
    clearTimeout(showTip._t);
    showTip._t = setTimeout(() => (tipEl.className = "bwvs-tip"), 2200);
  };

  // 下载飞行动画：从源元素(按钮)位置飞一个圆点进视口右下角（主程序下载托盘方位），
  // 再轻提示「下载已开始」。源元素可为 DOM 节点或坐标对。
  const flyIntoDownloadTray = (from) => {
    const dot = document.createElement("div");
    dot.className = "bwvs-fly";
    document.body.appendChild(dot);
    let sx, sy;
    if (from && typeof from.getBoundingClientRect === "function") {
      const r = from.getBoundingClientRect();
      sx = r.left + r.width / 2;
      sy = r.top + r.height / 2;
    } else if (from) {
      [sx, sy] = from;
    } else {
      sx = vw / 2;
      sy = vh / 2;
    }
    const tx = window.innerWidth - 34; // 主程序右下角下载托盘
    const ty = window.innerHeight - 34;
    dot.style.left = sx - 13 + "px";
    dot.style.top = sy - 13 + "px";
    requestAnimationFrame(() => {
      dot.style.transform = `translate(${tx - sx}px, ${ty - sy}px) scale(.5)`;
      dot.style.opacity = "0";
    });
    setTimeout(() => dot.remove(), 900);
  };

  // ------------------------------------------------------------ 页面内嗅探
  // 接收 hook.js（MAIN world）postMessage 过来的视频请求 URL。
  // 与页面同生命周期，不依赖 background SW（MV3 SW 休眠会丢 webRequest 事件）。
  const hookedUrls = new Map(); // url -> ts
  window.addEventListener("message", (ev) => {
    const d = ev.data;
    if (d && d.type === "bwvs-media" && typeof d.url === "string") {
      hookedUrls.set(d.url, Date.now());
      if (hookedUrls.size > 200) {
        let oldest = null;
        let oldestTs = Infinity;
        for (const [u, ts] of hookedUrls) {
          if (ts < oldestTs) {
            oldestTs = ts;
            oldest = u;
          }
        }
        if (oldest) hookedUrls.delete(oldest);
      }
    }
  });

  // ------------------------------------------------------------ 发送下载
  // 入队成功后轮询进度：按钮显示「百分比 / 速度」，终态显示 ✓ / ✗
  const sendToDownloader = async (url, btn) => {
    const old = btn.textContent;
    btn.disabled = true;
    btn.textContent = "…";
    flyIntoDownloadTray(btn); // 先播飞行动画（无论成败都有「扔进下载托盘」的反馈）
    try {
      const res = await chrome.runtime.sendMessage({ type: "DOWNLOAD", url });
      if (res?.ok) {
        showTip("已加入视频下载 ✓（程序内可见进度）", true);
        pollProgress(res.taskId, res.port, btn, old);
      } else {
        showTip(res?.error || "发送失败", false);
        btn.disabled = false;
        btn.textContent = old;
      }
    } catch (e) {
      showTip("发送失败：" + e, false);
      btn.disabled = false;
      btn.textContent = old;
    }
  };

  const pollProgress = (taskId, port, btn, fallbackText) => {
    if (!taskId) {
      btn.textContent = "✓"; // 入队成功但旧版程序无 taskId，仅提示完成
      return;
    }
    let stopped = false;
    btn._stopPoll = () => {
      stopped = true;
    };
    const tick = async () => {
      if (stopped) return;
      let r = null;
      try {
        r = await chrome.runtime.sendMessage({ type: "POLL", taskId, port });
      } catch (e) {
        /* ignore */
      }
      if (stopped) return;
      if (r?.ok && r.found) {
        const st = String(r.status || "");
        if (st === "completed") {
          btn.textContent = "✓";
          showTip("下载完成 ✓", true);
          return;
        }
        if (st === "error" || st === "cancelled") {
          btn.textContent = "✗";
          btn.title = r.errorMsg || st;
          showTip(r.errorMsg || "下载失败", false);
          return;
        }
        if (st === "downloading") {
          const pct = Math.round(Number(r.progress) || 0);
          btn.textContent = `${pct}%`;
        } else {
          // waiting / paused / 解析中
          btn.textContent = r.statusText || "等待中";
        }
      }
      // 继续轮询（任务在程序里由用户暂停/取消也如实反映）
      setTimeout(tick, 1000);
    };
    tick();
  };

  // ------------------------------------------------------------ 叠放按钮
  // direction: 相对视频的方向（"right-top" 右上角 / "left-bottom" 左下角…，默认贴元素内右上）
  const overlayBtns = []; // {btn, el, place}
  const overlaidEls = new Set();
  // 用户点右下角「取消」后为 true：不再叠加任何下载按钮；点悬浮球识别时复位
  let canceledAll = false;

  const placeBtn = (rec) => {
    const r = rec.el.getBoundingClientRect();
    if (!r || r.width === 0 || r.height === 0) {
      rec.btn.style.display = "none";
      return;
    }
    // 元素不在视口内（滚出屏幕/被遮挡成不可见）→ 隐藏按钮，避免悬空漂浮在视口边缘。
    // position:fixed 相对视口，若元素本身滚出了可视区，按钮不应仍贴在视野里。
    const vw = window.innerWidth,
      vh = window.innerHeight;
    const visible = r.right > 0 && r.left < vw && r.bottom > 0 && r.top < vh;
    if (!visible) {
      rec.btn.style.display = "none";
      return;
    }
    const bw = rec.btn.offsetWidth || 54;
    const bh = rec.btn.offsetHeight || 24;
    const gap = 8;
    const dir = rec.dir || "right-top";
    if (dir === "top-left") {
      rec.btn.style.left = r.left + gap + "px";
      rec.btn.style.top = r.top + gap + "px";
    } else if (dir === "bottom-right") {
      rec.btn.style.left = r.right - bw - gap + "px";
      rec.btn.style.top = r.bottom - bh - gap + "px";
    } else if (dir === "bottom-left") {
      rec.btn.style.left = r.left + gap + "px";
      rec.btn.style.top = r.bottom - bh - gap + "px";
    } else {
      // right-top
      rec.btn.style.left = r.right - bw - gap + "px";
      rec.btn.style.top = r.top + gap + "px";
    }
    rec.btn.style.display = "";
  };

  const addOverlay = (el, url, dir, viaPage) => {
    if (canceledAll) return; // 用户已点「取消」：不再叠加任何按钮
    if (overlaidEls.has(el)) return;
    const r = el.getBoundingClientRect();
    // 只跳过完全不可见的（display:none 等）；小缩略图也识别，保证"页面所有视频"都出按钮
    if (!r || r.width === 0 || r.height === 0) return;
    overlaidEls.add(el);

    const btn = document.createElement("button");
    btn.textContent = viaPage ? "下载视频" : "下载";
    btn.title = viaPage
      ? "解析本页面视频（YT·抖音·TikTok 等平台）\n" + url
      : url;
    btn.style.cssText =
      "position:fixed;z-index:2147483646;font:12px/1 system-ui,sans-serif;background:#6366f1;color:#fff;border:none;cursor:pointer;padding:5px 12px;border-radius:6px;box-shadow:0 2px 8px rgba(0,0,0,.35);";
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      sendToDownloader(url, btn);
    });
    document.body.appendChild(btn);
    const rec = { btn, el, dir: dir || "right-top", url };
    rec.place = () => placeBtn(rec);
    rec.place();
    overlayBtns.push(rec);
    refreshCancelState();
    if (window.ResizeObserver) {
      rec.ro = new ResizeObserver(rec.place);
      rec.ro.observe(el);
    }
    // 滚轮/尺寸变化统一跟随；监听只在首次叠加时挂一次
    if (overlayBtns.length === 1) {
      window.addEventListener("scroll", placeAll, true);
      window.addEventListener("resize", placeAll);
    }
  };

  const placeAll = () => overlayBtns.forEach((rec) => rec.place());

  // viaPage 按钮地址同步：平台页（如 TikTok 主页）hover 到别的卡片时，
  // 若复用同一个 <video> 元素换 src，按钮已存在但地址要跟着卡片链接更新
  const syncOverlayUrl = (d) => {
    const rec = overlayBtns.find((r) => r.el === d.el);
    if (!rec || rec.url === d.url) return;
    rec.url = d.url;
    rec.btn.title = d.viaPage
      ? "解析本页面视频（YT·抖音·TikTok 等平台）\n" + d.url
      : d.url;
  };

  const dispose = () => {
    if (overlayBtns.length) {
      window.removeEventListener("scroll", placeAll, true);
      window.removeEventListener("resize", placeAll);
    }
    overlayBtns.forEach((rec) => {
      if (rec.ro) rec.ro.disconnect();
      if (typeof rec.btn._stopPoll === "function") rec.btn._stopPoll();
      if (rec.btn.parentNode) rec.btn.parentNode.removeChild(rec.btn);
    });
    overlayBtns.length = 0;
    overlaidEls.clear();
    extra.classList.remove("show");
    refreshCancelState();
  };

  // ------------------------------------------------------------ 已知视频平台判定
  // 这些平台的「页面 URL」即可被主程序 yt-dlp 直接解析出视频，即使页面里的
  // <video> 是 blob:/MSE（无 http 直链）也能下。与 video_downloader.rs 白名单一致。
  const PLATFORM_RE =
    /(youtube\.com|youtu\.be|bilibili\.com|b23\.tv|vimeo\.com|tiktok\.com|douyin\.com|kuaishou\.com|xigua\.com|ixigua\.com|weibo\.com|twitter\.com|x\.com|reddit\.com|twitch\.tv)/i;
  const isPlatformPage = () => PLATFORM_RE.test(location.hostname);
  // 平台页里，「主播放器」判定：正在播 +5 / 有时长 +3 / 播放中 +2 / 面积加分。
  // ⚠️ 低于阈值不算主播放器（列表页/首页的推荐卡片面积小且未播，不误绑页面 URL）
  const scoreVideo = (v) => {
    let s = 0;
    if (v.duration && !isNaN(v.duration)) s += 3;
    if (!v.paused && !v.ended) s += 5;
    if (v.currentTime > 0) s += 2;
    const r = v.getBoundingClientRect();
    if (r && r.width && r.height)
      s += Math.min(10, (r.width * r.height) / 60000);
    return s;
  };
  const MAIN_SCORE_MIN = 3;

  // ------------------------------------------------------------ 收集视频元素与直链
  // 返回 [{el, url, dir, viaPage}]：优先真实 http 直链；否则占位待嗅探补。
  // viaPage=true 表示该按钮的下载地址=页面 URL（交 yt-dlp 解析），用于平台页。
  // ⚠️ TikTok 播放 API 地址（/aweme/v1/play/?...）不是真实视频文件：带签名
  // token、短时效，yt-dlp generic 提取器直接抓会 403。必须换算成视频详情页
  // 地址（/@用户/video/ID）交给 yt-dlp 的 TikTok 提取器。
  const TIKTOK_PLAY_RE = /^https?:\/\/[^/]*tiktok\.com\/aweme\/v1\/play\//i;
  // 平台「视频详情页」地址格式（列表页/主页 URL 不算——yt-dlp 会当成合集
  // 下载第一个视频，不是用户点的那个）
  // 抖音实际卡片/详情页 URL 形态（实测只认 /video/\d+ 时 9 种只中 1 种）：
  //   /video/7361234567890                直接详情页
  //   /@用户名/video/7361234567890        主页卡片链接（带 @）
  //   /user/UID/video/7361234567890       旧格式卡片
  //   /note/7361234567890                 图集详情页
  //   /discover?modal_id=7361234567890    弹窗形式（视频 ID 在查询参数，
  //     精选/关注/搜索页的卡片最常见这种）——由 cleanPageUrl 换算成 /video/id
  const VIDEO_PAGE_URL_RE =
    /tiktok\.com\/@[^/]+\/video\/\d+|youtube\.com\/(watch|shorts)|youtu\.be\/|bilibili\.com\/video\/|douyin\.com\/(?:@[^/]+\/|user\/[^/]+\/)?(?:video|note)\/\d+|ixigua\.com\/\d+/i;
  // 抖音弹窗形式卡片（视频 ID 在 modal_id 查询参数里）
  const DOUYIN_MODAL_RE = /douyin\.com\/[^\s]*[?&]modal_id=(\d+)/i;
  const isVideoDetailHref = (href) =>
    VIDEO_PAGE_URL_RE.test(href) || DOUYIN_MODAL_RE.test(href);
  // 解析某 video 元素对应的「视频详情页」地址：
  // ① 卡片祖先链上的 <a href=详情页>（主页/列表页每张卡片本身就是链接，实测可用）
  // ② 当前页面本身就是详情页（watch 页）
  // 都拿不到 → null（宁可不出按钮，也不绑主页/列表页 URL 下错视频）
  // 清洗平台详情页 URL：去掉会话/追踪参数，但**必须保留查询参数里的视频 ID**。
  // ⚠️ split("?")[0] 对 YouTube 是破坏性操作：watch?v=ID 的 ID 在查询参数里，
  // 直接切会得到 youtube.com/watch（无 ID），交给 yt-dlp 会下成别的视频（实测踩坑）。
  // 唯一 ID 在查询参数里的是 YouTube watch/?v=，须 strip 掉其它参数但保留 v。
  const cleanPageUrl = (href) => {
    // 抖音 modal_id 弹窗形式：视频 ID 在查询参数里，换算成规范详情页。
    // 放在 split("?") 之前——对弹窗 URL 直接去参数会丢掉视频 ID（与 YT v= 同坑）。
    const mi = href.match(/[?&]modal_id=(\d+)/i);
    if (mi && /douyin\.com/i.test(href))
      return "https://www.douyin.com/video/" + mi[1];
    const qi = href.indexOf("?");
    if (qi < 0) return href;
    const base = href.slice(0, qi);
    const qs = href.slice(qi + 1);
    if (/youtube\.com\/watch|youtu\.be\//i.test(href)) {
      const vm = qs.match(/(?:^|&)v=([^&]+)/);
      if (vm) return base + "?v=" + vm[1];
    }
    return base;
  };
  // 解析某 video 元素对应的「视频详情页」地址：
  // ① 卡片祖先链上的 href（主页/列表页每张卡片本身就是链接，实测可用）
  //    ⚠️ 抖音卡片是**带 href 属性的 div**（React 模拟链接），不是 <a> 标签
  //    ——closest("a[href]") 会漏掉，必须 closest("[href]") + getAttribute 取值，
  //    且 div 无 .href getter，协议相对值（//host/...）需用 URL 补全。
  // ② 当前页面本身就是详情页（watch 页）
  // 都拿不到 → null（宁可不出按钮，也不绑主页/列表页 URL 下错视频）
  const hrefOf = (el) => {
    // 优先 DOM 属性（<a> 的 .href 是解析后的完整 URL），div 用 getAttribute
    const raw =
      typeof el.href === "string" && el.href
        ? el.href
        : el.getAttribute("href") || "";
    if (!raw) return "";
    try {
      return new URL(raw, location.href).href; // 补全协议相对 //host/...
    } catch (e) {
      return raw;
    }
  };
  const resolveCardPageUrl = (v) => {
    const a = v.closest && v.closest("[href]");
    if (a && isVideoDetailHref(hrefOf(a))) return cleanPageUrl(hrefOf(a));
    if (isVideoDetailHref(location.href)) return cleanPageUrl(location.href);
    return null;
  };

  const collect = () => {
    const out = [];
    const seenEl = new Set();
    const platform = isPlatformPage();
    let mainVideo = null;
    let mainScore = -1;
    document.querySelectorAll("video").forEach((v) => {
      if (seenEl.has(v)) return;
      seenEl.add(v);
      let url = "";
      let viaPage = false;
      const native = v.currentSrc || v.getAttribute("src") || "";
      if (native && /^https?:\/\//i.test(native)) url = native;
      if (!url) {
        const s = v.querySelector("source[src]");
        if (s) {
          const ss = s.getAttribute("src") || "";
          if (/^https?:\/\//i.test(ss)) url = ss;
        }
      }
      // TikTok play API 地址 → 换算成详情页地址（yt-dlp TikTok 提取器稳定）
      if (url && TIKTOK_PLAY_RE.test(url)) {
        const pageUrl = resolveCardPageUrl(v);
        if (pageUrl) {
          url = pageUrl;
          viaPage = true; // 地址来自页面链接而非媒体直链，按钮文案区分
        }
      }
      // 平台页：记下「主播放器」，稍后用页面 URL 兜底（低于阈值不算，防列表页误绑）
      if (platform && !url) {
        const sc = scoreVideo(v);
        if (sc >= MAIN_SCORE_MIN && sc > mainScore) {
          mainScore = sc;
          mainVideo = v;
        }
      }
      out.push({ el: v, url: url || null, dir: "right-top", viaPage });
    });
    // 平台页主播放器无 http 直链（blob:/MSE）→ 解析它所属卡片的详情页地址。
    // ⚠️ 绝不绑主页/列表页 location.href：yt-dlp 会把主页当合集下载第一个视频，
    // 不是用户点的那个（实测踩坑：@cartooncrack2 主页 URL 被直接入队）。
    if (platform && mainVideo) {
      for (const d of out) {
        if (d.el === mainVideo && !d.url) {
          const pageUrl = resolveCardPageUrl(mainVideo);
          if (pageUrl) {
            d.url = pageUrl;
            d.viaPage = true;
          }
          break;
        }
      }
    }
    // 卡片链接来源（主页/列表页）：TikTok 等平台只有 hover 的卡片才渲染
    // <video>，但**每张卡片本身就是 <a href=详情页>**——直接以卡片链接为锚点
    // 出按钮，不依赖 hover。按 URL 去重（video 按钮已绑的地址跳过）。
    if (platform) {
      const seenUrl = new Set(out.filter((d) => d.url).map((d) => d.url));
      // ⚠️ 按钮去重：卡片既有 <video>（已出「下载」/「下载视频」）又有 <a href=详情页>
      // 时，两者矩形重叠（都贴右上角）会叠出两个按钮。凡与已收集 video 显著重叠的
      // <a> 锚点视为「同一卡片」，跳过，只保留 card 里第一个（video）按钮。
      const videoEls = out.map((d) => d.el);
      const overlapsVideo = (a) => {
        const ra = a.getBoundingClientRect();
        if (!ra || ra.width === 0 || ra.height === 0) return false;
        for (const v of videoEls) {
          const rv = v.getBoundingClientRect();
          if (!rv || rv.width === 0 || rv.height === 0) continue;
          const ox = Math.min(ra.right, rv.right) - Math.max(ra.left, rv.left);
          const oy = Math.min(ra.bottom, rv.bottom) - Math.max(ra.top, rv.top);
          if (ox > 0 && oy > 0) {
            const ov = ox * oy;
            const min = Math.min(ra.width * ra.height, rv.width * rv.height);
            if (min > 0 && ov / min >= 0.5) return true; // 重叠过半即同一卡片
          }
        }
        return false;
      };
      // ⚠️ 扫 [href] 而不是 a[href]：抖音瀑布流卡片是**带 href 属性的 div**
      //（React 模拟链接，实测 66 张卡片 0 个 <a>），只扫 a 会全部漏掉。
      // href 取值/补全用 hrefOf（div 无 .href getter）。
      document.querySelectorAll("[href]").forEach((el) => {
        const href = hrefOf(el);
        if (!isVideoDetailHref(href)) return;
        const u = cleanPageUrl(href);
        if (seenUrl.has(u)) return;
        const r = el.getBoundingClientRect();
        if (!r || r.width === 0 || r.height === 0) return; // 虚拟滚动未渲染
        if (overlapsVideo(el)) return; // 同一卡片已有 video 按钮，去重
        seenUrl.add(u);
        out.push({ el, url: u, dir: "right-top", viaPage: true });
      });
      // 兜底：抖音部分元素带 data-aweme-id（视频 ID）属性，直接换算详情页
      document.querySelectorAll("[data-aweme-id]").forEach((el) => {
        const id = el.getAttribute("data-aweme-id") || "";
        if (!/^\d{10,}$/.test(id)) return;
        const u = "https://www.douyin.com/video/" + id;
        if (seenUrl.has(u)) return;
        const r = el.getBoundingClientRect();
        if (!r || r.width === 0 || r.height === 0) return;
        if (overlapsVideo(el)) return;
        seenUrl.add(u);
        out.push({ el, url: u, dir: "right-top", viaPage: true });
      });
    }
    return out;
  };

  // ------------------------------------------------------------ 激活
  // manual=true：悬浮球手动重扫（先清旧按钮，找到/找不到都弹提示）
  // manual=false：自动扫描（增量补按钮，不打扰；供页面加载与 DOM 变化时调用）
  const engage = async (manual) => {
    if (manual) dispose();
    const domList = collect();

    // 先给有直链的叠按钮（平台页主视频为 viaPage 卡片详情页地址）
    domList.forEach((d) => {
      if (!d.url) return;
      if (overlaidEls.has(d.el)) {
        if (d.viaPage) syncOverlayUrl(d); // hover 换卡片 → 地址跟着更新
        return;
      }
      addOverlay(d.el, d.url, d.dir, d.viaPage);
    });

    // 嗅探来源 1：页面内 hook（主力，与页面同生命周期不丢事件）
    // 嗅探来源 2：background webRequest（兜底，SW 休眠期会丢事件）
    let sniffed = [...hookedUrls.keys()];
    try {
      const res = await chrome.runtime.sendMessage({ type: "SNAPSHOT" });
      const bg = (res?.videos || []).map((v) => v.url).filter(Boolean);
      for (const u of bg) if (!sniffed.includes(u)) sniffed.push(u);
    } catch (e) {
      /* ignore */
    }
    // TikTok play API 地址（签名短时效，yt-dlp generic 抓 403）不进嗅探列表；
    // 视频按钮的对应地址已在 collect 里换算成详情页链接。
    // 另过滤 ttwstatic/login_static 等 UI 装饰视频（界面动画素材，不是内容视频）
    const STATIC_ASSET_RE =
      /ttwstatic|login_static|\/obj\/|sf16-|sf3-|common\/blended/i;
    sniffed = sniffed.filter(
      (u) => !TIKTOK_PLAY_RE.test(u) && !STATIC_ASSET_RE.test(u),
    );
    // 优先级排序：完整视频文件 > 播放列表(m3u8/mpd，yt-dlp 会合并分段) > 其它
    const rankUrl = (u) =>
      /\.mp4(\?|#|$)/i.test(u) ? 3 : /\.(m3u8|mpd)(\?|#|$)/i.test(u) ? 2 : 1;
    sniffed.sort((a, b) => rankUrl(b) - rankUrl(a));

    // ⚠️ 配对原则：绝不按顺序「猜配」。DOM 里 blob 播放器和嗅探直链之间没有
    // 可靠的对应关系，多个 video 按序分配必然张冠李戴（视频1绑到视频3的地址）。
    // 只有「恰好一个无直链 video」时才绑最优直链；多余的直链直接丢弃
    // （用户要求不显示「其它可下载视频」列表，宁缺毋滥）。
    const noSrc = domList.filter((d) => !d.url && !overlaidEls.has(d.el));
    if (noSrc.length === 1 && sniffed.length) {
      addOverlay(noSrc[0].el, sniffed[0], noSrc[0].dir, false);
    }

    if (manual && overlayBtns.length === 0) {
      showTip("未识别到视频，请先播放再点识别", false);
      return false;
    }
    return true;
  };

  // ---------------------------------------------------- 自动显示（不用点球）
  // 页面加载后直接把「下载」按钮叠在每个视频上；DOM 新增 video（SPA/懒加载）
  // 由 MutationObserver 触发重扫；blob:/MSE 播放器等直链晚出现的场景由低频
  // 轮询兜底（嗅探直链到位后自动补按钮）。
  let autoTimer = 0;
  let autoStarted = false;
  const scheduleScan = () => {
    clearTimeout(autoTimer);
    autoTimer = setTimeout(() => engage(false), 600);
  };
  const startAuto = () => {
    if (autoStarted || !document.body) return;
    autoStarted = true;
    engage(false);
    if (window.MutationObserver) {
      new MutationObserver(scheduleScan).observe(document.body, {
        childList: true,
        subtree: true,
      });
    }
    // 低频兜底轮询：只有页面还有「可见但没按钮」的视频时才值得扫
    setInterval(() => {
      const uncovered = [...document.querySelectorAll("video")].some((v) => {
        if (overlaidEls.has(v)) return false;
        const r = v.getBoundingClientRect();
        return r && r.width > 0 && r.height > 0;
      });
      if (uncovered) engage(false);
    }, 2500);
  };
  if (document.body) {
    startAuto();
  } else {
    document.addEventListener("DOMContentLoaded", startAuto, { once: true });
  }

  // 兜底条渲染：无 DOM 播放器可贴的嗅探直链（保证页面所有可下载视频都被识别）
  const renderExtra = (urls) => {
    extraBody.textContent = "";
    extra.classList.remove("show");
    if (!urls.length) return;
    extraTitle.textContent = `其它可下载视频（${urls.length}）`;
    urls.forEach((u) => {
      const row = document.createElement("div");
      row.className = "extra-row";
      const info = document.createElement("div");
      info.className = "extra-url";
      info.textContent = u;
      info.title = u;
      const btn = document.createElement("button");
      btn.className = "extra-dl";
      btn.textContent = "下载";
      btn.addEventListener("click", () => sendToDownloader(u, btn));
      row.appendChild(info);
      row.appendChild(btn);
      extraBody.appendChild(row);
    });
    extra.classList.add("show");
  };

  // 悬浮球：手动「重新识别」——清掉现有按钮整体重扫（自动扫描的兜底手段，
  // 比如主视频切换、动态播放器重建后想立刻刷新按钮位置/地址时用）
  fab.addEventListener("click", async () => {
    canceledAll = false; // 重新识别 → 恢复被「取消下载」隐藏的按钮
    fab.classList.add("active");
    status.textContent = "…";
    const ok = await engage(true);
    status.textContent = "";
    fab.classList.remove("active");
    if (ok) showTip("已重新识别 ✓", true);
  });

  // 全局「取消下载」：清空并锁定——当前页面所有下载按钮消失，自动扫描不再补，
  // 只有再点悬浮球识别才恢复。
  cancelAllBtn.addEventListener("click", () => {
    canceledAll = true;
    dispose();
    cancelAllBtn.disabled = true;
    showTip("已取消下载按钮，点 ▶ 重新识别", false);
  });

  // 有叠加按钮时点亮「取消下载」，全部清空后置灰
  const refreshCancelState = () => {
    cancelAllBtn.disabled = overlayBtns.length === 0;
  };
})();
