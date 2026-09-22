@echo off
setlocal
rem ============================================================
rem Glaux app launcher (Tauri dev mode)
rem
rem Usage:
rem   scripts\glaux-app.bat [C:\path\to\MySong.glaux]
rem
rem Defaults to <repo>\tests\live\TestSong.glaux when no argument.
rem The app hosts an MCP server at http://127.0.0.1:41920/mcp
rem (override port with GLAUX_MCP_PORT). Register with Claude Code:
rem   claude mcp add --transport http glaux http://127.0.0.1:41920/mcp
rem
rem Requires: Rust 1.88+, Node.js (LTS), WebView2 (preinstalled on Win11).
rem Keep this file ASCII-only (see glaux-mcp.bat).
rem ============================================================

set "ROOT=%~dp0.."

if "%~1"=="" (
    set "GLAUX_PROJECT=%ROOT%\tests\live\TestSong.glaux"
) else (
    set "GLAUX_PROJECT=%~1"
)

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

set "CARGO_TARGET_DIR=%ROOT%\target\windows"

pushd "%ROOT%\app"

rem Always run npm install: it is a fast no-op when up to date, and it
rem self-heals a partially broken node_modules (e.g. after cross-platform work).
echo Syncing npm dependencies...
call npm install --no-audit --no-fund
if errorlevel 1 (
    popd
    echo npm install failed.
    exit /b 1
)

echo Project: %GLAUX_PROJECT%
call npm run tauri dev
set "RC=%errorlevel%"
popd
exit /b %RC%
