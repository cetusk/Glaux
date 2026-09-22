@echo off
setlocal
rem ============================================================
rem Glaux MCP server launcher (stdio)
rem
rem Usage:
rem   scripts\glaux-mcp.bat <MySong.glaux>
rem
rem Register with Claude Code (run on the host):
rem   claude mcp add glaux -- cmd /c "<repo>\scripts\glaux-mcp.bat" "C:\path\to\MySong.glaux"
rem
rem Notes:
rem  - stdout is reserved for the MCP protocol. Any echo added here
rem    MUST be redirected to stderr (1>&2).
rem  - Requires Rust 1.88+ on Windows (rmcp's MSRV).
rem  - Windows builds go to target\windows so they do not clash with
rem    artifacts built inside the Linux sandbox.
rem  - Keep this file ASCII-only: cmd parses batch files with the
rem    console codepage (e.g. CP932) and non-ASCII text breaks parsing.
rem ============================================================

set "ROOT=%~dp0.."

if "%~1"=="" (
    echo usage: glaux-mcp.bat ^<MySong.glaux^> 1>&2
    exit /b 1
)

where cargo >nul 2>nul
if errorlevel 1 (
    echo cargo not found. Install Rust from https://rustup.rs 1>&2
    exit /b 1
)

set "CARGO_TARGET_DIR=%ROOT%\target\windows"

rem Send build logs to stderr; keep stdout clean for MCP.
cargo build --release -p glaux-mcp --manifest-path "%ROOT%\Cargo.toml" 1>&2
if errorlevel 1 (
    echo failed to build glaux-mcp 1>&2
    exit /b 1
)

"%CARGO_TARGET_DIR%\release\glaux-mcp.exe" %*
exit /b %errorlevel%
