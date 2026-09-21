@echo off
chcp 65001 >nul
setlocal

echo ============================================
echo   BwBrowser macOS 打包脚本
echo ============================================
echo 注意: 此脚本需在 macOS 上运行 (Windows 下仅生成配置)
echo.

set /p VERSION="请输入版本号 (如 0.30.0): "

if "%VERSION%"=="" (
    echo 错误: 版本号不能为空
    pause
    exit /b 1
)

echo.
echo [1/4] 更新 tauri.conf.json 版本号...
powershell -Command "(Get-Content 'src-tauri\tauri.conf.json' -Raw) -replace '\"version\": \"[^\"]+\"', '\"version\": \"%VERSION%\"' | Set-Content 'src-tauri\tauri.conf.json' -NoNewline"
echo 版本号已更新为 %VERSION%

echo.
echo [2/4] 安装依赖...
call pnpm install
if errorlevel 1 (
    echo 错误: 依赖安装失败
    pause
    exit /b 1
)

echo.
echo [3/4] 构建 Rust sidecar (bwbrowser-proxy)...
call pnpm copy-proxy-binary:release
if errorlevel 1 (
    echo 错误: sidecar 构建失败
    pause
    exit /b 1
)

echo.
echo [4/4] 构建 Tauri 应用 (DMG 安装包)...
echo.
echo 在 macOS 上运行以下命令:
echo   pnpm tauri build --target aarch64-apple-darwin --bundles dmg
echo   或
echo   pnpm tauri build --target x86_64-apple-darwin --bundles dmg
echo.

echo 如当前为 macOS 环境,将自动执行构建...
call pnpm tauri build --bundles dmg
if errorlevel 1 (
    echo.
    echo 如果构建失败,请确认在 macOS 上运行,且已安装 Xcode Command Line Tools
    echo 手动执行: pnpm tauri build --bundles dmg
    pause
    exit /b 1
)

echo.
echo ============================================
echo   打包完成!
echo   版本: %VERSION%
echo   DMG 位置: src-tauri\target\release\bundle\dmg\
echo ============================================
echo.

pause
