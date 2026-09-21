# Simprint 项目浏览器内核管理与指纹注入机制研究报告

## 一、项目架构概览

Simprint 是一个基于 Tauri 2.x 的指纹浏览器项目，采用前后端分离架构：

- **前端**: Next.js + React（`src/` 目录），插件化架构（`plugins/` 目录）
- **后端**: Rust + Tauri（`src-tauri/src/` 目录）
- **业务层**: `src-tauri/crates/business/`（SQLite 数据库 + 业务逻辑）
- **运行时层**: `src-tauri/crates/runtime/`（内嵌浏览器运行时，负责实际启动浏览器）
- **数据库**: SQLite，迁移文件位于 `src-tauri/crates/business/migrations/`

---

## 二、浏览器内核的启动与管理

### 2.1 核心文件路径

| 层级 | 文件路径 | 职责 |
|------|----------|------|
| Tauri 命令层 | `src-tauri/src/commands/environment.rs` | 暴露 Tauri 命令给前端 |
| 服务层（主进程） | `src-tauri/src/services/environment/kernel/mod.rs` | `KernelService` - 内核准备与启动服务入口 |
| 运行时桥接 | `src-tauri/src/services/environment/kernel/runtime_bridge.rs` | 主进程与内嵌 runtime 之间的桥接 |
| 运行时管理器 | `src-tauri/src/app/runtime.rs` | `SimprintRuntimeManager` - 管理内嵌 runtime 生命周期 |
| 运行时内核（实际启动） | `src-tauri/crates/runtime/src/services/environment/kernel/launcher.rs` | `launch_browser()` - 真正启动浏览器进程 |
| 运行时内核服务 | `src-tauri/crates/runtime/src/services/environment/kernel/mod.rs` | `KernelRuntime` - runtime 内的内核服务分发 |
| 内核下载器 | `src-tauri/src/services/environment/kernel/downloader.rs` | 内核下载与解压 |
| 内核校验器 | `src-tauri/src/services/environment/kernel/verifier.rs` | chrome.dll 签名校验 |
| 内核工具 | `src-tauri/src/services/environment/kernel/utils.rs` | 路径解析、哈希计算、布局校验 |

### 2.2 内核管理核心逻辑

**KernelService::ensure_kernel_ready**（`src-tauri/src/services/environment/kernel/mod.rs`）:

1. 加锁（`state::acquire_kernel_prepare_lock`）防止并发下载同一内核
2. 解析 profiles 根目录和内核安装目录
3. 若目录已存在：
   - 校验包布局完整性（`validate_kernel_package_layout`）
   - 校验核心 DLL 签名（`verifier::verify_kernel`）
   - 主签名匹配 → 直接复用
   - 兼容签名命中 → 复用并记录
   - 校验失败 → 重新下载替换
4. 若目录不存在或校验失败 → 下载到 staging 目录 → 备份旧目录 → 原子替换 → 记录安装记录

**内核安装的原子性保障**:
- 下载到 `.{name}.staging-{uuid}` 临时目录
- 旧目录重命名为 `.{name}.backup-{uuid}`
- staging 重命名为正式目录
- 失败时回滚（restore backup，remove staging）

### 2.3 内嵌运行时架构

Simprint 的浏览器运行时是**内嵌**的（同进程），不是独立子进程：

```
Tauri 主进程
  └── SimprintRuntimeManager (src-tauri/src/app/runtime.rs)
        └── RuntimeHost (crates/runtime/src/app/host.rs)  ← 同进程内的运行时
              ├── KernelRuntime (launcher.rs) → 启动浏览器子进程
              ├── EventBus Manager → 与浏览器扩展通信
              └── CDP Endpoint Manager → 管理 CDP 端口
```

通信协议：二进制 MessagePack 格式（`rmp_serde`），自定义帧协议（MAGIC + version + payload），通过内存直接调用（`handle_request`）而非 IPC。

---

## 三、浏览器内核注册表（browser_kernels）

### 3.1 默认内核清单

**文件**: `src-tauri/crates/business/resources/default-browser-kernels.json`

```json
{
  "schema_version": 1,
  "source_id": "simprint-builtin",
  "kernels": [
    {
      "type_code": "SIMPRINT_KERNEL_CHROMIUM",
      "resource_name": "Chrome 144",
      "install_dir_name": "Chrome 144",
      "version": "144.0.7559.118.5",
      "name": "simprint-browser-144.0.7559.118.zip",
      "platform": "windows",
      "arch": "x86_64",
      "url": "https://pub-39307a5e69c74324855a762027cbf9bf.r2.dev/versions/...",
      "hash": "...",           // 包文件 SHA256（下载校验）
      "signature": "...",      // chrome.dll 前 10MB 的 SHA256（安装校验）
      "compatible_signatures": [...],  // 兼容签名列表
      "file_size": 184547016,
      "package_format": "zip",
      "requires_extract": true,
      "is_latest": true,
      "status": "active",
      "priority": 100
    }
  ]
}
```

### 3.2 数据库表结构

**迁移文件**: `src-tauri/crates/business/migrations/20260812000002_create_browser_kernel_registry.sql`

三张核心表：

1. **browser_kernel_artifacts** — 内核元数据（内容寻址）
   - `kernel_id`: 主键，由内容派生的 SHA256（type_code + platform + arch + hash + signature + package_format + requires_extract）
   - `type_code`: 类型代码（如 `SIMPRINT_KERNEL_CHROMIUM`）
   - `resource_name`: 资源名称
   - `install_dir_name`: 安装目录名
   - `version`: 版本号
   - `platform` / `arch`: 平台和架构
   - `package_hash`: 包文件哈希
   - `executable_signature`: 可执行文件签名
   - `compatible_executable_signatures`: 兼容签名（JSON 数组）

2. **browser_kernel_sources** — 内核下载源
   - 多对一：一个内核可以有多个下载源
   - `source_id` + `url` + `priority` 优先级
   - 支持启用/禁用

3. **browser_kernel_installations** — 安装记录
   - 内核是否已安装、安装路径、验证签名、安装时间

4. **environment_kernel_bindings** — 环境与内核的绑定
   - 一个环境绑定一个内核
   - 支持从旧版 `window_info.kernel` 迁移

### 3.3 内核 ID 的内容寻址设计

内核 ID 由内核内容的关键属性派生（`kernel_id()` 函数）：

```
kernel_id = SHA256(
  type_code + platform + arch + package_hash +
  executable_signature + package_format + requires_extract +
  entrypoint_template + extract_root
)
```

- 修改显示名称（resource_name）或下载 URL 不会改变 kernel_id
- 修改包哈希或签名会生成新的 kernel_id
- 导入是幂等的（ON CONFLICT 更新非关键字段）

### 3.4 业务层服务

**文件**: `src-tauri/crates/business/src/services/browser_kernels.rs`

关键函数：
- `import_default_catalog()` — 导入内置清单到数据库
- `import_catalog_file(path)` — 导入用户自定义清单文件
- `list_browser_kernels(platform, type_code)` — 按类型分组列出
- `get_browser_kernel(kernel_id)` — 按 ID 查询
- `find_browser_kernel_by_name(resource_name)` — 按名称查找
- `default_browser_kernel()` — 获取默认 Chromium 内核
- `bind_environment_kernel(env_uuid, kernel_id)` — 绑定环境到内核
- `get_environment_kernel(env_uuid)` — 获取环境绑定的内核
- `resolve_requested_kernel(window_info, allow_default)` — 解析环境请求的内核（kernel_id → kernel 名称 → 默认）
- `migrate_legacy_environment_bindings()` — 迁移旧版名称绑定到 ID 绑定
- `record_kernel_installation(kernel_id, path, signature)` — 记录安装

---

## 四、指纹注入机制

### 4.1 FingerprintConfig 结构

**定义**: `src-tauri/src/infrastructure/runtime/api.rs`

| 类别 | 字段 | 说明 |
|------|------|------|
| 基础信息 | `language`, `interface_language`, `timezone`, `platform`, `user_agent` | 浏览器环境标识 |
| 窗口 | `window_width/height`, `window_x/y`, `resolution`, `color_depth`, `device_pixel_ratio`, `max_touch_points` | 显示相关 |
| 画布指纹 | `canvas` | Canvas 噪声注入种子 |
| WebGL | `webgl_image`, `webgl_info`, `webgl_vendor`, `webgl_renderer`, `webgpu` | WebGL 指纹伪装 |
| 音频 | `audio_context` | AudioContext 噪声 |
| 字体 | `font_fingerprint`, `font_list` | 字体指纹控制 |
| WebRTC | `webrtc` | "disable" / "replace" / 真实 |
| 其他 | `client_rects`, `media_devices`, `speech_voices`, `do_not_track` | 杂项指纹 |
| 硬件 | `hardware_concurrency`, `device_memory` | 硬件信息伪装 |
| 安全 | `ssl_fingerprint`, `port_scan_protection`, `scan_whitelist` | 安全相关 |
| 设备 | `device_name`, `mac_address`, `mac_address_mode` | 设备标识 |
| 启动 | `hardware_acceleration`, `disable_sandbox`, `startup_parameters` | 启动参数 |
| 标识 | `env_id`, `env_name` | 环境标识 |
| 其他 | `sound`, `images`, `video`, `random_fingerprint_on_launch` | 开关项 |

### 4.2 指纹注入的四种途径

#### 途径 1：CDP 脚本注入（主要方式，优先执行）

**文件**: `src-tauri/crates/runtime/src/services/environment/kernel/launcher.rs`

在 `launch_browser()` 中，CDP 就绪后立即执行：

```rust
// 1. 生成指纹脚本
let script = generate_fingerprint_script(&ext_config);

// 2. 通过 CDP Page.addScriptToEvaluateOnNewDocument 注入
inject_fingerprint_via_cdp(cdp_port, &script).await;

// 3. 通过 CDP Network.setUserAgentOverride 覆盖 UA
inject_user_agent_override(cdp_port, &effective_ua).await;
```

注入流程：
1. 连接 `http://127.0.0.1:{port}/json/list` 获取所有 page target
2. 对每个 page 通过 WebSocket 连接 CDP
3. 发送 `Page.enable`
4. 发送 `Page.addScriptToEvaluateOnNewDocument`，脚本在每个新文档创建时执行

#### 途径 2：浏览器扩展注入（备用/兜底方式）

**文件**: `src-tauri/crates/runtime/src/services/environment/kernel/fingerprint_extension.rs`

创建一个 MV3 扩展 `simprint-fp-extension`，通过 `--load-extension` 参数加载：

```
simprint-fp-extension/
├── manifest.json     (MV3, content_scripts, run_at: document_start)
└── content.js        (注入 MAIN_JS 指纹脚本到页面)
```

扩展通过 `<script>` 标签注入的方式将脚本注入页面上下文（绕过 content script 隔离）。

两种注入方式有幂等性保护（`self.__simprintFPInjected` 标记）。

#### 途径 3：命令行参数

**文件**: `src-tauri/crates/runtime/src/services/environment/kernel/launcher.rs` → `spawn_browser_process()`

| 参数 | 来源 | 说明 |
|------|------|------|
| `--simprint-env-id={uuid}` | 环境 ID | 自定义参数，供浏览器内扩展识别环境 |
| `--user-data-dir={path}` | 缓存路径 | 用户数据目录 |
| `--remote-debugging-port={port}` | 动态分配 | CDP 调试端口 |
| `--remote-allow-origins=*` | 固定 | 允许所有来源连接 CDP |
| `--window-position={x,y}` | 配置 | 窗口位置 |
| `--window-size={w,h}` | 配置 | 窗口大小 |
| `--load-extension={dirs}` | 扩展列表 | 加载指纹扩展 + 用户扩展 |
| `--disable-webrtc` | webrtc=disable | 禁用 WebRTC |
| `--force-webrtc-ip-handling-policy=...` | webrtc=replace | WebRTC 代理模式 |
| `--disable-features=...` | 多个来源 | 禁用功能特性列表 |
| `--no-first-run` | 固定 | 跳过首次运行向导 |
| `--no-default-browser-check` | 固定 | 不检查默认浏览器 |
| `--disable-session-crashed-bubble` | 固定 | 禁用崩溃恢复气泡 |
| `--disable-popup-blocking` | 固定 | 禁用弹窗拦截 |
| `--autoplay-policy=no-user-gesture-required` | 固定 | 允许自动播放 |
| `--simprint-display-id={id}` | display_id | 显示器 ID（自定义参数） |
| 用户自定义 | `startup_parameters` | 用户配置的额外启动参数 |

**注意**：代理**不通过** `--proxy-server` 命令行参数注入（因为 Chrome 的该参数不支持 SOCKS5 认证），而是通过 eventbus 传递给浏览器内的扩展。

#### 途径 4：进程环境变量

```rust
// 时区通过 TZ 环境变量注入
if let Some(ref tz) = fp.timezone {
    if !tz.is_empty() && tz != "real" && tz != "system" {
        command.env("TZ", tz);
    }
}
```

### 4.3 指纹脚本详解

**文件**: `src-tauri/crates/runtime/src/services/environment/kernel/fingerprint_extension.rs` → `MAIN_JS`

脚本覆盖的指纹维度：

| 指纹类型 | 实现方式 |
|----------|----------|
| **Canvas** | 劫持 `getImageData` / `toDataURL` / `toBlob`，对 320x320 以下的画布添加像素噪声 |
| **WebGL** | 劫持 `getParameter`，伪装 UNMASKED_VENDOR_WEBGL(37445) 和 UNMASKED_RENDERER_WEBGL(37446)；确保 WEBGL_debug_renderer_info 扩展可用；伪装 shader precision |
| **AudioContext** | 劫持 `AudioBuffer.getChannelData`，在采样点添加微小噪声 |
| **WebRTC** | disable 模式直接替换 RTCPeerConnection；replace 模式包装并控制 ICE 行为 |
| **字体** | 劫持 `document.fonts.check()` 和 `CanvasRenderingContext2D.measureText`，控制可见字体列表 |
| **navigator.platform** | `Object.defineProperty` 覆盖 getter |
| **navigator.userAgent** | 覆盖 getter，同时补全 UA 版本号（x.0.0.0 → 完整版本） |
| **navigator.userAgentData** | 覆盖 brands / mobile / platform / model / getHighEntropyValues |
| **navigator.language / languages** | 覆盖 getter |
| **时区** | 劫持 `Intl.DateTimeFormat.prototype.resolvedOptions` 和 `format`，伪装时区 |
| **screen.colorDepth / pixelDepth** | 覆盖 getter |
| **window.devicePixelRatio** | 覆盖 getter |
| **navigator.maxTouchPoints** | 覆盖 getter |
| **navigator.doNotTrack** | 覆盖 getter |
| **navigator.hardwareConcurrency** | 覆盖 getter |
| **navigator.deviceMemory** | 覆盖 getter |
| **Element.getClientRects** | 添加微小偏移噪声 |
| **navigator.mediaDevices.enumerateDevices** | 伪装 deviceId 和 groupId |
| **speechSynthesis.getVoices** | 伪装 voiceURI |

**反检测保护**：
- 所有被替换的函数都通过 `Function.prototype.toString` 伪装成原生函数（返回 `function name() { [native code] }`）
- 维护 `name` 和 `length` 属性的一致性
- 递归处理 iframe 的 `contentWindow` / `contentDocument`
- 处理 `window.open` 打开的新窗口

### 4.4 UA 版本号补全机制

由于指纹配置中 UA 可能只写主版本号（如 "Chrome/144"），代码会自动补全为完整版本号：

```
Chrome 144 → 144.0.6574.137
Chrome 143 → 143.0.6351.112
...
```

在两处补全：CDP `Network.setUserAgentOverride` 调用前，以及指纹 JS 脚本内部。

### 4.5 代理配置注入（eventbus 途径）

代理配置（含 SOCKS5 认证）通过浏览器内扩展 + eventbus 通道注入：

1. 启动时将代理配置放入 `LaunchConfig`，通过 `eventbus_manager().start_server()` 建立服务端
2. 浏览器内的 Simprint 扩展连接到 eventbus server，接收 `LaunchConfig`
3. 扩展通过 Chrome extension API 设置代理（支持 SOCKS5 认证）
4. 运行中可通过 `Topic::ProxySet` 事件动态更换代理

---

## 五、macOS 支持情况

### 5.1 代码层面的多平台支持

**Cargo.toml** 中的平台条件编译：

```toml
# Windows 专属依赖
[target.'cfg(windows)'.dependencies]
smbios-lib = "0.9.2"
winreg = "0.55"
windows = { version = "0.61.1", features = [...] }

# 三平台通用插件
[target.'cfg(any(target_os = "macos", windows, target_os = "linux"))'.dependencies]
tauri-plugin-autostart = "2.5.1"
tauri-plugin-single-instance = "2.2.0"
```

**utils.rs** 中的平台条件编译：

```rust
pub fn exe_name() -> &'static str {
    #[cfg(target_os = "windows")]
    return "simprint.exe";
    #[cfg(not(target_os = "windows"))]
    return "simprint";
}

pub fn core_dll_name() -> &'static str {
    #[cfg(target_os = "windows")]
    return "chrome.dll";
    #[cfg(target_os = "macos")]
    return "Chromium Framework";
    #[cfg(target_os = "linux")]
    return "libchrome.so";
}
```

**launcher.rs** 中的平台条件编译：
- Windows: `CREATE_NO_WINDOW` 标志隐藏控制台窗口
- 非 Windows: 设置可执行文件权限为 0o755

### 5.2 实际支持状态

| 方面 | macOS 支持 |
|------|-----------|
| Rust 代码框架 | 有条件编译支持 |
| Tauri 配置 | **仅有 Windows 配置**（nsis、webviewInstallMode） |
| CI 构建 | **仅有 Windows**（windows-latest、windows-2025） |
| 浏览器内核 | **仅有 Windows 平台**（default-browser-kernels.json 中 platform=windows） |
| 打包目标 | `targets: "nsis"`（仅 Windows 安装包） |
| 资源文件 | mihomo 和 sing-box 均为 Windows 可执行文件 |

**结论**：代码框架具备跨平台扩展能力（条件编译已就位），但当前实际发布和内核仅支持 Windows 平台。macOS 需要新增对应平台的浏览器内核、调整 Tauri 配置、添加 CI 构建流程才能支持。

---

## 六、Environment 配置的结构与存储方式

### 6.1 数据库存储

**表**: `environments` + `environment_configs`

`environment_configs` 中与指纹/启动相关的 JSON 字段：
- `window_info` — 窗口配置 + kernel_id（内核绑定）
- `basic_settings` — 基础设置（timezone、language 等）
- `fingerprint_settings` — 指纹设置（完整 FingerprintConfig）
- `cookie_config` — Cookie 配置
- `url_config` — URL 配置
- `account_config` — 账号配置

环境与内核的绑定通过 `environment_kernel_bindings` 表，也兼容旧版 `window_info.kernel` 名称方式。

### 6.2 环境状态枚举

**定义**: `src-tauri/src/domain/environment.rs`

```
Verifying → Downloading → Extracting → Ready → Initializing → Starting → Running → Stopping → Stopped
                                                                   ↓
                                                                 Error
```

状态由 `EnvironmentStatusManager` 管理（主进程和 runtime 各有一份）。

### 6.3 用户数据目录结构

```
{cache_path}/browser/cache/{env_uuid}/
├── Default/
│   ├── Preferences         (启动前修改：禁用 session restore、启用媒体自动播放等)
│   ├── Sessions/           (启动前删除)
│   ├── "Last Session"      (启动前删除)
│   ├── "Last Tabs"         (启动前删除)
│   ├── "Current Session"   (启动前删除)
│   └── "Current Tabs"      (启动前删除)
├── "Local State"           (启动前修改：infobars 标记等)
├── simprint-fp-extension/  (启动时生成的指纹扩展)
│   ├── manifest.json
│   └── content.js
└── simprint-browser.log    (浏览器 stderr/stdout 日志)
```

启动前会对 Preferences 做多项修改：
- `session.restore_on_startup = 5`（总是打开新标签页）
- `profile.exit_type = "Normal"`（清除崩溃标记）
- 清除 `startup_urls` 和 `urls_to_restore_on_startup`
- 启用媒体流、通知、加密媒体
- 启用自动播放（`autoplay_mode = 1`）
- 禁用 infobars 警告条

---

## 七、浏览器启动完整流程

### 7.1 流程图

```
前端（React 插件）
    │
    │  invoke('start_environment_by_uuid', { envUuid })
    ▼
Tauri 命令层 (commands/environment.rs)
    │
    │  EnvironmentLaunchRuntimeService::resolve_launch_paths()
    │  EnvironmentLaunchRuntimeService::start_environment_by_uuid()
    ▼
KernelService::ensure_kernel_ready()  (services/environment/kernel/mod.rs)
    │
    ├─ 目录已存在？
    │   ├─ 是 → 校验布局 → 校验签名 → 通过/失败
    │   └─ 否 → 下载 → 解压 → staging → 原子替换
    │
    │  record_kernel_installation() → DB 记录
    ▼
KernelService::launch_environment()
    │
    ▼
runtime_bridge::launch_environment()
    │
    ├─ prepare_start_request()
    │   ├─ 创建 user_data_dir
    │   ├─ 清理 session restore 文件
    │   ├─ 检测语言/时区（后台异步，不阻塞启动）
    │   ├─ 分配窗口位置
    │   ├─ 解密账号密码
    │   ├─ 处理代理配置（高级协议走 mihomo）
    │   └─ 安装扩展（含指纹扩展）
    │
    ▼
SimprintRuntimeManager::send_environment_command(StartEnvironment)
    │
    │  (同进程内调用，通过 Message 协议)
    ▼
runtime crate: KernelRuntime::execute()
    │
    ▼
launch_browser()  (crates/runtime/src/.../launcher.rs)
    │
    ├─ 1. 状态检查（防重复启动）
    ├─ 2. 分配 CDP 端口
    ├─ 3. 启动 eventbus server
    ├─ 4. spawn_browser_process()
    │   ├─ 构建命令行参数（~30 个参数）
    │   ├─ 创建指纹扩展目录
    │   ├─ 设置 TZ 环境变量
    │   ├─ 重定向 stdout/stderr 到日志文件
    │   └─ 启动进程（Windows: CREATE_NO_WINDOW）
    ├─ 5. wait_for_browser_ready() — 等待 eventbus 握手
    ├─ 6. wait_for_cdp_ready() — 轮询 /json/version
    ├─ 7. CDP 指纹注入
    │   ├─ Page.addScriptToEvaluateOnNewDocument (主指纹脚本)
    │   └─ Network.setUserAgentOverride (UA 覆盖)
    ├─ 8. 设置状态为 Running
    └─ 9. 发送 environment.launch_ready 事件
```

### 7.2 关键启动参数汇总

```
基础参数:
  --simprint-env-id={uuid}
  --user-data-dir={path}
  --remote-debugging-port={port}
  --remote-allow-origins=*
  --enable-logging=stderr
  --no-first-run
  --no-default-browser-check
  --disable-session-crashed-bubble
  --disable-popup-blocking
  --autoplay-policy=no-user-gesture-required
  --ignore-gpu-blocklist
  --enable-gpu-rasterization
  --enable-zero-copy

窗口参数:
  --window-position=x,y
  --window-size=w,h
  --simprint-display-id={id}

指纹相关:
  --load-extension=fp-ext-dir,user-ext1,...
  --disable-features=InfiniteSessionRestore,RendererCodeIntegrity,
                     PrivacySandboxSettings4,IpProtection,
                     BlockInsecurePrivateNetworkRequests
  (webrtc=disable) --disable-webrtc --enforce-webrtc-ip-permission-check
  (webrtc=replace) --force-webrtc-ip-handling-policy=disable_non_proxied_udp

开发模式额外:
  --no-sandbox
  --test-type
  --v=1

用户自定义:
  startup_parameters 中的所有参数
```

---

## 八、关键文件索引

### 8.1 内核管理

- `src-tauri/src/services/environment/kernel/mod.rs` — KernelService 主服务
- `src-tauri/src/services/environment/kernel/runtime_bridge.rs` — 主进程↔runtime 桥接
- `src-tauri/crates/runtime/src/services/environment/kernel/launcher.rs` — 浏览器启动核心
- `src-tauri/crates/runtime/src/services/environment/kernel/mod.rs` — KernelRuntime
- `src-tauri/src/services/environment/kernel/downloader.rs` — 内核下载
- `src-tauri/src/services/environment/kernel/verifier.rs` — 内核校验
- `src-tauri/src/services/environment/kernel/utils.rs` — 工具函数
- `src-tauri/src/services/environment/kernel/state.rs` — 内核准备锁

### 8.2 指纹相关

- `src-tauri/crates/runtime/src/services/environment/kernel/fingerprint_extension.rs` — 指纹脚本与扩展生成
- `src-tauri/src/infrastructure/runtime/api.rs` — FingerprintConfig 结构定义
- `src-tauri/src/domain/environment.rs` — Environment/KernelDetail 领域模型

### 8.3 内核注册表

- `src-tauri/crates/business/resources/default-browser-kernels.json` — 默认内核清单
- `src-tauri/crates/business/src/services/browser_kernels.rs` — 业务层服务
- `src-tauri/crates/business/migrations/20260812000002_create_browser_kernel_registry.sql` — 数据库 schema
- `src-tauri/crates/business/migrations/20260812000003_add_browser_kernel_compatible_signatures.sql` — 兼容签名字段

### 8.4 运行时管理

- `src-tauri/src/app/runtime.rs` — SimprintRuntimeManager
- `src-tauri/src/app/context.rs` — AppContext（全局上下文）
- `src-tauri/crates/runtime/src/app/host.rs` — RuntimeHost
- `src-tauri/src/infrastructure/runtime/message.rs` — 消息协议（二进制帧）
- `src-tauri/src/infrastructure/runtime/api.rs` — 请求/响应类型

### 8.5 前端（环境管理）

- `plugins/services/environment/src/runtime.ts` — 环境启动前端调用
- `plugins/services/environment/src/types.ts` — TypeScript 类型定义
- `plugins/pages/environment-manager/src/index.tsx` — 环境管理页面

### 8.6 CI / 构建

- `.github/workflows/ci.yml` — CI（仅 Windows）
- `.github/workflows/build-windows.yml` — Windows 构建（x64/arm64/x86）
- `.github/workflows/release.yml` — 发布流程
- `src-tauri/tauri.conf.json` — Tauri 配置（仅 Windows bundle）
