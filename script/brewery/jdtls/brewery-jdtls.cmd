@echo off
rem Brewery's jdtls launcher for Windows. Starts the Eclipse JDT Language Server
rem of this bundle without Python (jdtls's own bin\jdtls.bat needs it).
rem
rem Brewery sets it as lsp.jdtls.settings.jdtls_launcher. The Java extension
rem (6.8.28, src/jdtls_server.rs) then runs it through java-lsp-proxy as
rem     cmd /C brewery-jdtls.cmd [--jvm-arg=<JVM option>]... [-data <workspace>]
rem and speaks LSP over stdin/stdout, so only jdtls may write to stdout.
rem The JVM arguments match the extension's own build_jdtls_launch_args.
rem
rem   java       <bundle>\jre\bin\java.exe, else %JAVA_HOME%\bin\java.exe, else PATH
rem   config     shared, read-only: <bundle>\jdtls\config_win
rem              writable: <cache>\config\<sha1 of this jdtls directory>
rem   workspace  -data when given, else <cache>\workspace\jdtls-<sha1 of the cwd>
rem   cache      %BREWERY_JDTLS_CACHE_DIR%, else %LOCALAPPDATA%\Brewery\jdtls
rem
rem cmd.exe splits unquoted arguments at "=", so --jvm-arg=X arrives either as
rem one quoted argument or as "--jvm-arg" "X"; both work. A JVM option that
rem itself contains "=" and no space must therefore be passed as --jvm-arg "X".
rem Every other argument goes to jdtls after -data.
rem
rem The bundled copy has CRLF line endings (the fetcher converts this LF file).
setlocal EnableExtensions DisableDelayedExpansion

set "JDTLS_HOME=%~dp0"
set "JDTLS_HOME=%JDTLS_HOME:~0,-1%"
for %%I in ("%JDTLS_HOME%\..") do set "BUNDLE_ROOT=%%~fI"

set "JAVA=%BUNDLE_ROOT%\jre\bin\java.exe"
if exist "%JAVA%" goto java_found
set "JAVA=java"
if not defined JAVA_HOME goto java_found
if exist "%JAVA_HOME%\bin\java.exe" set "JAVA=%JAVA_HOME%\bin\java.exe"
:java_found

set "LAUNCHER="
if exist "%JDTLS_HOME%\plugins\org.eclipse.equinox.launcher.jar" set "LAUNCHER=%JDTLS_HOME%\plugins\org.eclipse.equinox.launcher.jar"
if defined LAUNCHER goto launcher_found
for %%F in ("%JDTLS_HOME%\plugins\org.eclipse.equinox.launcher_*.jar") do set "LAUNCHER=%%~fF"
if defined LAUNCHER goto launcher_found
>&2 echo brewery-jdtls: no org.eclipse.equinox.launcher jar in "%JDTLS_HOME%\plugins"
exit /b 1
:launcher_found

set "SHARED_CONFIG=%JDTLS_HOME%\config_win"
if exist "%SHARED_CONFIG%\config.ini" goto config_found
>&2 echo brewery-jdtls: "%SHARED_CONFIG%\config.ini" not found
exit /b 1
:config_found

set "JVM_ARGS="
set "JDTLS_ARGS="
set "DATA="
:next_arg
if "%~1"=="" goto args_done
set "ARG=%~1"
if "%ARG%"=="--jvm-arg" goto jvm_arg_split
if "%ARG:~0,10%"=="--jvm-arg=" goto jvm_arg_joined
if "%ARG%"=="-data" goto data_arg
set JDTLS_ARGS=%JDTLS_ARGS% %1
shift
goto next_arg
:jvm_arg_split
if "%~2"=="" goto missing_value
set JVM_ARGS=%JVM_ARGS% "%~2"
shift
shift
goto next_arg
:jvm_arg_joined
set JVM_ARGS=%JVM_ARGS% "%ARG:~10%"
shift
goto next_arg
:data_arg
if "%~2"=="" goto missing_value
set "DATA=%~2"
shift
shift
goto next_arg
:missing_value
>&2 echo brewery-jdtls: %1 needs a value
exit /b 2
:args_done

set "CACHE=%BREWERY_JDTLS_CACHE_DIR%"
if defined CACHE goto cache_found
set "CACHE=%TEMP%\Brewery\jdtls"
if defined LOCALAPPDATA set "CACHE=%LOCALAPPDATA%\Brewery\jdtls"
:cache_found

rem SHA-1 keys like the extension's jdtls-<sha1> workspace names. PowerShell ships
rem with Windows; without it the keys fall back to the sanitized paths.
set "BREWERY_JDTLS_KEY_1=%JDTLS_HOME%"
set "BREWERY_JDTLS_KEY_2=%CD%"
set "CONFIG_KEY="
set "DATA_KEY="
for /f "usebackq tokens=1,2" %%A in (`powershell.exe -NoLogo -NoProfile -NonInteractive -Command "$s = [Security.Cryptography.SHA1]::Create(); $u = [Text.Encoding]::UTF8; [BitConverter]::ToString($s.ComputeHash($u.GetBytes($env:BREWERY_JDTLS_KEY_1))).Replace('-', '').ToLower() + ' ' + [BitConverter]::ToString($s.ComputeHash($u.GetBytes($env:BREWERY_JDTLS_KEY_2))).Replace('-', '').ToLower()" 2^>nul`) do (
    set "CONFIG_KEY=%%A"
    set "DATA_KEY=%%B"
)
if defined DATA_KEY goto keys_found
set "CONFIG_KEY=%JDTLS_HOME::=%"
set "CONFIG_KEY=%CONFIG_KEY:\=_%"
set "CONFIG_KEY=%CONFIG_KEY: =_%"
set "DATA_KEY=%CD::=%"
set "DATA_KEY=%DATA_KEY:\=_%"
set "DATA_KEY=%DATA_KEY: =_%"
:keys_found
set "CONFIG_AREA=%CACHE%\config\%CONFIG_KEY%"
if not defined DATA set "DATA=%CACHE%\workspace\jdtls-%DATA_KEY%"

"%JAVA%" -Declipse.application=org.eclipse.jdt.ls.core.id1 -Dosgi.bundles.defaultStartLevel=4 -Declipse.product=org.eclipse.jdt.ls.core.product -Dosgi.checkConfiguration=true "-Dosgi.sharedConfiguration.area=%SHARED_CONFIG%" -Dosgi.sharedConfiguration.area.readOnly=true -Dosgi.configuration.cascaded=true "-Dosgi.configuration.area=%CONFIG_AREA%" -Djava.import.generatesMetadataFilesAtProjectRoot=false -Djava.net.preferIPv4Stack=true -Xms1G --add-modules=ALL-SYSTEM --add-opens java.base/java.util=ALL-UNNAMED --add-opens java.base/java.lang=ALL-UNNAMED %JVM_ARGS% -jar "%LAUNCHER%" -data "%DATA%" %JDTLS_ARGS%
exit /b %ERRORLEVEL%
