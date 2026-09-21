@echo off
chcp 65001 >nul 2>&1
setlocal enabledelayedexpansion

echo ============================================
echo   BwBrowser Installer Build Script
echo ============================================
echo.

REM ---- Prompt for version number ----
set /p "VERSION=Enter version number (e.g. 0.30.0): "

if "!VERSION!"=="" (
    echo Error: Version number cannot be empty.
    echo.
    pause
    exit /b 1
)

echo.
echo Target version: !VERSION!
echo.

REM ---- Confirm before proceeding ----
set /p "CONFIRM=Proceed with build? [Y/N]: "
if /i not "!CONFIRM!"=="Y" (
    echo Build cancelled.
    echo.
    pause
    exit /b 0
)

echo.
echo ============================================

REM ---- Step 1: Update version in all config files ----
echo [1/2] Updating version in config files ...
node scripts/set-version.mjs "!VERSION!"
if errorlevel 1 (
    echo Error: Failed to update version files.
    echo.
    pause
    exit /b 1
)

echo.
echo ============================================

REM ---- Step 2: Clean previous build cache ----
echo [2/3] Cleaning previous build cache ...
echo     - Rust release target
echo     - Next.js .next cache
echo     - Frontend dist
echo.

cd /d "%~dp0src-tauri" && cargo clean --release
cd /d "%~dp0"
if exist ".next" rmdir /s /q ".next"
if exist "out" rmdir /s /q "out"
if errorlevel 1 (
    echo Warning: Some cache directories could not be removed ^(may be in use^).
)
echo Cache cleaned.
echo.

REM ---- Step 3: Build the installer ----
echo [3/3] Building installer ...
echo     This will:
echo       1. Copy proxy binary
echo       2. Build the frontend ^(Next.js^)
echo       3. Compile Rust backend ^(release mode, may take 10-30 min^)
echo       4. Bundle into NSIS installer ^(.exe^)
echo.
echo Building ...
echo.

pnpm tauri build
if errorlevel 1 (
    echo.
    echo ============================================
    echo   BUILD FAILED
    echo ============================================
    echo.
    echo Check the output above for error details.
    echo.
    pause
    exit /b 1
)

echo.
echo ============================================
echo   BUILD SUCCESSFUL
echo ============================================
echo.
echo Version: !VERSION!
echo.

echo Installer files (NSIS):
echo   src-tauri\target\release\bundle\nsis\
if exist "src-tauri\target\release\bundle\nsis" (
    dir /b "src-tauri\target\release\bundle\nsis\*.exe" 2>nul
) else (
    echo   (NSIS bundle directory not found)
)

echo.
echo MSI files:
echo   src-tauri\target\release\bundle\msi\
if exist "src-tauri\target\release\bundle\msi" (
    dir /b "src-tauri\target\release\bundle\msi\*.msi" 2>nul
) else (
    echo   (MSI bundle directory not found - only built if configured)
)

echo.

REM ---- Open the folder containing the installer ----
set "NSIS_DIR=%~dp0src-tauri\target\release\bundle\nsis"
if exist "%NSIS_DIR%\*.exe" (
    echo Opening installer folder ...
    start "%NSIS_DIR%"
    explorer "%NSIS_DIR%"
) else if exist "%~dp0src-tauri\target\release\bundle\msi" (
    echo Opening installer folder ...
    start "%~dp0src-tauri\target\release\bundle\msi"
    explorer "%~dp0src-tauri\target\release\bundle\msi"
) else (
    echo Installer folder not found.
)

pause
