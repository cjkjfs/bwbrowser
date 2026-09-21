@echo off
chcp 65001 >nul
title BW Browser Dev
cd /d "%~dp0"

echo ============================================
echo   BW Browser Dev Server
echo ============================================
echo.

where node >nul 2>&1
if errorlevel 1 (
    echo [ERROR] Node.js not found
    pause
    exit /b 1
)

where pnpm >nul 2>&1
if errorlevel 1 (
    echo [ERROR] pnpm not found, run: npm install -g pnpm
    pause
    exit /b 1
)

echo Starting...
echo.

pnpm tauri dev

echo.
echo App exited.
pause
