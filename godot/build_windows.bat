@echo off
rem Build the Glaux Godot extension (release), put the DLL into the demo addon,
rem and make the distributable addon (godot\dist\addons\glaux and godot\dist\glaux-godot-addon.zip).
rem For Linux / macOS use godot/build_unix.sh (both can add to the same dist).
rem Requires Rust 1.94 or later. Close the Godot editor first (it locks the DLL).
cd /d "%~dp0.."
cargo build -p glaux-godot --release || exit /b 1
if defined CARGO_TARGET_DIR (set "T=%CARGO_TARGET_DIR%") else (set "T=target")
if not exist godot\demo\addons\glaux\bin mkdir godot\demo\addons\glaux\bin
copy /Y "%T%\release\glaux_godot.dll" godot\demo\addons\glaux\bin\ >nul || exit /b 1
echo Copied to godot\demo\addons\glaux\bin\glaux_godot.dll

rem --- distributable addon ---
rem Keep other platforms' binaries in dist\addons\glaux\bin (e.g. the .so from godot/build_unix.sh)
if exist godot\dist\glaux-godot-addon.zip del /Q godot\dist\glaux-godot-addon.zip
if not exist godot\dist\addons\glaux\bin mkdir godot\dist\addons\glaux\bin
copy /Y godot\demo\addons\glaux\glaux.gdextension godot\dist\addons\glaux\ >nul || exit /b 1
copy /Y godot\demo\addons\glaux\glaux_player.png godot\dist\addons\glaux\ >nul || exit /b 1
copy /Y godot\demo\addons\glaux\plugin.cfg godot\dist\addons\glaux\ >nul || exit /b 1
copy /Y godot\demo\addons\glaux\plugin.gd godot\dist\addons\glaux\ >nul || exit /b 1
copy /Y godot\demo\addons\glaux\export_plugin.gd godot\dist\addons\glaux\ >nul || exit /b 1
copy /Y godot\demo\addons\glaux\README.md godot\dist\addons\glaux\ >nul || exit /b 1
copy /Y godot\demo\addons\glaux\AI_GUIDE.md godot\dist\addons\glaux\ >nul || exit /b 1
copy /Y godot\demo\addons\glaux\PROMPT.md godot\dist\addons\glaux\ >nul || exit /b 1
copy /Y godot\demo\addons\glaux\CHANGELOG.md godot\dist\addons\glaux\ >nul || exit /b 1
copy /Y "%T%\release\glaux_godot.dll" godot\dist\addons\glaux\bin\ >nul || exit /b 1
powershell -NoProfile -Command "Compress-Archive -Path 'godot\dist\addons' -DestinationPath 'godot\dist\glaux-godot-addon.zip' -Force" || exit /b 1
echo Made godot\dist\addons\glaux and godot\dist\glaux-godot-addon.zip
