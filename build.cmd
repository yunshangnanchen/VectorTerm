@echo off
rem =====================================================================
rem VectorTerm build entry - single source of truth.
rem build.ps1 is a thin PowerShell wrapper that forwards its args here.
rem
rem Why vcvars: VS 2026 + cc-rs 1.11 rebuild the compiler environment and
rem cl.exe fails to load c1.dll (D8050). Running vcvars64.bat and setting
rem CRATE_CC_NO_DEFAULTS=1 makes cc-rs inherit a working environment.
rem
rem This file is ASCII-only on purpose: cmd.exe decodes .cmd files with the
rem OEM codepage, so UTF-8 Chinese comments can turn into live metachars.
rem =====================================================================
setlocal EnableExtensions DisableDelayedExpansion
set "PROJ=%~dp0"
pushd "%PROJ%"
if errorlevel 1 (
  echo [build] ERROR: cannot enter project directory: %PROJ%
  exit /b 1
)

if /i "%~1"=="-h"     goto :usage
if /i "%~1"=="--help"  goto :usage
if /i "%~1"=="help"    goto :usage

rem ---------------------------------------------------------------------
rem 0. Packaging shortcut. `installer` / `package` = release build + Inno
rem    Setup. Handled before the cargo dispatch because it is not a cargo
rem    subcommand and needs its own argument handling.
rem ---------------------------------------------------------------------
if /i "%~1"=="installer" goto :installer
if /i "%~1"=="package"   goto :installer

rem ---------------------------------------------------------------------
rem 1. Cargo on PATH. Honour CARGO_HOME, otherwise the rustup default.
rem ---------------------------------------------------------------------
if defined CARGO_HOME (
  set "PATH=%CARGO_HOME%\bin;%PATH%"
) else (
  set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
)
where cargo >nul 2>&1
if errorlevel 1 (
  echo [build] ERROR: cargo.exe not found on PATH.
  echo         Install Rust with the MSVC toolchain, or set CARGO_HOME.
  goto :fail
)

rem ---------------------------------------------------------------------
rem 2. Upstream sources. Without them cargo emits an opaque path error,
rem    so bootstrap once. Set VECTORTERM_NO_SETUP=1 to handle it yourself.
rem ---------------------------------------------------------------------
if exist ".wezterm-src\term\src\lib.rs" goto :sources_ok
if defined VECTORTERM_NO_SETUP goto :sources_missing
echo [build] wezterm-term sources missing - running setup.ps1 once (needs network)...
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%PROJ%setup.ps1"
if errorlevel 1 (
  echo [build] ERROR: setup.ps1 failed. Run .\setup.ps1 or bash ./setup.sh manually.
  goto :fail
)
if not exist ".wezterm-src\term\src\lib.rs" goto :sources_missing
:sources_ok

rem ---------------------------------------------------------------------
rem 3. MSVC environment. Skip when already inside a developer prompt,
rem    or when VECTORTERM_SKIP_VCVARS=1 (e.g. CI sets up its own toolchain).
rem ---------------------------------------------------------------------
if not defined VECTORTERM_NO_CC_FIX set "CRATE_CC_NO_DEFAULTS=1"
if defined VECTORTERM_SKIP_VCVARS goto :env_ready
where cl.exe >nul 2>&1
if not errorlevel 1 (
  if defined VCToolsInstallDir (
    echo [build] reusing the active MSVC environment
    goto :env_ready
  )
)
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" (
  echo [build] ERROR: vswhere.exe not found.
  echo         Install Visual Studio Build Tools with the VC++ x64 toolset.
  goto :fail
)
set "VSDIR="
for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2^>nul`) do if not defined VSDIR set "VSDIR=%%i"
if not defined VSDIR (
  echo [build] ERROR: no Visual Studio installation with the VC++ x64 toolset.
  goto :fail
)
set "VCVARS=%VSDIR%\VC\Auxiliary\Build\vcvars64.bat"
if not exist "%VCVARS%" (
  echo [build] ERROR: vcvars64.bat not found: %VCVARS%
  goto :fail
)
call "%VCVARS%" >nul 2>&1
if errorlevel 1 (
  echo [build] ERROR: vcvars64.bat failed: %VCVARS%
  goto :fail
)
where cl.exe >nul 2>&1
if errorlevel 1 (
  echo [build] ERROR: cl.exe still not on PATH after vcvars64.bat.
  goto :fail
)
echo [build] MSVC toolset %VCToolsVersion%
:env_ready

rem ---------------------------------------------------------------------
rem 4. Dispatch. First argument may be any cargo subcommand; otherwise
rem    everything is passed to `cargo build`. REST keeps quoting intact.
rem ---------------------------------------------------------------------
set "ALL=%*"
set "SUBCMD=build"
set "REST=%ALL%"
set "SUB_GIVEN="
if not "%~1"=="" (
  for %%C in (build b test t run r check c clippy fmt bench clean doc tree fix metadata rustc add remove update) do (
    if /i "%~1"=="%%C" (
      set "SUBCMD=%%C"
      set "SUB_GIVEN=1"
      for /f "tokens=1,* delims= " %%A in ("%ALL%") do set "REST=%%B"
    )
  )
)

rem ---------------------------------------------------------------------
rem 4b. VectorTerm's own flags are not cargo flags. `cargo build --config
rem     x.lua` makes cargo parse the Lua file as TOML and abort, so route
rem     anything that looks like an application argument to
rem     `run --release -- ...` instead.
rem ---------------------------------------------------------------------
set "APP_ARGS="
if /i "%~1"=="--config" set "APP_ARGS=1"
if /i "%~1"=="--cwd" set "APP_ARGS=1"
if /i "%~1"=="--working-directory" set "APP_ARGS=1"
if "%~1"=="--" set "APP_ARGS=1"
if not defined APP_ARGS if not defined SUB_GIVEN (
  echo("%ALL%" | findstr /i /c:"--config" >nul 2>&1
  if not errorlevel 1 set "APP_ARGS=1"
)
if not defined APP_ARGS goto :dispatch_done
set "SUBCMD=run"
if "%~1"=="--" (
  set "REST=--release %ALL%"
) else (
  rem Drop a leading --release: the run below always builds release, and
  rem the binary has no use for the flag.
  set "REST=--release -- %ALL:--release =%"
)
:dispatch_done

rem Profile / target dir are only needed for the config copy. Use the
rem space form (--target-dir DIR / --profile NAME); =VALUE is not parsed.
set "PROFILE=debug"
set "TARGET_DIR=target"
if defined CARGO_TARGET_DIR  set "TARGET_DIR=%CARGO_TARGET_DIR%"
if defined VECTORTERM_TARGET_DIR set "TARGET_DIR=%VECTORTERM_TARGET_DIR%"
set "NEXT_IS_DIR="
set "NEXT_IS_PROFILE="
if not defined REST goto :scan_done
for %%F in (%REST%) do (
  if defined NEXT_IS_DIR (
    set "TARGET_DIR=%%~F"
    set "NEXT_IS_DIR="
  ) else if defined NEXT_IS_PROFILE (
    set "PROFILE=%%~F"
    set "NEXT_IS_PROFILE="
  ) else if /i "%%F"=="--target-dir" (
    set "NEXT_IS_DIR=1"
  ) else if /i "%%F"=="--profile" (
    set "NEXT_IS_PROFILE=1"
  ) else if /i "%%F"=="--release" (
    set "PROFILE=release"
  )
)
:scan_done

rem ---------------------------------------------------------------------
rem 4c. Also build the GUI config manager when we are producing an exe.
rem     This manifest is a mixed package+workspace: the root IS the
rem     `vectorterm` package, so a plain `cargo build` only builds the root
rem     package and never settings-gui. The result was that
rem     target\<profile>\ held only VectorTerm.exe, and the in-terminal
rem     /settings command (which looks for vectorterm-settings.exe next to
rem     VectorTerm.exe) silently failed - "settings won't open". Build it
rem     explicitly for build / run so both exes always sit side by side.
rem     Incremental cargo makes the extra call a no-op once it is warm.
rem ---------------------------------------------------------------------
if not "%SUBCMD%"=="build" if not "%SUBCMD%"=="b" if not "%SUBCMD%"=="run" if not "%SUBCMD%"=="r" goto :settings_prebuild_done
set "SETTINGS_ARGS="
if not "%PROFILE%"=="debug" set "SETTINGS_ARGS=--profile %PROFILE%"
if not "%TARGET_DIR%"=="target" set "SETTINGS_ARGS=%SETTINGS_ARGS% --target-dir %TARGET_DIR%"
echo [build] cargo build -p vectorterm-settings %SETTINGS_ARGS%
cargo build -p vectorterm-settings %SETTINGS_ARGS%
if errorlevel 1 (
  echo [build] ERROR: failed to build the GUI config manager vectorterm-settings.
  echo         The terminal itself is fine; check the cargo messages above.
  goto :fail
)
:settings_prebuild_done

call :now
set "T0=%NOW%"
echo [build] cargo %SUBCMD% %REST%
cargo %SUBCMD% %REST%
set "RC=%ERRORLEVEL%"
if not "%RC%"=="0" (
  echo [build] ERROR: cargo exited with code %RC%
  goto :fail
)
call :now
set /a "ELAPSED=%NOW%-%T0%"
if %ELAPSED% LSS 0 set /a "ELAPSED+=8640000"
set /a "SECS=%ELAPSED%/100"
set /a "CSECS=%ELAPSED%-%SECS%*100"
if %CSECS% LSS 10 set "CSECS=0%CSECS%"
echo [build] OK in %SECS%.%CSECS%s

rem ---------------------------------------------------------------------
rem 5. Ship the default config + built-in themes next to the exe so
rem    double-clicking works. Only for build/run, only when the target dir
rem    exists. The default config is copied only when it changed (so local
rem    edits there are not silently lost); themes are always synced because
rem    they are read-only presets shipped with the program.
rem ---------------------------------------------------------------------
if defined VECTORTERM_NO_CONFIG_COPY goto :config_done
if not "%SUBCMD%"=="build" if not "%SUBCMD%"=="b" if not "%SUBCMD%"=="run" if not "%SUBCMD%"=="r" goto :done
set "DEST_DIR=%TARGET_DIR%\%PROFILE%"
if not exist "%DEST_DIR%" goto :config_done
if not exist "%DEST_DIR%\config" mkdir "%DEST_DIR%\config" >nul 2>&1
rem 5a. Factory default config: only overwrite when it differs.
if not exist "%PROJ%config\config.lua" goto :config_themes
set "DEST=%DEST_DIR%\config\config.lua"
if exist "%DEST%" (
  fc /b "%PROJ%config\config.lua" "%DEST%" >nul 2>&1
  if not errorlevel 1 goto :config_themes
)
copy /y "%PROJ%config\config.lua" "%DEST%" >nul
if errorlevel 1 (
  echo [build] WARN: could not copy default config to %DEST%
) else (
  echo [build] default config -^> %DEST%
)

:config_themes
rem 5b. Built-in themes: every config\*.lua except config.lua itself.
rem     dir /b inside for /f keeps paths containing spaces intact.
rem     NOTE: keep this file ASCII-only, see the header comment above.
set "THEME_FOUND="
for /f "delims=" %%F in ('dir /b /a-d "%PROJ%config\*.lua" 2^>nul') do (
  if /i not "%%F"=="config.lua" (
    copy /y "%PROJ%config\%%F" "%DEST_DIR%\config\" >nul
    set "THEME_FOUND=1"
  )
)
if defined THEME_FOUND echo [build] built-in themes -^> %DEST_DIR%\config

:config_licenses
rem 5c. Font licenses: the fonts are embedded into VectorTerm.exe (include_bytes!),
rem     so SIL OFL requires the license texts next to the shipped binary.
rem     NOTE: keep this file ASCII-only, see the header comment above.
if not exist "%DEST_DIR%\licenses" mkdir "%DEST_DIR%\licenses" >nul 2>&1
copy /y "%PROJ%assets\fonts\LICENSE_*.txt" "%DEST_DIR%\licenses\" >nul
copy /y "%PROJ%assets\fonts\README.md" "%DEST_DIR%\licenses\" >nul
echo [build] font licenses -^> %DEST_DIR%\licenses
:config_done
rem ---------------------------------------------------------------------
rem 5b. Release exe must be GUI subsystem. A console-subsystem exe pops a
rem     black window when double-clicked from Explorer - exactly the "debug
rem     output window" users complain about. Report the PE header we
rem     actually produced so this never regresses silently.
rem ---------------------------------------------------------------------
if not "%PROFILE%"=="release" goto :subsys_done
set "SHIPPED=%TARGET_DIR%\%PROFILE%\VectorTerm.exe"
if not exist "%SHIPPED%" goto :subsys_done
where dumpbin >nul 2>&1
if errorlevel 1 goto :subsys_done
set "SUBSYS="
for /f "tokens=2,*" %%A in ('dumpbin /nologo /headers "%SHIPPED%" 2^>nul ^| findstr /i /c:"subsystem ("') do set "SUBSYS=%%A %%B"
if not defined SUBSYS goto :subsys_done
echo [build] PE subsystem: %SUBSYS%
echo %SUBSYS% | findstr /i /c:"Windows GUI" >nul && goto :subsys_done
echo [build] WARN: release exe is NOT a GUI-subsystem binary.
echo [build]       Double-clicking it will open a console window.
echo [build]       Check the windows_subsystem attribute in src\main.rs.
:subsys_done

if not "%SUBCMD%"=="build" if not "%SUBCMD%"=="b" if not "%SUBCMD%"=="run" if not "%SUBCMD%"=="r" goto :done
echo [build] exe: %TARGET_DIR%\%PROFILE%\VectorTerm.exe

:done
popd
exit /b 0

rem =====================================================================
rem Packaging: build release, then hand it to Inno Setup.
rem
rem   build.cmd installer                release build + setup exe
rem   build.cmd installer --no-build     reuse the existing release exe
rem   build.cmd installer --iscc PATH    point at a specific ISCC.exe
rem
rem Everything after the subcommand that is not one of the two flags above
rem is forwarded to ISCC untouched (e.g. /Q, /DMyAppVersion=...).
rem =====================================================================
:installer
rem SHIFT also shifts %0, so remember the script path before touching the
rem argument list - the release build below re-enters this file.
set "SELF=%~f0"
set "DO_BUILD=1"
set "ISCC_ARGS="
shift
:inst_arg_loop
if "%~1"=="" goto :inst_args_done
if /i "%~1"=="--no-build" (
  set "DO_BUILD=0"
  shift
  goto :inst_arg_loop
)
if /i "%~1"=="--iscc" (
  shift
  set "VECTORTERM_ISCC=%~1"
  shift
  goto :inst_arg_loop
)
set "ISCC_ARGS=%ISCC_ARGS% %~1"
shift
goto :inst_arg_loop
:inst_args_done

if not "%DO_BUILD%"=="1" goto :inst_build_done
echo [build] packaging needs the release exe - building it first
call "%SELF%" --release
if errorlevel 1 goto :fail
:inst_build_done

rem Target dir: honour CARGO_TARGET_DIR / VECTORTERM_TARGET_DIR, resolve to
rem an absolute path for ISCC (/O wants one).
set "TARGET_DIR=target"
if defined CARGO_TARGET_DIR  set "TARGET_DIR=%CARGO_TARGET_DIR%"
if defined VECTORTERM_TARGET_DIR set "TARGET_DIR=%VECTORTERM_TARGET_DIR%"
set "OUTDIR=%TARGET_DIR%\installer"
set "APPEXE=%TARGET_DIR%\release\VectorTerm.exe"
set "SETTINGSEXE=%TARGET_DIR%\release\vectorterm-settings.exe"
if not "%TARGET_DIR:~1,1%"==":" (
  set "OUTDIR=%PROJ%%TARGET_DIR%\installer"
  set "APPEXE=%PROJ%%TARGET_DIR%\release\VectorTerm.exe"
  set "SETTINGSEXE=%PROJ%%TARGET_DIR%\release\vectorterm-settings.exe"
)
if exist "%APPEXE%" goto :inst_exe_ok
echo [build] ERROR: release exe not found: %APPEXE%
echo         Run: build.cmd --release
goto :fail
:inst_exe_ok
if exist "%SETTINGSEXE%" goto :inst_settings_ok
echo [build] ERROR: settings exe not found: %SETTINGSEXE%
echo         Run: build.cmd --release (builds both vectorterm and vectorterm-settings)
goto :fail
:inst_settings_ok

rem Version shown in the installer and in Apps & features. Read it from
rem Cargo.toml so the two can never drift apart. `version = "x.y.z"` is the
rem only line starting with "version"; dependencies are written inline.
set "APPVER="
for /f "tokens=2 delims== " %%V in ('findstr /b /c:"version" "%PROJ%Cargo.toml"') do if not defined APPVER set "APPVER=%%~V"
if defined APPVER goto :inst_ver_ok
set "APPVER=0.1.0"
echo [build] WARN: could not read the version from Cargo.toml, using %APPVER%
:inst_ver_ok

rem ---------------------------------------------------------------
rem ISCC.exe lookup:
rem   env var -> PATH -> standard dirs -> other drives -> registry.
rem Inno Setup 6.5+ is required (the .iss ships a 6.5-format Chinese
rem .isl, and a 6.3-only install would fail to compile it); 7.x works.
rem The drive scan matters: Inno installs fine to D:\ or E:\Program
rem Files, and then nothing short of the registry knows about it.
rem ---------------------------------------------------------------
set "ISCC="
if defined VECTORTERM_ISCC (
  if exist "%VECTORTERM_ISCC%" set "ISCC=%VECTORTERM_ISCC%"
  if exist "%VECTORTERM_ISCC%\ISCC.exe" set "ISCC=%VECTORTERM_ISCC%\ISCC.exe"
)
if defined ISCC goto :iscc_found
for /f "delims=" %%I in ('where iscc.exe 2^>nul') do if not defined ISCC set "ISCC=%%I"
if defined ISCC goto :iscc_found
for %%V in (7 6) do (
  if not defined ISCC if exist "%ProgramFiles%\Inno Setup %%V\ISCC.exe" set "ISCC=%ProgramFiles%\Inno Setup %%V\ISCC.exe"
  if not defined ISCC if exist "%ProgramFiles(x86)%\Inno Setup %%V\ISCC.exe" set "ISCC=%ProgramFiles(x86)%\Inno Setup %%V\ISCC.exe"
  if not defined ISCC if exist "%LOCALAPPDATA%\Programs\Inno Setup %%V\ISCC.exe" set "ISCC=%LOCALAPPDATA%\Programs\Inno Setup %%V\ISCC.exe"
)
if defined ISCC goto :iscc_found
for %%R in (C D E F G H I J K) do (
  for %%V in (7 6) do (
    if not defined ISCC if exist "%%R:\Program Files\Inno Setup %%V\ISCC.exe" set "ISCC=%%R:\Program Files\Inno Setup %%V\ISCC.exe"
    if not defined ISCC if exist "%%R:\Program Files (x86)\Inno Setup %%V\ISCC.exe" set "ISCC=%%R:\Program Files (x86)\Inno Setup %%V\ISCC.exe"
  )
)
if defined ISCC goto :iscc_found
for %%K in (
  "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 7_is1"
  "HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 7_is1"
  "HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 7_is1"
  "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1"
  "HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1"
  "HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1"
) do (
  if not defined ISCC for /f "tokens=2,*" %%A in ('reg query %%K /v InstallLocation 2^>nul ^| findstr /i "InstallLocation"') do (
    if exist "%%B\ISCC.exe" set "ISCC=%%B\ISCC.exe"
  )
)
if defined ISCC goto :iscc_found
echo [build] ERROR: Inno Setup 6 (ISCC.exe) not found.
echo.
echo   Install it once, then re-run this command:
echo     winget install --id JRSoftware.InnoSetup -e
echo   or download from https://jrsoftware.org/isdl.php
echo.
echo   Already installed somewhere unusual?
echo     set VECTORTERM_ISCC=C:\path\to\ISCC.exe
echo     build.cmd installer --iscc "C:\path\to\ISCC.exe"
goto :fail
:iscc_found

echo [build] iscc:    %ISCC%
echo [build] version: %APPVER%
echo [build] output:  %OUTDIR%
rem The iss points at ..\target\release by default; only override it when
rem the target dir was changed. Note the quote placement: ISCC wants
rem   /D"MySourceExe=C:\path with spaces\VectorTerm.exe"
set SRCDEF=
if not "%TARGET_DIR%"=="target" set SRCDEF=/D"MySourceExe=%APPEXE%" /D"MySourceSettingsExe=%SETTINGSEXE%"
"%ISCC%" /Qp /DMyAppVersion=%APPVER% /O"%OUTDIR%" %SRCDEF% %ISCC_ARGS% "%PROJ%installer\VectorTerm.iss"
if errorlevel 1 (
  echo [build] ERROR: ISCC failed - see the messages above.
  goto :fail
)
if not exist "%OUTDIR%\VectorTerm-%APPVER%-setup.exe" goto :inst_no_output
echo [build] installer: %OUTDIR%\VectorTerm-%APPVER%-setup.exe
popd
exit /b 0
:inst_no_output
echo [build] WARN: ISCC reported success but %OUTDIR%\VectorTerm-%APPVER%-setup.exe is missing.
echo         Check OutputBaseFilename in installer\VectorTerm.iss.
popd
exit /b 1

:sources_missing
echo [build] ERROR: .wezterm-src\term\src\lib.rs is missing.
echo         Run .\setup.ps1 (or bash ./setup.sh) once, then build again.
goto :fail

:fail
popd
exit /b 1

:usage
echo Usage: build.cmd [cargo-subcommand] [cargo args...]
echo ---------------------------------------------------------------------
echo   build.cmd                           cargo build
echo   build.cmd --release                 cargo build --release ^(ship this one^)
echo   build.cmd test --release -- --nocapture
echo   build.cmd clippy -- -D warnings
echo   build.cmd run -- --config config\config.lua
echo   build.cmd --config config\config.lua    same as: run --release -- --config ...
echo   build.cmd clean
echo   build.cmd installer                 release build + Inno Setup package
echo   build.cmd installer --no-build      package the existing release exe
echo   build.cmd --help
echo ---------------------------------------------------------------------
echo Any cargo subcommand works as the first argument [build/test/run/check/
echo clippy/fmt/bench/clean/doc/tree/fix/metadata/rustc/...]. Everything
echo after it is passed through unchanged, including quoted paths.
echo VectorTerm's own --config/--cwd/-- are routed to `run --release -- ^<args^>`,
echo because cargo would otherwise parse the .lua file as its own TOML config.
echo From PowerShell: powershell -ExecutionPolicy Bypass -File .\build.ps1 --release
echo ---------------------------------------------------------------------
echo Packaging [installer / package]:
echo   Release exe -^> installer\VectorTerm.iss -^> target\installer\VectorTerm-^<ver^>-setup.exe
echo   Also bundles vectorterm-settings.exe (the GUI config manager, same dir).
echo   Version is read from Cargo.toml; extra args are forwarded to ISCC.
echo   The setup wizard offers "add VectorTerm to PATH" - checked by default,
echo   untick it to keep your PATH untouched. Uninstall removes the entry.
echo   Needs Inno Setup 6.3+ : winget install --id JRSoftware.InnoSetup -e
echo ---------------------------------------------------------------------
echo A successful build also copies config\config.lua next to the exe
echo [debug or release], only when the file content changed.
echo ---------------------------------------------------------------------
echo Release vs debug:
echo   release  GUI subsystem  - no console window on double-click, logs go
echo                            to the parent console or ~\.config\VectorTerm\VectorTerm.log
echo   debug    console subsystem - `cargo run` prints logs straight to the terminal
echo ---------------------------------------------------------------------
echo Environment switches:
echo   VECTORTERM_NO_SETUP=1        do not auto-run setup.ps1 for missing sources
echo   VECTORTERM_SKIP_VCVARS=1     assume the MSVC env is already initialized
echo   VECTORTERM_NO_CC_FIX=1       do not set CRATE_CC_NO_DEFAULTS=1
echo   VECTORTERM_NO_CONFIG_COPY=1  do not copy the default config
echo   VECTORTERM_TARGET_DIR=...    target dir used for the config copy
echo   CARGO_TARGET_DIR=...     standard cargo target dir (also used here)
echo   VECTORTERM_ISCC=...          ISCC.exe used by [installer] (file or its dir)
echo ---------------------------------------------------------------------
echo Use the space form --target-dir DIR / --profile NAME if you override
echo them; the --target-dir=DIR form is not detected for the config copy.
popd
exit /b 0

rem Return centiseconds-since-midnight in NOW. Leading spaces / octal-safe.
:now
set "TT=%TIME: =0%"
set "NOW=0"
for /f "tokens=1-4 delims=:.," %%a in ("%TT%") do set /a "NOW=((1%%a-100)*360000)+(1%%b-100)*6000+(1%%c-100)*100+(1%%d-100)"
exit /b 0
