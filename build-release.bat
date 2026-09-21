@echo off
chcp 65001 >nul
setlocal enabledelayedexpansion

REM ============================================================
REM  BW Browser - 打包发布脚本
REM  构建 Windows 安装包
REM ============================================================

title BW Browser Build

cd /d "%~dp0"

echo.
echo ============================================
echo   BW Browser 打包发布中...
echo ============================================
echo.

REM ---------- 检查 Node.js ----------
where node >nul 2>&1
if %errorlevel% neq 0 (
    echo [错误] 未检测到 Node.js，请先安装 Node.js
    pause
    exit /b 1
)

REM ---------- 清理旧构建 ----------
echo [1/5] 清理旧的构建文件...
if exist "src-tauri\target\release" (
    echo   清理 release 目录...
)

REM ---------- 安装依赖 ----------
echo.
echo [2/5] 检查依赖...
where pnpm >nul 2>&1
if %errorlevel% neq 0 (
    echo [警告] 未检测到 pnpm，使用 npm
    npm install
) else (
    pnpm install --frozen-lockfile
)

REM ---------- 复制代理二进制 ----------
echo.
echo [3/5] 复制代理二进制文件...
if exist "src-tauri\binaries" (
    echo   二进制目录已存在
) else (
    node src-tauri\copy-proxy-binary.mjs
)

REM ---------- 构建前端 ----------
echo.
echo [4/5] 构建前端 (Next.js build)...
set NEXT_TELEMETRY_DISABLED=1
npx next build
if %errorlevel% neq 0 (
    echo.
    echo [错误] 前端构建失败！
    pause
    exit /b 1
)

REM ---------- 构建 Tauri ----------
echo.
echo [5/5] 构建 Tauri 安装包...
echo   首次构建需要较长时间，请耐心等待...
echo.

cd src-tauri
cargo tauri build --bundles nsis

if %errorlevel% equ 0 (
    echo.
    echo ============================================
    echo   打包成功！
    echo   输出目录: src-tauri\target\release\bundle\
    echo ============================================
) else (
    echo.
    echo [错误] 打包失败！
)

pause
