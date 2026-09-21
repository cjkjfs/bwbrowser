@echo off
chcp 65001 >nul
setlocal

echo ============================================
echo   BwBrowser Windows 打包脚本
echo ============================================
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
echo [4/4] 构建 Tauri 应用 (NSIS 安装包)...
call pnpm tauri build --target x86_64-pc-windows-msvc --bundles nsis
if errorlevel 1 (
    echo 错误: Tauri 构建失败
    pause
    exit /b 1
)

echo.
echo ============================================
echo   打包完成!
echo   版本: %VERSION%
echo   安装包位置: src-tauri\target\release\bundle\nsis\
echo ============================================
echo.

dir "src-tauri\target\release\bundle\nsis\*.exe" 2>nul

pause
