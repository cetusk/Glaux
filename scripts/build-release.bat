@echo off
setlocal
rem ============================================================
rem Build the release version of the Glaux app (Tauri) for Windows.
rem
rem Usage:
rem   scripts\build-release.bat
rem
rem Output (copied to <repo>\release\):
rem   Glaux.exe                     portable app. Copy it anywhere and run.
rem                                 (needs WebView2 runtime: preinstalled on Win10/11)
rem   Glaux_<version>_x64-setup.exe installer. Per-user install (no admin),
rem                                 adds a Start menu shortcut.
rem
rem Requires: Rust 1.88+, Node.js (LTS). The first build takes several minutes.
rem Close any running Glaux (dev or release) first: the exe may be locked.
rem Keep this file ASCII-only (see glaux-mcp.bat).
rem ============================================================

set "ROOT=%~dp0.."

where cargo >nul 2>nul
if errorlevel 1 (
    echo cargo not found. Install Rust from https://rustup.rs
    exit /b 1
)
where npm >nul 2>nul
if errorlevel 1 (
    echo npm not found. Install Node.js LTS from https://nodejs.org
    exit /b 1
)

rem Same target dir as glaux-app.bat so dev and release share compiled crates.
set "CARGO_TARGET_DIR=%ROOT%\target\windows"

pushd "%ROOT%\app"
echo Syncing npm dependencies...
call npm install --no-audit --no-fund
if errorlevel 1 (
    popd
    echo npm install failed.
    exit /b 1
)
rem Remove installers of older versions left in the bundle folder,
rem so that only the one built now is copied below.
if exist "%CARGO_TARGET_DIR%\release\bundle\nsis\*-setup.exe" del /Q "%CARGO_TARGET_DIR%\release\bundle\nsis\*-setup.exe"
echo Building release (frontend + Rust + installer)...
call npm run tauri build
if errorlevel 1 (
    popd
    echo tauri build failed.
    exit /b 1
)
popd

set "OUT=%ROOT%\release"
rem Keep other files in release\ (e.g. the renamed files made by make_release.py).
rem Replace only the files this script makes.
if not exist "%OUT%" mkdir "%OUT%"
if exist "%OUT%\Glaux.exe" del /Q "%OUT%\Glaux.exe"
if exist "%OUT%\Glaux_*_x64-setup.exe" del /Q "%OUT%\Glaux_*_x64-setup.exe"
copy /Y "%CARGO_TARGET_DIR%\release\glaux-app.exe" "%OUT%\Glaux.exe" >nul
if errorlevel 1 (
    echo glaux-app.exe not found in %CARGO_TARGET_DIR%\release
    exit /b 1
)
copy /Y "%CARGO_TARGET_DIR%\release\bundle\nsis\*-setup.exe" "%OUT%\" >nul
if errorlevel 1 (
    echo installer not found in %CARGO_TARGET_DIR%\release\bundle\nsis
    exit /b 1
)
echo.
echo Done. See %OUT%
dir /B "%OUT%"
exit /b 0
