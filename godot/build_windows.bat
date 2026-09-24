@echo off
rem Build the Glaux Godot extension (release) and copy the DLL into the demo addon.
rem Requires Rust 1.94 or later.
cd /d "%~dp0.."
cargo build -p glaux-godot --release || exit /b 1
if defined CARGO_TARGET_DIR (set "T=%CARGO_TARGET_DIR%") else (set "T=target")
if not exist godot\demo\addons\glaux\bin mkdir godot\demo\addons\glaux\bin
copy /Y "%T%\release\glaux_godot.dll" godot\demo\addons\glaux\bin\ >nul || exit /b 1
echo Copied to godot\demo\addons\glaux\bin\glaux_godot.dll
