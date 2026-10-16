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
rem   Gradle     %JAVA_HOME% (see brewery.gradle.java.home below)
rem   heap       -Xms%BREWERY_JDTLS_MIN_MEMORY% (else 1G) -Xmx%BREWERY_JDTLS_MAX_MEMORY%
rem   config     shared, read-only: <bundle>\jdtls\config_win
rem              writable: <cache>\config\<sha1 of this jdtls directory>
rem   workspace  -data when given, else <cache>\workspace\jdtls-<sha1 of the cwd>;
rem              rebuilt when this jdtls changes (see the stamp below)
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

set "JAVA_FROM_HOME="
set "JAVA=%BUNDLE_ROOT%\jre\bin\java.exe"
if exist "%JAVA%" goto java_found
set "JAVA=java"
if not defined JAVA_HOME goto java_found
if not exist "%JAVA_HOME%\bin\java.exe" goto java_found
set "JAVA=%JAVA_HOME%\bin\java.exe"
set "JAVA_FROM_HOME=1"
:java_found

rem Gradle (jdtls's import and builds) runs on the user's JDK, as gradlew does:
rem JAVA_HOME reaches jdtls as brewery.gradle.java.home, which Brewery's
rem lsp.jdtls.initialization_options use for java.import.gradle.java.home. It is
rem empty without a usable JAVA_HOME, when jdtls runs on it (then it is jdtls's
rem default JVM anyway) or when the project's Gradle wrapper cannot run on it;
rem jdtls then picks a JDK that wrapper can run on.
set "GRADLE_JAVA_HOME="
if defined JAVA_FROM_HOME goto gradle_java_home_done
if not defined JAVA_HOME goto gradle_java_home_done
set "GRADLE_JAVA_HOME=%JAVA_HOME:"=%"
rem A trailing backslash would escape the quote that closes the JVM argument.
if "%GRADLE_JAVA_HOME:~-1%"=="\" set "GRADLE_JAVA_HOME=%GRADLE_JAVA_HOME:~0,-1%"
if not exist "%GRADLE_JAVA_HOME%\bin\java.exe" goto no_gradle_java_home
rem The JDK's feature version from its release file (1.8.0_x is 8), the wrapper's
rem Gradle version as major*100+minor; either unknown (0): JAVA_HOME as it is.
set "JAVA_VERSION_VALUE="
set "JAVA_FEATURE=0"
set "GRADLE_NUMBER=0"
if not exist "%GRADLE_JAVA_HOME%\release" goto java_feature_done
for /f "usebackq tokens=1,* delims==" %%A in ("%GRADLE_JAVA_HOME%\release") do if "%%A"=="JAVA_VERSION" set "JAVA_VERSION_VALUE=%%~B"
if not defined JAVA_VERSION_VALUE goto java_feature_done
for /f "tokens=1,2 delims=._+-" %%A in ("%JAVA_VERSION_VALUE%") do set /a "JAVA_FEATURE=%%A" 2>nul & if "%%A"=="1" set /a "JAVA_FEATURE=%%B" 2>nul
:java_feature_done
set "WRAPPER_PROPERTIES=%CD%\gradle\wrapper\gradle-wrapper.properties"
set "DIST_URL="
if not exist "%WRAPPER_PROPERTIES%" goto gradle_number_done
for /f "usebackq tokens=1,* delims==" %%A in ("%WRAPPER_PROPERTIES%") do if "%%A"=="distributionUrl" set "DIST_URL=%%B"
if not defined DIST_URL goto gradle_number_done
rem .../gradle-8.10.2-bin.zip: the file name, then the version after "gradle-".
for %%F in ("%DIST_URL:/=\%") do set "DIST_NAME=%%~nF"
for /f "tokens=2 delims=-" %%V in ("%DIST_NAME%") do set "GRADLE_VERSION=%%V"
for /f "tokens=1,2 delims=." %%A in ("%GRADLE_VERSION%") do set /a "GRADLE_NUMBER=%%A*100+%%B" 2>nul
:gradle_number_done
if %JAVA_FEATURE% EQU 0 goto gradle_java_home_done
if %GRADLE_NUMBER% EQU 0 goto gradle_java_home_done
rem The oldest Gradle that runs on each Java (jdtls's GradleUtils.getHighestSupportedJava;
rem https://docs.gradle.org/current/userguide/compatibility.html); Gradle 9 needs Java 17.
set "MIN_GRADLE=0"
if %JAVA_FEATURE% GEQ 9 set "MIN_GRADLE=403"
if %JAVA_FEATURE% GEQ 10 set "MIN_GRADLE=407"
if %JAVA_FEATURE% GEQ 11 set "MIN_GRADLE=500"
if %JAVA_FEATURE% GEQ 12 set "MIN_GRADLE=504"
if %JAVA_FEATURE% GEQ 13 set "MIN_GRADLE=600"
if %JAVA_FEATURE% GEQ 14 set "MIN_GRADLE=603"
if %JAVA_FEATURE% GEQ 15 set "MIN_GRADLE=607"
if %JAVA_FEATURE% GEQ 16 set "MIN_GRADLE=700"
if %JAVA_FEATURE% GEQ 17 set "MIN_GRADLE=703"
if %JAVA_FEATURE% GEQ 18 set "MIN_GRADLE=705"
if %JAVA_FEATURE% GEQ 19 set "MIN_GRADLE=706"
if %JAVA_FEATURE% GEQ 20 set "MIN_GRADLE=803"
if %JAVA_FEATURE% GEQ 21 set "MIN_GRADLE=805"
if %JAVA_FEATURE% GEQ 22 set "MIN_GRADLE=808"
if %JAVA_FEATURE% GEQ 23 set "MIN_GRADLE=810"
if %JAVA_FEATURE% GEQ 24 set "MIN_GRADLE=814"
if %JAVA_FEATURE% GEQ 25 set "MIN_GRADLE=901"
if %JAVA_FEATURE% GEQ 26 set "MIN_GRADLE=904"
if %JAVA_FEATURE% GEQ 27 goto no_gradle_java_home
if %GRADLE_NUMBER% LSS %MIN_GRADLE% goto no_gradle_java_home
if %JAVA_FEATURE% LSS 17 if %GRADLE_NUMBER% GEQ 900 goto no_gradle_java_home
goto gradle_java_home_done
:no_gradle_java_home
set "GRADLE_JAVA_HOME="
:gradle_java_home_done

rem The heap: the extension passes lsp.jdtls.settings.min_memory / max_memory only to
rem a jdtls it starts itself, so Brewery hands them over in lsp.jdtls.binary.env.
rem --jvm-arg options come later and win.
set "MIN_MEMORY=1G"
if defined BREWERY_JDTLS_MIN_MEMORY set "MIN_MEMORY=%BREWERY_JDTLS_MIN_MEMORY%"
set JVM_MEMORY="-Xms%MIN_MEMORY%"
if defined BREWERY_JDTLS_MAX_MEMORY set JVM_MEMORY=%JVM_MEMORY% "-Xmx%BREWERY_JDTLS_MAX_MEMORY%"

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

rem A workspace remembers paths into the configuration area: Gradle projects keep
rem "--init-script <config>\org.eclipse.osgi\<bundle id>\0\.cp\gradle\init\init.gradle"
rem in their Buildship settings. When this jdtls is replaced (a Brewery update),
rem Equinox reinstalls its bundles under new ids and those paths stop existing, so
rem Gradle imports fail until the workspace is rebuilt. The stamp names this jdtls;
rem a workspace from another one (or from before the stamp) is moved away and
rem deleted, and jdtls imports the project again. If it is in use (locked), it is
rem kept and the stamp is not written, so the next start tries again.
set "STAMP=%CONFIG_AREA%"
for %%F in ("%SHARED_CONFIG%\config.ini") do set "STAMP=%STAMP%|%%~zF|%%~tF"
for %%F in ("%JDTLS_HOME%\plugins\org.eclipse.jdt.ls.core_*.jar") do set "STAMP=%STAMP%|%%~nxF|%%~zF|%%~tF"
set "STAMP_FILE=%DATA%\.metadata\brewery-jdtls.stamp"
set "OLD_STAMP="
if exist "%STAMP_FILE%" set /p OLD_STAMP=<"%STAMP_FILE%"
if "%OLD_STAMP%"=="%STAMP%" goto workspace_ready
if not exist "%DATA%\.metadata" goto write_stamp
for %%D in ("%DATA%") do set "STALE_NAME=%%~nxD.stale-%RANDOM%%RANDOM%"
if "%STALE_NAME:~0,7%"==".stale-" goto workspace_ready
for %%D in ("%DATA%") do set "STALE=%%~dpD%STALE_NAME%"
ren "%DATA%" "%STALE_NAME%" >nul 2>&1 || goto workspace_ready
rd /s /q "%STALE%" >nul 2>&1
:write_stamp
if not exist "%DATA%\.metadata" mkdir "%DATA%\.metadata" >nul 2>&1
<nul set /p "=%STAMP%" >"%STAMP_FILE%" 2>nul
:workspace_ready

"%JAVA%" -Declipse.application=org.eclipse.jdt.ls.core.id1 -Dosgi.bundles.defaultStartLevel=4 -Declipse.product=org.eclipse.jdt.ls.core.product -Dosgi.checkConfiguration=true "-Dosgi.sharedConfiguration.area=%SHARED_CONFIG%" -Dosgi.sharedConfiguration.area.readOnly=true -Dosgi.configuration.cascaded=true "-Dosgi.configuration.area=%CONFIG_AREA%" -Djava.import.generatesMetadataFilesAtProjectRoot=false -Djava.net.preferIPv4Stack=true "-Dbrewery.gradle.java.home=%GRADLE_JAVA_HOME%" %JVM_MEMORY% --add-modules=ALL-SYSTEM --add-opens java.base/java.util=ALL-UNNAMED --add-opens java.base/java.lang=ALL-UNNAMED %JVM_ARGS% -jar "%LAUNCHER%" -data "%DATA%" %JDTLS_ARGS%
exit /b %ERRORLEVEL%
