/**
 * 局域网 viewer 逻辑：画面渲染 + 鼠标/键盘/触摸 + 文件管理
 */

let ws = null;
let screenW = 0,
  screenH = 0;
let mouseMode = "absolute";
let connected = false;
let streaming = false;
let frameCount = 0;
let lastFpsTime = 0;
let canvas, ctx, touchLayer, screenWrap, zoomIndicator;
let pendingMove = null;
let moveTimer = null;

// 控制端操作列的三个按钮用不同 mode 打开本页：
//   full  = 完全控制；view = 仅查看；files = 仅文件传输（只读 + 自动展开文件面板）
const viewParams = new URLSearchParams(location.search);
const viewMode = viewParams.get("mode") || "full";
const readOnly = viewMode === "view" || viewMode === "files";
const autoFiles = viewMode === "files";

// 连接密码记忆：viewer 的源就是被控端自身（http://<ip>:<port>），localStorage
// 按源隔离，天然只作用于这一台被控端。完全控制/仅查看/文件传输是三个独立窗口，
// 靠它免去每开一个窗口就重输一次密码。
const PWD_STORAGE_KEY = "bwbrowser.viewer.pwd";
let attemptedPwd = "";

function readStoredPwd() {
  try {
    return localStorage.getItem(PWD_STORAGE_KEY) || "";
  } catch (e) {
    return "";
  }
}

function writeStoredPwd(pwd) {
  try {
    if (pwd) localStorage.setItem(PWD_STORAGE_KEY, pwd);
    else localStorage.removeItem(PWD_STORAGE_KEY);
  } catch (e) {
    /* 禁用 localStorage 时静默降级为每次手输 */
  }
}

// ==================== 视图变换 ====================
let baseScale = 1.0;
let userZoom = 1.0;
let panX = 0,
  panY = 0;
const ZOOM_MIN = 0.5;
const ZOOM_MAX = 5.0;
const LONG_PRESS_MS = 500;
const PAN_THRESHOLD = 8;

function getActualScale() {
  return baseScale * userZoom;
}

function calcFitScale() {
  if (!canvas.width || !screenWrap) return 1.0;
  const rect = screenWrap.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return 1.0;
  const sx = rect.width / canvas.width;
  const sy = rect.height / canvas.height;
  return Math.min(sx, sy);
}

function clampPan() {
  if (!screenWrap || !canvas.width) return;
  const rect = screenWrap.getBoundingClientRect();
  const s = getActualScale();
  const cw = canvas.width * s;
  const ch = canvas.height * s;
  if (cw <= rect.width) {
    panX = (rect.width - cw) / 2;
  } else {
    panX = Math.min(panX, 0);
    panX = Math.max(panX, rect.width - cw);
  }
  if (ch <= rect.height) {
    panY = (rect.height - ch) / 2;
  } else {
    panY = Math.min(panY, 0);
    panY = Math.max(panY, rect.height - ch);
  }
}

function updateCanvasTransform() {
  const s = getActualScale();
  canvas.style.transform = `translate(${panX}px, ${panY}px) scale(${s})`;
  showZoomIndicator();
}

function resetView() {
  baseScale = calcFitScale();
  userZoom = 1.0;
  panX = 0;
  panY = 0;
  clampPan();
  updateCanvasTransform();
}

function resetZoom() {
  userZoom = 1.0;
  clampPan();
  updateCanvasTransform();
}

let zoomIndicatorTimer = null;
function showZoomIndicator() {
  if (!zoomIndicator) return;
  const pct = Math.round((getActualScale() / baseScale) * 100);
  zoomIndicator.textContent = pct + "%";
  zoomIndicator.classList.add("show");
  if (zoomIndicatorTimer) clearTimeout(zoomIndicatorTimer);
  zoomIndicatorTimer = setTimeout(() => {
    zoomIndicator.classList.remove("show");
  }, 1500);
}

function zoomAtPoint(px, py, oldZoom, newZoom) {
  const oldScale = baseScale * oldZoom;
  const newScale = baseScale * newZoom;
  const cx = (px - panX) / oldScale;
  const cy = (py - panY) / oldScale;
  panX = px - cx * newScale;
  panY = py - cy * newScale;
}

// ==================== 初始化 ====================
window.onload = async () => {
  canvas = document.getElementById("remoteCanvas");
  ctx = canvas.getContext("2d");
  touchLayer = document.getElementById("touchLayer");
  screenWrap = document.getElementById("screenWrap");
  zoomIndicator = document.getElementById("zoomIndicator");

  try {
    const resp = await fetch("/info");
    const info = await resp.json();
    let html = "";
    if (info.name)
      html += `<div class="info-row"><span class="label">主机名</span><span class="value">${info.name}</span></div>`;
    if (info.os)
      html += `<div class="info-row"><span class="label">系统</span><span class="value">${info.os}</span></div>`;
    html += `<div class="info-row"><span class="label">分辨率</span><span class="value">${info.width}×${info.height}</span></div>`;
    const el = document.getElementById("serverInfo");
    if (el) el.innerHTML = html;
    screenW = info.width;
    screenH = info.height;
  } catch (e) {
    console.log("获取服务器信息失败", e);
  }

  document.getElementById("pwdInput").addEventListener("keydown", (e) => {
    if (e.key === "Enter") doConnect();
  });

  // 控制端把连接密码放在 URL（pwd）里带过来：直接鉴权，免手输；随后把 pwd 从
  // 地址栏抹掉，避免密码留在浏览器历史里。URL 没带时回退到上次记住的密码，
  // 这样重新打开窗口、或换一个模式（完全控制/仅查看）都不必再输。
  const urlPwd = viewParams.get("pwd");
  if (urlPwd) stripPwdFromUrl();
  const autoPwd = urlPwd || readStoredPwd();
  if (autoPwd) {
    document.getElementById("pwdInput").value = autoPwd;
    doConnect(autoPwd);
  } else {
    document.getElementById("pwdInput").focus();
  }

  window.addEventListener("resize", () => {
    if (canvas.width) {
      baseScale = calcFitScale();
      clampPan();
      updateCanvasTransform();
    }
  });
  window.addEventListener("orientationchange", () => {
    setTimeout(() => {
      if (canvas.width) {
        baseScale = calcFitScale();
        clampPan();
        updateCanvasTransform();
      }
    }, 200);
  });
};

// ==================== 连接 ====================
// 读走 pwd 后从地址栏移除，保留其余参数（如 mode）
function stripPwdFromUrl() {
  const p = new URLSearchParams(location.search);
  if (!p.has("pwd")) return;
  p.delete("pwd");
  const qs = p.toString();
  history.replaceState(
    null,
    "",
    location.pathname + (qs ? "?" + qs : "") + location.hash,
  );
}

function getWsUrl() {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${location.host}/ws`;
}

function doConnect(pwdOverride) {
  const input = document.getElementById("pwdInput");
  const pwd = (pwdOverride !== undefined ? pwdOverride : input.value).trim();
  if (!pwd) {
    showError("请输入密码");
    return;
  }
  attemptedPwd = pwd;
  const btn = document.getElementById("connectBtn");
  btn.disabled = true;
  btn.textContent = "连接中...";
  ws = new WebSocket(getWsUrl());
  ws.binaryType = "arraybuffer";
  ws.onopen = () => {
    ws.send(JSON.stringify({ type: "auth", password: pwd }));
  };
  ws.onmessage = (e) => {
    if (typeof e.data === "string") handleTextMessage(JSON.parse(e.data));
    else handleBinaryFrame(e.data);
  };
  ws.onclose = () => {
    // 连接断了画面不再更新：收尾录制，把已录到的部分落盘，别让用户白录
    if (isRecording()) stopRecording();
    if (connected) {
      showDesktop(false);
      showLogin(true);
      showError("连接已断开");
    }
    btn.disabled = false;
    btn.textContent = "连接";
    connected = false;
    streaming = false;
  };
  ws.onerror = () => {
    showError("连接失败，请检查网络");
    btn.disabled = false;
    btn.textContent = "连接";
  };
}

function handleTextMessage(msg) {
  if (msg.type && msg.type.indexOf("file_") === 0) {
    handleFileMessage(msg);
    return;
  }
  switch (msg.type) {
    case "auth_ok":
      connected = true;
      writeStoredPwd(attemptedPwd);
      screenW = msg.screen_width;
      screenH = msg.screen_height;
      document.getElementById("hostName").textContent =
        msg.hostname || "远程主机";
      document.getElementById("hostOs").textContent = msg.os || "";
      showLogin(false);
      showDesktop(true);
      applyViewMode();
      startHeartbeat();
      // 文件传输模式只看文件，不推流远程桌面
      if (!autoFiles) startStreaming();
      break;
    case "auth_fail":
      // 密码可能已被改过：清掉记忆，避免下次又自动拿旧密码撞失败
      writeStoredPwd("");
      showError(msg.message || "密码错误");
      ws.close();
      break;
    case "heartbeat_ack":
      break;
    case "error":
      console.error("服务器错误:", msg.message);
      break;
  }
}

// ==================== 画面渲染 ====================
function handleBinaryFrame(buffer) {
  if (buffer.byteLength < 12) return;
  const dv = new DataView(buffer);
  const w = dv.getUint32(0);
  const h = dv.getUint32(4);
  const len = dv.getUint32(8);
  const jpegData = buffer.slice(12, 12 + len);

  const blob = new Blob([jpegData], { type: "image/jpeg" });
  const url = URL.createObjectURL(blob);
  const img = new Image();
  img.onload = () => {
    if (canvas.width !== w) {
      canvas.width = w;
      canvas.height = h;
      baseScale = calcFitScale();
      userZoom = 1.0;
      panX = 0;
      panY = 0;
    }
    ctx.drawImage(img, 0, 0, w, h);
    URL.revokeObjectURL(url);
    clampPan();
    updateCanvasTransform();

    frameCount++;
    const now = performance.now();
    if (now - lastFpsTime > 1000) {
      const fps = Math.round((frameCount * 1000) / (now - lastFpsTime));
      const el = document.getElementById("connStatus");
      if (el) el.textContent = `已连接 ${fps}fps`;
      frameCount = 0;
      lastFpsTime = now;
    }
    const overlay = document.getElementById("loadingOverlay");
    if (overlay) overlay.style.display = "none";
  };
  img.onerror = () => {
    URL.revokeObjectURL(url);
  };
  img.src = url;
}

function startHeartbeat() {
  if (window._hbTimer) return;
  window._hbTimer = setInterval(() => {
    if (ws && ws.readyState === WebSocket.OPEN)
      ws.send(JSON.stringify({ type: "heartbeat" }));
  }, 15000);
}

function startStreaming() {
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  streaming = true;
  ws.send(JSON.stringify({ type: "start_stream" }));
  lastFpsTime = performance.now();
  frameCount = 0;
}

// ==================== 坐标转换 ====================
function getScreenCoords(clientX, clientY) {
  const rect = canvas.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0) return { x: 0, y: 0 };
  const scaleX = canvas.width / rect.width;
  const scaleY = canvas.height / rect.height;
  return {
    x: Math.round((clientX - rect.left) * scaleX),
    y: Math.round((clientY - rect.top) * scaleY),
  };
}

function sendMsg(obj) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(obj));
}

function sendMouseMove(x, y) {
  if (mouseMode === "relative") return;
  if (moveTimer) return;
  pendingMove = { x, y };
  moveTimer = setTimeout(() => {
    if (pendingMove)
      sendMsg({ type: "mouse_move", x: pendingMove.x, y: pendingMove.y });
    moveTimer = null;
    pendingMove = null;
  }, 8);
}

// ==================== 触摸手势 ====================
const gesture = {
  mode: "none",
  startX: 0,
  startY: 0,
  lastX: 0,
  lastY: 0,
  startTime: 0,
  moved: false,
  mouseDown: false,
  longPressTimer: null,
  initialDist: 0,
  initialZoom: 1.0,
  zoomMoved: false,
  centerX: 0,
  centerY: 0,
  panStartX: 0,
  panStartY: 0,
  panTouchStartX: 0,
  panTouchStartY: 0,
};

function getTouchDist(t1, t2) {
  const dx = t1.clientX - t2.clientX;
  const dy = t1.clientY - t2.clientY;
  return Math.sqrt(dx * dx + dy * dy);
}

function getWrapPoint(clientX, clientY) {
  const rect = screenWrap.getBoundingClientRect();
  return { x: clientX - rect.left, y: clientY - rect.top };
}

function initTouchEvents() {
  if (readOnly) return;
  const target = touchLayer;

  target.addEventListener(
    "touchstart",
    (e) => {
      e.preventDefault();
      if (e.touches.length === 1) {
        const t = e.touches[0];
        gesture.mode = "pending";
        gesture.startX = t.clientX;
        gesture.startY = t.clientY;
        gesture.lastX = t.clientX;
        gesture.lastY = t.clientY;
        gesture.startTime = Date.now();
        gesture.moved = false;
        gesture.mouseDown = false;
        gesture.zoomMoved = false;

        if (gesture.longPressTimer) clearTimeout(gesture.longPressTimer);
        gesture.longPressTimer = setTimeout(() => {
          if (gesture.mode === "pending" && !gesture.moved) {
            if (userZoom <= 1.01) {
              const { x, y } = getScreenCoords(t.clientX, t.clientY);
              sendMsg({ type: "mouse_move", x, y });
              sendMsg({ type: "mouse_down", button: "left" });
              gesture.mouseDown = true;
              gesture.mode = "mouse_drag";
            }
          }
        }, LONG_PRESS_MS);
      } else if (e.touches.length === 2) {
        if (gesture.longPressTimer) {
          clearTimeout(gesture.longPressTimer);
          gesture.longPressTimer = null;
        }
        if (gesture.mouseDown) {
          sendMsg({ type: "mouse_up", button: "left" });
          gesture.mouseDown = false;
        }

        const t1 = e.touches[0],
          t2 = e.touches[1];
        gesture.mode = "zoom";
        gesture.initialDist = getTouchDist(t1, t2);
        gesture.initialZoom = userZoom;
        gesture.zoomMoved = false;

        const wp = getWrapPoint(
          (t1.clientX + t2.clientX) / 2,
          (t1.clientY + t2.clientY) / 2,
        );
        gesture.centerX = wp.x;
        gesture.centerY = wp.y;
      }
    },
    { passive: false },
  );

  target.addEventListener(
    "touchmove",
    (e) => {
      e.preventDefault();
      if (e.touches.length === 1) {
        const t = e.touches[0];
        const dx = t.clientX - gesture.startX;
        const dy = t.clientY - gesture.startY;
        const dist = Math.sqrt(dx * dx + dy * dy);

        if (gesture.mode === "pending" && dist > PAN_THRESHOLD) {
          gesture.moved = true;
          if (gesture.longPressTimer) {
            clearTimeout(gesture.longPressTimer);
            gesture.longPressTimer = null;
          }
          if (userZoom > 1.01) {
            gesture.mode = "pan";
            gesture.panStartX = panX;
            gesture.panStartY = panY;
            gesture.panTouchStartX = t.clientX;
            gesture.panTouchStartY = t.clientY;
          } else {
            gesture.mode = "mouse_move";
            const { x, y } = getScreenCoords(t.clientX, t.clientY);
            sendMsg({ type: "mouse_move", x, y });
          }
        } else if (gesture.mode === "pan") {
          const moveDx = t.clientX - gesture.panTouchStartX;
          const moveDy = t.clientY - gesture.panTouchStartY;
          panX = gesture.panStartX + moveDx;
          panY = gesture.panStartY + moveDy;
          clampPan();
          updateCanvasTransform();
        } else if (gesture.mode === "mouse_move") {
          const { x, y } = getScreenCoords(t.clientX, t.clientY);
          sendMouseMove(x, y);
        } else if (gesture.mode === "mouse_drag") {
          const { x, y } = getScreenCoords(t.clientX, t.clientY);
          sendMsg({ type: "mouse_move", x, y });
        }
        gesture.lastX = t.clientX;
        gesture.lastY = t.clientY;
      } else if (e.touches.length === 2 && gesture.mode === "zoom") {
        const t1 = e.touches[0],
          t2 = e.touches[1];
        const dist = getTouchDist(t1, t2);
        const delta = Math.abs(dist - gesture.initialDist);
        if (delta > 5) gesture.zoomMoved = true;

        const ratio = dist / gesture.initialDist;
        let newZoom = gesture.initialZoom * ratio;
        newZoom = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, newZoom));

        const center = getWrapPoint(
          (t1.clientX + t2.clientX) / 2,
          (t1.clientY + t2.clientY) / 2,
        );
        zoomAtPoint(center.x, center.y, gesture.initialZoom, newZoom);
        userZoom = newZoom;
        clampPan();
        updateCanvasTransform();
      }
    },
    { passive: false },
  );

  target.addEventListener(
    "touchend",
    (e) => {
      e.preventDefault();
      if (gesture.longPressTimer) {
        clearTimeout(gesture.longPressTimer);
        gesture.longPressTimer = null;
      }

      if (gesture.mode === "pending" && !gesture.moved) {
        const elapsed = Date.now() - gesture.startTime;
        const { x, y } = getScreenCoords(gesture.startX, gesture.startY);
        if (elapsed < LONG_PRESS_MS) {
          sendMsg({ type: "mouse_move", x, y });
          sendMsg({ type: "mouse_click", button: "left" });
        } else {
          sendMsg({ type: "mouse_move", x, y });
          sendMsg({ type: "mouse_click", button: "right" });
        }
      } else if (gesture.mode === "mouse_drag") {
        if (gesture.mouseDown) {
          const { x, y } = getScreenCoords(gesture.lastX, gesture.lastY);
          sendMsg({ type: "mouse_move", x, y });
          sendMsg({ type: "mouse_up", button: "left" });
        }
      } else if (gesture.mode === "zoom" && !gesture.zoomMoved) {
        const { x, y } = getScreenCoords(
          gesture.centerX + screenWrap.getBoundingClientRect().left,
          gesture.centerY + screenWrap.getBoundingClientRect().top,
        );
        sendMsg({ type: "mouse_move", x, y });
        sendMsg({ type: "mouse_click", button: "right" });
      }
      gesture.mode = "none";
      gesture.mouseDown = false;
    },
    { passive: false },
  );

  document.addEventListener("gesturestart", (e) => {
    e.preventDefault();
  });
}

// ==================== 鼠标事件（PC） ====================
function initMouseEvents() {
  if (readOnly) return;
  const target = canvas;
  target.addEventListener("mousemove", (e) => {
    const { x, y } = getScreenCoords(e.clientX, e.clientY);
    sendMouseMove(x, y);
  });
  target.addEventListener("mousedown", (e) => {
    const { x, y } = getScreenCoords(e.clientX, e.clientY);
    const btn = ["left", "middle", "right"][e.button] || "left";
    sendMsg({ type: "mouse_move", x, y });
    sendMsg({ type: "mouse_down", button: btn });
  });
  target.addEventListener("mouseup", (e) => {
    const { x, y } = getScreenCoords(e.clientX, e.clientY);
    const btn = ["left", "middle", "right"][e.button] || "left";
    sendMsg({ type: "mouse_move", x, y });
    sendMsg({ type: "mouse_up", button: btn });
  });
  target.addEventListener("contextmenu", (e) => {
    e.preventDefault();
  });
  target.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      if (e.ctrlKey) {
        const wp = getWrapPoint(e.clientX, e.clientY);
        const oldZoom = userZoom;
        const delta = e.deltaY > 0 ? 0.9 : 1.1;
        let newZoom = oldZoom * delta;
        newZoom = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, newZoom));
        zoomAtPoint(wp.x, wp.y, oldZoom, newZoom);
        userZoom = newZoom;
        clampPan();
        updateCanvasTransform();
      } else {
        sendMsg({
          type: "mouse_scroll",
          dx: Math.round(e.deltaX / 50),
          dy: Math.round(e.deltaY / 50),
        });
      }
    },
    { passive: false },
  );

  let lastTap = 0;
  target.addEventListener("click", (e) => {
    const now = Date.now();
    if (now - lastTap < 350) {
      const wp = getWrapPoint(e.clientX, e.clientY);
      if (userZoom > 1.01) {
        userZoom = 1.0;
      } else {
        userZoom = 2.0;
        zoomAtPoint(wp.x, wp.y, 1.0, 2.0);
      }
      clampPan();
      updateCanvasTransform();
    }
    lastTap = now;
  });
}

// ==================== 键盘 ====================
function initKeyboardEvents() {
  if (readOnly) return;
  document.addEventListener("keydown", (e) => {
    if (!connected) return;
    if (filePanelOpen()) return;
    const key = mapKey(e.key);
    if (key) {
      e.preventDefault();
      sendMsg({ type: "key_down", key: key });
    }
  });
  document.addEventListener("keyup", (e) => {
    if (!connected) return;
    if (filePanelOpen()) return;
    const key = mapKey(e.key);
    if (key) {
      e.preventDefault();
      sendMsg({ type: "key_up", key: key });
    }
  });
}

function mapKey(k) {
  const map = {
    Enter: "enter",
    Return: "enter",
    " ": "space",
    Tab: "tab",
    Backspace: "backspace",
    Delete: "delete",
    Escape: "esc",
    Shift: "shift",
    Control: "ctrl",
    Alt: "alt",
    Meta: "cmd",
    OS: "cmd",
    ArrowUp: "up",
    ArrowDown: "down",
    ArrowLeft: "left",
    ArrowRight: "right",
    Home: "home",
    End: "end",
    PageUp: "page_up",
    PageDown: "page_down",
    CapsLock: "caps_lock",
  };
  if (map[k]) return map[k];
  const fMatch = k.match(/^F(\d+)$/);
  if (fMatch) return "f" + fMatch[1];
  if (k.length === 1) return k;
  return null;
}

// ==================== UI 辅助 ====================
function showLogin(show) {
  document.getElementById("loginPage").style.display = show ? "flex" : "none";
}
function showDesktop(show) {
  const page = document.getElementById("desktopPage");
  page.classList.toggle("active", show);
  if (show && !window._inputInit) {
    initTouchEvents();
    initMouseEvents();
    initKeyboardEvents();
    initInputPanel();
    initTrackpad();
    initFilePanel();
    window._inputInit = true;
    setTimeout(() => {
      resetView();
    }, 100);
  }
}
function showError(msg) {
  const el = document.getElementById("errorMsg");
  if (el) el.textContent = msg;
}
function disconnect() {
  if (ws) {
    sendMsg({ type: "stop_stream" });
    ws.close();
  }
}

// 连接成功后按 URL 参数落到对应模式：只读模式告知被控端拒绝输入，文件模式再展开文件面板
function applyViewMode() {
  if (viewMode === "full") return;
  document.body.classList.add("readonly");
  sendMsg({ type: "set_mode", mode: "view" });
  const badge = document.getElementById("modeBadge");
  if (badge) {
    badge.textContent = autoFiles ? "文件传输" : "仅查看";
    badge.style.display = "inline-block";
  }
  if (autoFiles) {
    document.body.classList.add("files-mode");
    const panel = document.getElementById("filePanel");
    if (panel && !panel.classList.contains("active")) toggleFiles();
  }
}

function toggleFullscreen() {
  if (!document.fullscreenElement) document.documentElement.requestFullscreen();
  else document.exitFullscreen();
}

async function toggleLandscape() {
  try {
    if (!document.fullscreenElement) {
      await document.documentElement.requestFullscreen();
      if (screen.orientation && screen.orientation.lock) {
        try {
          await screen.orientation.lock("landscape");
        } catch (e) {
          console.log("无法锁定横屏，请手动旋转设备");
        }
      }
      setTimeout(() => {
        resetView();
      }, 300);
    } else {
      if (screen.orientation && screen.orientation.unlock)
        screen.orientation.unlock();
      await document.exitFullscreen();
      setTimeout(() => {
        resetView();
      }, 300);
    }
  } catch (e) {
    console.error("横屏切换失败:", e);
  }
}

function toggleSettings() {
  document.getElementById("settingsPanel").classList.toggle("active");
}
function onQualityChange(val) {
  document.getElementById("qualityVal").textContent = val;
  sendMsg({ type: "set_quality", quality: parseInt(val) });
}
function onFpsChange(val) {
  document.getElementById("fpsVal").textContent = val;
  sendMsg({ type: "set_fps", fps: parseInt(val) });
}
function onScaleChange(val) {
  const pct = parseInt(val);
  document.getElementById("scaleVal").textContent = pct + "%";
  sendMsg({ type: "set_scale", scale: pct / 100 });
}
function onMouseModeChange(val) {
  mouseMode = val;
}
function sendKey(key) {
  sendMsg({ type: "key_down", key: key });
  setTimeout(() => {
    sendMsg({ type: "key_up", key: key });
  }, 50);
}

// ==================== 键盘输入面板 ====================
let isComposing = false;

function initInputPanel() {
  const input = document.getElementById("textInput");
  if (!input) return;

  input.addEventListener("compositionstart", (e) => {
    isComposing = true;
  });

  input.addEventListener("compositionend", (e) => {
    isComposing = false;
    if (e.data) {
      sendMsg({ type: "type_text", text: e.data });
    }
    input.value = "";
  });

  input.addEventListener("input", (e) => {
    if (isComposing || e.isComposing) return;
    if (e.data) {
      sendMsg({ type: "type_text", text: e.data });
      input.value = "";
    }
  });

  input.addEventListener("keydown", (e) => {
    if (e.isComposing || isComposing) return;
    if (e.key === "Enter") {
      e.preventDefault();
      sendKey("enter");
    } else if (e.key === "Backspace" && input.value === "") {
      e.preventDefault();
      sendKey("backspace");
    } else if (e.key === "Tab") {
      e.preventDefault();
      sendKey("tab");
    } else if (e.key === "Escape") {
      e.preventDefault();
      closeInputPanel();
    }
  });
}

function openInputPanel() {
  document.getElementById("inputPanel").classList.add("active");
  setTimeout(() => {
    document.getElementById("textInput").focus();
  }, 300);
}

function closeInputPanel() {
  const panel = document.getElementById("inputPanel");
  panel.classList.remove("active");
  const input = document.getElementById("textInput");
  if (input) input.blur();
}

function sendInputText() {
  const input = document.getElementById("textInput");
  if (!input) return;
  const text = input.value;
  if (text) {
    sendMsg({ type: "type_text", text: text });
    input.value = "";
  }
}

// ==================== 触控板模式 ====================
let trackpadActive = false;
const TRACKPAD_SENSITIVITY = 1.8;

function toggleTrackpad() {
  trackpadActive = !trackpadActive;
  const overlay = document.getElementById("trackpadOverlay");
  const btn = document.getElementById("btnTrackpad");
  const mobileBtn = document.getElementById("mobileTrackpad");
  overlay.classList.toggle("active", trackpadActive);
  if (btn) btn.classList.toggle("active", trackpadActive);
  if (mobileBtn) mobileBtn.classList.toggle("active", trackpadActive);
  if (mobileBtn) mobileBtn.textContent = trackpadActive ? "触控(开)" : "触控板";
}

function initTrackpad() {
  const area = document.getElementById("trackpadArea");
  if (!area) return;

  let lastX = 0,
    lastY = 0;
  let twoFinger = false;
  let lastScrollY = 0;

  area.addEventListener(
    "touchstart",
    (e) => {
      e.preventDefault();
      if (e.touches.length === 1) {
        twoFinger = false;
        lastX = e.touches[0].clientX;
        lastY = e.touches[0].clientY;
      } else if (e.touches.length === 2) {
        twoFinger = true;
        lastScrollY = (e.touches[0].clientY + e.touches[1].clientY) / 2;
      }
    },
    { passive: false },
  );

  area.addEventListener(
    "touchmove",
    (e) => {
      e.preventDefault();
      if (e.touches.length === 1 && !twoFinger) {
        const t = e.touches[0];
        const dx = Math.round((t.clientX - lastX) * TRACKPAD_SENSITIVITY);
        const dy = Math.round((t.clientY - lastY) * TRACKPAD_SENSITIVITY);
        if (dx !== 0 || dy !== 0) {
          sendMsg({ type: "mouse_move_relative", dx: dx, dy: dy });
        }
        lastX = t.clientX;
        lastY = t.clientY;
      } else if (e.touches.length === 2 && twoFinger) {
        const cy = (e.touches[0].clientY + e.touches[1].clientY) / 2;
        const scrollDy = Math.round((cy - lastScrollY) / 3);
        if (scrollDy !== 0) {
          sendMsg({ type: "mouse_scroll", dx: 0, dy: scrollDy });
          lastScrollY = cy;
        }
      }
    },
    { passive: false },
  );

  area.addEventListener(
    "touchend",
    (e) => {
      e.preventDefault();
      if (e.touches.length === 0) twoFinger = false;
    },
    { passive: false },
  );

  const btnMap = [
    { id: "trackpadLeft", button: "left" },
    { id: "trackpadMiddle", button: "middle" },
    { id: "trackpadRight", button: "right" },
  ];
  btnMap.forEach((item) => {
    const btn = document.getElementById(item.id);
    if (!btn) return;

    function pressDown(e) {
      e.preventDefault();
      btn.classList.add("pressed");
      sendMsg({ type: "mouse_down", button: item.button });
    }
    function pressUp(e) {
      e.preventDefault();
      btn.classList.remove("pressed");
      sendMsg({ type: "mouse_up", button: item.button });
    }

    btn.addEventListener("touchstart", pressDown, { passive: false });
    btn.addEventListener("touchend", pressUp, { passive: false });
    btn.addEventListener("touchcancel", pressUp, { passive: false });
    btn.addEventListener("mousedown", pressDown);
    btn.addEventListener("mouseup", pressUp);
    btn.addEventListener("mouseleave", (e) => {
      if (btn.classList.contains("pressed")) pressUp(e);
    });
  });
}

// ==================== 修饰键切换 ====================
const modifiers = { ctrl: false, alt: false, shift: false };

function toggleModifier(key) {
  modifiers[key] = !modifiers[key];
  const capKey = key.charAt(0).toUpperCase() + key.slice(1);
  const btn = document.getElementById("mobile" + capKey);
  if (btn) btn.classList.toggle("active", modifiers[key]);
  if (modifiers[key]) {
    sendMsg({ type: "key_down", key: key });
  } else {
    sendMsg({ type: "key_up", key: key });
  }
}

// ==================== 录屏 ====================
// 录的是 viewer 画布上的远程画面：MediaRecorder 直接对 canvas.captureStream()
// 编码，不依赖被控端，也不需要额外解码库。保存位置按可用能力逐级降级：
//   1) 应用内窗口（Tauri）：原生「另存为」对话框，选中后边录边写入所选文件；
//   2) 浏览器安全上下文：File System Access API 的 showSaveFilePicker；
//   3) 其余情况（http://<局域网IP> 既不是安全上下文、也没有 IPC）：分块缓存在
//      内存，录制结束后由浏览器下载。
// viewer 是远程源（被控端自己的 http 页面），Tauri 仍会注入 IPC，能调哪些命令
// 由 capabilities/remote-viewer.json 授权。
let mediaRecorder = null;
let recChunks = [];
let recWritable = null;
let recWriteQueue = Promise.resolve();
let recMime = "";
let recFileName = "";
let recFilePath = "";
let recFileOk = false;
let recWroteAny = false;
let recStarting = false;
let recStartAt = 0;
let recTimer = null;
let recBytes = 0;
let recSizeWarned = false;

// 只能缓存在内存时，长录制会吃光内存；到上限就自动收尾
const REC_MAX_BYTES = 1536 * 1024 * 1024;

function pickRecordingMime() {
  if (typeof MediaRecorder === "undefined") return "";
  const candidates = [
    "video/webm;codecs=vp9",
    "video/webm;codecs=vp8",
    "video/webm",
    "video/mp4",
  ];
  for (const m of candidates) {
    if (MediaRecorder.isTypeSupported(m)) return m;
  }
  return "";
}

function recordingFileName(ext) {
  const host = (
    document.getElementById("hostName").textContent || "remote"
  ).trim();
  const d = new Date();
  const pad = (n) => String(n).padStart(2, "0");
  const stamp =
    d.getFullYear() +
    pad(d.getMonth() + 1) +
    pad(d.getDate()) +
    "-" +
    pad(d.getHours()) +
    pad(d.getMinutes()) +
    pad(d.getSeconds());
  return host.replace(/[\\/:*?"<>|]/g, "_") + "-" + stamp + "." + ext;
}

// 应用内窗口里被控端页面是远程源，Tauri 会把 IPC 注进来；命令是否放行由
// capabilities/remote-viewer.json 决定，不在授权范围时 invoke 会直接 reject。
function tauriInvoke() {
  const internals = window.__TAURI_INTERNALS__;
  return internals && typeof internals.invoke === "function"
    ? internals.invoke.bind(internals)
    : null;
}

// 选保存位置。返回 null 表示用户取消；其余返回 { kind, ... }：
//   tauri   -> { path }      边录边写入该文件
//   fsapi   -> { writable }  边录边写入浏览器文件句柄
//   download -> {}           无可用落盘方式，录制结束后下载
async function pickRecordingTarget(name, ext) {
  const inv = tauriInvoke();
  if (inv) {
    try {
      const path = await inv("plugin:dialog|save", {
        options: {
          title: "选择录屏保存位置",
          defaultPath: name,
          filters: [
            ext === "mp4"
              ? { name: "MP4 视频", extensions: ["mp4"] }
              : { name: "WebM 视频", extensions: ["webm"] },
          ],
        },
      });
      if (!path) return null;
      return { kind: "tauri", path };
    } catch (e) {
      console.warn("原生另存为对话框不可用，改用浏览器保存", e);
    }
  }
  if (window.showSaveFilePicker) {
    try {
      const handle = await window.showSaveFilePicker({
        suggestedName: name,
        types: [
          ext === "mp4"
            ? { description: "MP4 视频", accept: { "video/mp4": [".mp4"] } }
            : { description: "WebM 视频", accept: { "video/webm": [".webm"] } },
        ],
      });
      return { kind: "fsapi", writable: await handle.createWritable() };
    } catch (e) {
      if (e && e.name === "AbortError") return null;
      console.warn("浏览器另存为不可用，录制结束后下载", e);
    }
  }
  return { kind: "download" };
}

// 把一帧分块写进原生对话框选定的文件。首块截断覆盖，之后追加；
// 路径由 dialog 插件登记进 fs scope，所以这里能直接写。
async function writeRecordingChunk(path, blob, append) {
  const inv = tauriInvoke();
  if (!inv) throw new Error("Tauri IPC unavailable");
  const buf = new Uint8Array(await blob.arrayBuffer());
  await inv("plugin:fs|write_file", buf, {
    headers: {
      path: encodeURIComponent(path),
      options: JSON.stringify({ append: !!append, create: true }),
    },
  });
}

// 分块落盘一律串到同一条队列上：写是异步的，并发调用会互相覆盖。写失败的判定
// 也放在队列里按序执行，这样一旦某块写失败，之后的分块就连续缓存在内存里，
// 录制结束后再整段补写到文件末尾，顺序不会乱。
function enqueueRecordingChunk(chunk) {
  recWriteQueue = recWriteQueue.then(async () => {
    if (recFilePath && recFileOk) {
      try {
        await writeRecordingChunk(recFilePath, chunk, recWroteAny);
        recWroteAny = true;
        return;
      } catch (err) {
        recFileOk = false;
        console.warn("录屏写入文件失败，改为内存缓存", err);
      }
    }
    if (recWritable) {
      try {
        await recWritable.write(chunk);
        return;
      } catch (err) {
        recWritable = null;
        console.warn("录屏写入文件失败，改为内存缓存", err);
      }
    }
    recChunks.push(chunk);
    if (!recSizeWarned && recBytes >= REC_MAX_BYTES) {
      recSizeWarned = true;
      alert(
        "录制已达 1.5 GB，自动停止。当前环境无法边录边写文件，只能缓存在内存中。",
      );
      stopRecording();
    }
  });
}

function isRecording() {
  return mediaRecorder !== null && mediaRecorder.state !== "inactive";
}

function toggleRecording() {
  if (isRecording()) stopRecording();
  else startRecording();
}

async function startRecording() {
  if (recStarting || isRecording()) return;
  if (!connected || !canvas || !canvas.width) {
    alert("尚未连接远程桌面，无法开始录制");
    return;
  }
  const mime = pickRecordingMime();
  if (!mime) {
    alert("当前浏览器不支持录屏");
    return;
  }
  const ext = mime.indexOf("mp4") >= 0 ? "mp4" : "webm";
  recFileName = recordingFileName(ext);

  // 先选保存位置，选定后才开始录制
  recStarting = true;
  let target;
  try {
    target = await pickRecordingTarget(recFileName, ext);
  } catch (e) {
    alert("无法选择保存位置：" + (e && e.message ? e.message : e));
    return;
  } finally {
    recStarting = false;
  }
  if (!target) return; // 用户取消了保存位置选择

  recMime = mime;
  recChunks = [];
  recBytes = 0;
  recSizeWarned = false;
  recWriteQueue = Promise.resolve();
  recWroteAny = false;
  recWritable = target.kind === "fsapi" ? target.writable : null;
  recFilePath = target.kind === "tauri" ? target.path : "";
  recFileOk = target.kind === "tauri";

  let stream;
  try {
    stream = canvas.captureStream(30);
  } catch (e) {
    alert("无法捕获远程画面：" + (e && e.message ? e.message : e));
    return;
  }

  try {
    mediaRecorder = new MediaRecorder(stream, {
      mimeType: mime,
      videoBitsPerSecond: 6000000,
    });
  } catch (e) {
    stream.getTracks().forEach((t) => t.stop());
    alert("无法启动录制：" + (e && e.message ? e.message : e));
    return;
  }

  mediaRecorder.ondataavailable = (e) => {
    if (!e.data || !e.data.size) return;
    recBytes += e.data.size;
    enqueueRecordingChunk(e.data);
  };
  mediaRecorder.onstop = finishRecording;
  mediaRecorder.onerror = (e) => {
    console.error("录制出错", e.error || e);
  };

  mediaRecorder.start(1000);
  recStartAt = Date.now();
  setRecordingUI(true);
  recTimer = setInterval(updateRecordingBadge, 1000);
  updateRecordingBadge();
}

function stopRecording() {
  if (isRecording()) mediaRecorder.stop();
}

async function finishRecording() {
  if (recTimer) {
    clearInterval(recTimer);
    recTimer = null;
  }
  const recorder = mediaRecorder;
  mediaRecorder = null;
  if (recorder && recorder.stream) {
    recorder.stream.getTracks().forEach((t) => t.stop());
  }
  setRecordingUI(false);

  const path = recFilePath;
  const writable = recWritable;
  recFilePath = "";
  recWritable = null;
  recFileOk = false;

  // 队列内部已吞掉各自的写失败，这里不会被 reject
  await recWriteQueue;

  // 写入中途失败时，内存里留下的是连续的一段尾部，补写到文件末尾即可接上
  if (path && recChunks.length) {
    const tail = recChunks;
    recChunks = [];
    try {
      await writeRecordingChunk(path, new Blob(tail, { type: recMime }), true);
    } catch (e) {
      recChunks = tail;
      console.warn("补写录屏尾部失败，改为下载保存", e);
    }
  }
  if (writable) {
    try {
      await writable.close();
    } catch (e) {
      console.warn("关闭录屏文件失败", e);
    }
  }

  if (!recChunks.length) return;
  // 兜底：没有可落盘的目标，或边录边写失败，交给浏览器下载
  const blob = new Blob(recChunks, { type: recMime });
  recChunks = [];
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = recFileName;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 4000);
}

function setRecordingUI(on) {
  const btn = document.getElementById("btnRecord");
  if (btn) {
    btn.classList.toggle("recording", on);
    btn.textContent = on ? "停止" : "录制";
  }
  const badge = document.getElementById("recBadge");
  if (badge) badge.classList.toggle("active", on);
}

function updateRecordingBadge() {
  const badge = document.getElementById("recBadge");
  if (!badge) return;
  const total = Math.floor((Date.now() - recStartAt) / 1000);
  const mm = String(Math.floor(total / 60)).padStart(2, "0");
  const ss = String(total % 60).padStart(2, "0");
  const toDownload = !recFileOk && !recWritable;
  badge.textContent =
    "● 录制中 " +
    mm +
    ":" +
    ss +
    " · " +
    formatSize(recBytes) +
    (toDownload ? "（完成后下载）" : "");
}

// ==================== 文件管理 ====================
let filePath = "";
let fileParent = "";
let fileEntries = [];
let fileSelected = -1;
let fileDownloadBuf = null;
let fileDownloadName = "";
let fileUploadAck = null;

function filePanelOpen() {
  const p = document.getElementById("filePanel");
  return p && p.classList.contains("active");
}

function toggleFiles() {
  const panel = document.getElementById("filePanel");
  const btn = document.getElementById("btnFiles");
  const open = !panel.classList.contains("active");
  panel.classList.toggle("active", open);
  if (btn) btn.classList.toggle("active", open);
  if (open) fileRefresh();
}

function initFilePanel() {
  const input = document.getElementById("fileUploadInput");
  if (input)
    input.addEventListener("change", () => {
      fileUpload(input.files[0]);
      input.value = "";
    });
}

function fileRefresh() {
  sendMsg({ type: "file_list", path: filePath });
}
function fileGoUp() {
  sendMsg({ type: "file_list", path: fileParent || "" });
}

function escapeHtml(s) {
  return String(s).replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
}

function formatSize(n) {
  if (!n) return "";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0,
    v = n;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return (i === 0 ? v : v.toFixed(1)) + u[i];
}

function renderFileList() {
  const box = document.getElementById("fileList");
  const pathEl = document.getElementById("filePath");
  if (pathEl) {
    const label =
      filePath ||
      (fileEntries.length && fileEntries[0].name.match(/^[A-Z]:\\$/)
        ? "此电脑"
        : "/");
    pathEl.textContent = label;
    pathEl.title = label;
  }
  const upBtn = document.getElementById("fileUpBtn");
  if (upBtn)
    upBtn.disabled =
      !filePath && fileEntries.every((e) => !/^[A-Z]:\\$/.test(e.name));

  if (!fileEntries.length) {
    box.innerHTML = '<div class="file-empty">空目录</div>';
    return;
  }
  box.innerHTML = "";
  fileEntries.forEach((item, idx) => {
    const row = document.createElement("div");
    row.className = "file-row" + (idx === fileSelected ? " selected" : "");
    row.innerHTML =
      '<span class="ico">' +
      (item.is_dir ? "📁" : "📄") +
      "</span>" +
      '<span class="fname">' +
      escapeHtml(item.name) +
      "</span>" +
      '<span class="fsize">' +
      (item.is_dir ? "" : formatSize(item.size)) +
      "</span>";
    row.onclick = () => {
      fileSelect(idx);
    };
    row.ondblclick = () => {
      if (item.is_dir) {
        filePath = item.name.match(/^[A-Z]:\\$/)
          ? item.name
          : filePath
            ? filePath.replace(/[\\/]+$/, "") + "/" + item.name
            : item.name;
        fileRefresh();
      } else {
        fileDownload(item.name);
      }
    };
    box.appendChild(row);
  });
}

function fileSelect(idx) {
  fileSelected = idx;
  const item = fileEntries[idx];
  const isDir = item && item.is_dir;
  ["fileDownloadBtn", "fileRenameBtn", "fileDeleteBtn"].forEach((id) => {
    const b = document.getElementById(id);
    if (b) b.disabled = !item || isDir;
  });
  Array.prototype.forEach.call(
    document.querySelectorAll(".file-row"),
    (r, i) => {
      r.classList.toggle("selected", i === idx);
    },
  );
}

function fileJoin(name) {
  if (!filePath) return name;
  return filePath.replace(/[\\/]+$/, "") + "/" + name;
}

function fileMkdir() {
  const name = prompt("新建文件夹名称");
  if (!name) return;
  sendMsg({ type: "file_mkdir", path: filePath, name: name });
}

function fileRenameSelected() {
  const item = fileEntries[fileSelected];
  if (!item) return;
  const name = prompt("重命名为", item.name);
  if (!name || name === item.name) return;
  sendMsg({ type: "file_rename", path: fileJoin(item.name), new_name: name });
}

function fileDeleteSelected() {
  const item = fileEntries[fileSelected];
  if (!item) return;
  if (
    !confirm(
      "确定删除「" +
        item.name +
        "」？" +
        (item.is_dir ? "（含目录内全部内容）" : ""),
    )
  )
    return;
  sendMsg({ type: "file_delete", path: fileJoin(item.name) });
}

function fileDownloadSelected() {
  const item = fileEntries[fileSelected];
  if (!item || item.is_dir) return;
  fileDownload(item.name);
}

function fileDownload(name) {
  fileDownloadBuf = [];
  fileDownloadName = name;
  sendMsg({ type: "file_download", path: fileJoin(name) });
}

function filePickUpload() {
  const input = document.getElementById("fileUploadInput");
  if (input) input.click();
}

function b64FromBuffer(buf) {
  const bytes = new Uint8Array(buf);
  let bin = "";
  const STEP = 0x8000;
  for (let i = 0; i < bytes.length; i += STEP) {
    bin += String.fromCharCode.apply(null, bytes.subarray(i, i + STEP));
  }
  return btoa(bin);
}

async function fileUpload(file) {
  if (!file) return;
  if (!filePath) {
    alert("请先进入目标文件夹");
    return;
  }
  const CHUNK = 65536;
  sendMsg({
    type: "file_upload_start",
    path: filePath,
    name: file.name,
    size: file.size,
  });
  const ready = await waitUploadReady();
  if (!ready) {
    alert("上传初始化失败");
    return;
  }

  let offset = 0;
  showFileProgress("上传 " + file.name, 0);
  while (offset < file.size) {
    const slice = file.slice(offset, offset + CHUNK);
    const buf = await slice.arrayBuffer();
    sendMsg({ type: "file_upload_chunk", data: b64FromBuffer(buf) });
    offset += buf.byteLength;
    showFileProgress(
      "上传 " + file.name,
      Math.round((offset * 100) / file.size),
    );
    const ok = await waitUploadAck(offset);
    if (!ok) {
      alert("上传中断");
      hideFileProgress();
      return;
    }
  }
  sendMsg({ type: "file_upload_end" });
}

function waitUploadReady() {
  return new Promise((resolve) => {
    fileUploadAck = { kind: "ready", resolve: resolve };
    setTimeout(() => {
      if (fileUploadAck && fileUploadAck.kind === "ready") {
        fileUploadAck = null;
        resolve(false);
      }
    }, 5000);
  });
}

function waitUploadAck(received) {
  return new Promise((resolve) => {
    fileUploadAck = { kind: "progress", received: received, resolve: resolve };
    setTimeout(() => {
      if (fileUploadAck && fileUploadAck.kind === "progress") {
        fileUploadAck = null;
        resolve(true);
      }
    }, 5000);
  });
}

function showFileProgress(text, pct) {
  const el = document.getElementById("fileProgress");
  if (!el) return;
  el.classList.add("active");
  document.getElementById("fileProgressText").textContent =
    text + " " + pct + "%";
  document.getElementById("fileProgressBar").style.width = pct + "%";
}

function hideFileProgress() {
  const el = document.getElementById("fileProgress");
  if (el)
    setTimeout(() => {
      el.classList.remove("active");
    }, 800);
}

function handleFileMessage(msg) {
  switch (msg.type) {
    case "file_list_result":
      if (!msg.success) {
        alert(msg.message || "读取目录失败");
        return;
      }
      fileEntries = msg.entries || [];
      filePath = msg.path || "";
      fileParent = msg.parent || "";
      fileSelected = -1;
      ["fileDownloadBtn", "fileRenameBtn", "fileDeleteBtn"].forEach((id) => {
        const b = document.getElementById(id);
        if (b) b.disabled = true;
      });
      renderFileList();
      break;
    case "file_download_start":
      fileDownloadBuf = [];
      fileDownloadName = msg.name || "download";
      break;
    case "file_download_chunk":
      if (fileDownloadBuf) fileDownloadBuf.push(msg.data);
      break;
    case "file_download_end": {
      if (!fileDownloadBuf) break;
      const parts = fileDownloadBuf.map((b64) => {
        const bin = atob(b64);
        const arr = new Uint8Array(bin.length);
        for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
        return arr;
      });
      const blob = new Blob(parts);
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = fileDownloadName;
      document.body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(() => {
        URL.revokeObjectURL(url);
      }, 2000);
      fileDownloadBuf = null;
      break;
    }
    case "file_download_error":
      fileDownloadBuf = null;
      alert(msg.message || "下载失败");
      break;
    case "file_upload_ready":
      if (fileUploadAck && fileUploadAck.kind === "ready") {
        const r = fileUploadAck.resolve;
        fileUploadAck = null;
        r(!!msg.success);
      } else if (!msg.success) {
        alert(msg.message || "上传初始化失败");
      }
      break;
    case "file_upload_progress":
      if (fileUploadAck && fileUploadAck.kind === "progress") {
        const r = fileUploadAck.resolve;
        fileUploadAck = null;
        r(true);
      }
      break;
    case "file_upload_result":
      hideFileProgress();
      if (msg.success) fileRefresh();
      else alert(msg.message || "上传失败");
      break;
    case "file_op_result":
      if (msg.success) fileRefresh();
      else alert(msg.message || "操作失败");
      break;
  }
}
