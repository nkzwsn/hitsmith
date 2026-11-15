@echo off
rem Builds a release exe with the GNU toolchain. Needs rustup (stable-x86_64-pc-windows-gnu)
rem and WinLibs MinGW-w64 extracted to %USERPROFILE%\winlibs (provides as.exe for dlltool).
cd /d "%~dp0"
set "PATH=%USERPROFILE%\winlibs\mingw64\bin;%USERPROFILE%\.cargo\bin;%PATH%"
cargo build --release %*
if errorlevel 1 exit /b 1
echo.
echo Built target\release\hitsmith.exe
