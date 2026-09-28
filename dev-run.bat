@echo off
chcp 65001 >nul
title BW Browser Dev
cd /d "%~dp0"

echo ============================================
echo   BW Browser Dev Server
echo ============================================
echo.

where node >nul 2>&1
if errorlevel 1 goto :nonode

where pnpm >nul 2>&1
if errorlevel 1 goto :nopnpm

REM --- Auto-install missing JS dependencies ---
if exist "node_modules" goto :hasmod

echo [AUTO] node_modules missing, running pnpm install (first run may take a while)...
echo.
call pnpm install
if errorlevel 1 goto :installfail
echo.
echo [OK] Dependencies installed
echo.
goto :run

:hasmod
if exist "node_modules\.bin\tauri" goto :run
echo [AUTO] tauri CLI not found, dependencies may be incomplete, repairing...
echo.
call pnpm install
if errorlevel 1 goto :installfail
echo.
echo [OK] Dependencies repaired
echo.

:run
echo Starting...
echo.
call pnpm tauri dev

echo.
echo App exited.
pause
exit /b 0

:nonode
echo [ERROR] Node.js not found
pause
exit /b 1

:nopnpm
echo [ERROR] pnpm not found, run: npm install -g pnpm
pause
exit /b 1

:installfail
echo.
echo [ERROR] Dependency installation failed, check your network and retry.
pause
exit /b 1
