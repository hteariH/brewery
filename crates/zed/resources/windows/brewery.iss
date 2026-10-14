; Brewery's installer: a Brewery-owned copy of zed.iss (zed.iss stays untouched for upstream
; merges). Built by script/brewery/bundle-windows.ps1, which passes:
;   Channel         stable | preview | nightly | dev (crates/zed/RELEASE_CHANNEL)
;   Version         crates/zed/BREWERY_VERSION, e.g. 0.1.0 or 0.1.0-rc.1
;   NumericVersion  the x.y.z part of Version (VersionInfoVersion must be numeric)
;   PayloadDir      the laid-out install tree (Brewery.exe, bin\, tools\, jre\, ...)
;   ResourcesDir    crates\zed\resources\windows
;   OutputDir, AppSetupName
;
; Per-user install into %LOCALAPPDATA%\Programs\<AppName> (PrivilegesRequired=lowest), so the
; auto-updater can replace files without UAC. In update mode (/update=true, run silently by the
; app) everything the update helper swaps goes under {app}\install\ instead; the helper's job
; list is crates/auto_update_helper/src/brewery.rs.

; AppId GUIDs are Brewery's own, one per channel: never reuse Zed's. The mutex must match
; windows_only_instance.rs: release_channel::app_identifier() + "-Instance-Mutex".
#if Channel == "stable"
  #define AppId "{{96B775F6-6B94-45D9-B9AB-17BEEACD3A33}"
  #define AppName "Brewery"
  #define AppIconName "app-icon"
  #define AppMutex "Brewery-Editor-Stable-Instance-Mutex"
  #define RegValueName "Brewery"
  #define AppUserId "Brewery.Brewery"
#elif Channel == "preview"
  #define AppId "{{2C7EBAD1-3B60-48F1-A2CE-72895E78B81E}"
  #define AppName "Brewery Preview"
  #define AppIconName "app-icon-preview"
  #define AppMutex "Brewery-Editor-Preview-Instance-Mutex"
  #define RegValueName "BreweryPreview"
  #define AppUserId "Brewery.Brewery.Preview"
#elif Channel == "nightly"
  #define AppId "{{07C74540-11BF-4067-99CB-590F4D267BA3}"
  #define AppName "Brewery Nightly"
  #define AppIconName "app-icon-nightly"
  #define AppMutex "Brewery-Editor-Nightly-Instance-Mutex"
  #define RegValueName "BreweryNightly"
  #define AppUserId "Brewery.Brewery.Nightly"
#elif Channel == "dev"
  #define AppId "{{73FE6EB6-EF68-46F5-A0DB-B6C587A97EDD}"
  #define AppName "Brewery Dev"
  #define AppIconName "app-icon-dev"
  #define AppMutex "Brewery-Editor-Dev-Instance-Mutex"
  #define RegValueName "BreweryDev"
  #define AppUserId "Brewery.Brewery.Dev"
#else
  #error Unknown Channel
#endif
; The update helper and the CLI look for exactly this name (brewery.rs APP_EXE, cli main.rs).
#define AppExeName "Brewery"

[Setup]
AppId={#AppId}
AppName={#AppName}
AppVerName={#AppName} {#Version}
AppPublisher=Brewery contributors
AppPublisherURL=https://github.com/hteariH/brewery
AppSupportURL=https://github.com/hteariH/brewery/issues
AppUpdatesURL=https://github.com/hteariH/brewery/releases
AppCopyright=Copyright 2026 Brewery contributors. Portions copyright Zed Industries, Inc.
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableReadyPage=yes
AllowNoIcons=yes
OutputDir={#OutputDir}
OutputBaseFilename={#AppSetupName}
Compression=lzma2
SolidCompression=yes
LZMAUseSeparateProcess=yes
AppMutex={code:GetAppMutex}
SetupMutex={#AppMutex}Setup
SetupIconFile={#ResourcesDir}\{#AppIconName}.ico
UninstallDisplayIcon={app}\{#AppExeName}.exe
UninstallDisplayName={#AppName}
ChangesEnvironment=true
ChangesAssociations=true
MinVersion=10.0.16299
SourceDir={#PayloadDir}
AppVersion={#Version}
VersionInfoVersion={#NumericVersion}
VersionInfoCompany=Brewery contributors
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
ShowLanguageDialog=no
WizardStyle=modern
; Not Zed's Terms of Service: Brewery is GPL-3.0-or-later and needs no click-through.
InfoBeforeFile={#ResourcesDir}\windows-info.txt

CloseApplications=force

; Signing hook, off: nothing signs Brewery yet (SignPath later). Set BREWERY_SIGN_BUNDLE and
; pass /sDefaultsign=... to ISCC to turn it on.
#if GetEnv("BREWERY_SIGN_BUNDLE") != ""
SignTool=Defaultsign
#endif

; {autopf} is %LOCALAPPDATA%\Programs in a non-administrative install.
DefaultDirName={autopf}\{#AppName}
PrivilegesRequired=lowest

ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl,{#ResourcesDir}\messages\en.isl"

[InstallDelete]
; A fresh install over an older one replaces the bundle directories as a whole, so files that
; a newer jdtls or JRE dropped do not linger. The app is closed (AppMutex) at this point.
Type: filesandordirs; Name: "{app}\jre"; Check: not IsUpdating
Type: filesandordirs; Name: "{app}\brewery-daemon"; Check: not IsUpdating
Type: filesandordirs; Name: "{app}\jdtls"; Check: not IsUpdating
Type: filesandordirs; Name: "{app}\extensions"; Check: not IsUpdating
Type: filesandordirs; Name: "{app}\licenses"
; Update mode: start from an empty install\ (a rolled-back update leaves the previous one).
Type: filesandordirs; Name: "{app}\install"; Check: IsUpdating

[UninstallDelete]
; Delete logs
Type: filesandordirs; Name: "{app}\tools"
Type: filesandordirs; Name: "{app}\updates"
; Delete newer files which may not have been added by the initial installation
Type: filesandordirs; Name: "{app}\x64"
Type: filesandordirs; Name: "{app}\arm64"
; The bundle, with whatever jdtls and the JRE wrote next to themselves, and update leftovers
Type: filesandordirs; Name: "{app}\jre"
Type: filesandordirs; Name: "{app}\brewery-daemon"
Type: filesandordirs; Name: "{app}\jdtls"
Type: filesandordirs; Name: "{app}\extensions"
Type: filesandordirs; Name: "{app}\licenses"
Type: filesandordirs; Name: "{app}\install"
Type: filesandordirs; Name: "{app}\old"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addcontextmenufiles"; Description: "{cm:AddContextMenuFiles,{#AppName}}"; GroupDescription: "{cm:Other}"
Name: "addcontextmenufolders"; Description: "{cm:AddContextMenuFolders,{#AppName}}"; GroupDescription: "{cm:Other}"
Name: "associatewithfiles"; Description: "{cm:AssociateWithFiles,{#AppName}}"; GroupDescription: "{cm:Other}"
Name: "addtopath"; Description: "{cm:AddToPath}"; GroupDescription: "{cm:Other}"

[Dirs]
Name: "{app}"; AfterInstall: DisableAppDirInheritance

[Files]
; What the update helper swaps: under {app}\install\ in update mode (see GetInstallDir).
Source: "{#AppExeName}.exe"; DestDir: "{code:GetInstallDir}"; Flags: ignoreversion
Source: "bin\brewery.exe"; DestDir: "{code:GetInstallDir}\bin"; Flags: ignoreversion
Source: "bin\brewery"; DestDir: "{code:GetInstallDir}\bin"; Flags: ignoreversion
Source: "conpty.dll"; DestDir: "{code:GetInstallDir}"; Flags: ignoreversion
#ifexist PayloadDir + "\x64\OpenConsole.exe"
Source: "x64\OpenConsole.exe"; DestDir: "{code:GetInstallDir}\x64"; Flags: ignoreversion
#endif
#ifexist PayloadDir + "\arm64\OpenConsole.exe"
Source: "arm64\OpenConsole.exe"; DestDir: "{code:GetInstallDir}\arm64"; Flags: ignoreversion
#endif
Source: "jre\*"; DestDir: "{code:GetInstallDir}\jre"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "brewery-daemon\*"; DestDir: "{code:GetInstallDir}\brewery-daemon"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "jdtls\*"; DestDir: "{code:GetInstallDir}\jdtls"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "extensions\*"; DestDir: "{code:GetInstallDir}\extensions"; Flags: ignoreversion recursesubdirs createallsubdirs
; Written in place in both modes: the helper is not running yet, and nothing locks the texts.
Source: "tools\auto_update_helper.exe"; DestDir: "{app}\tools"; Flags: ignoreversion
Source: "licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}.exe"; AppUserModelID: "{#AppUserId}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}.exe"; Tasks: desktopicon; AppUserModelID: "{#AppUserId}"

[Run]
Filename: "{app}\{#AppExeName}.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall; Check: WizardNotSilent

[Registry]
; File associations: Java, JVM build files, config and docs only. These are "Open with"
; entries; the user's default app for a type is not changed.
Root: HKCU; Subkey: "Software\Classes\.gradle\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.gradle\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.gradle"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.gradle"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,Gradle}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.gradle"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.gradle\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.gradle\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.gradle\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.java\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.java\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.java"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.java"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,Java}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.java"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.java\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.java\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.java\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.json\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.json\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.json"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.json"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,JSON}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.json"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.json\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.json\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.json\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.kt\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.kt\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.kt"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.kt"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,Kotlin}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.kt"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.kt\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.kt\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.kt\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.kts\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.kts\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.kts"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.kts"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,Kotlin Script}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.kts"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.kts\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.kts\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.kts\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.md"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.md"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,Markdown}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.md"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.md\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.md\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.md\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.properties\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.properties\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.properties"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.properties"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,Properties}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.properties"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.properties\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.properties\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.properties\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.sql\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.sql\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.sql"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.sql"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,SQL}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.sql"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.sql\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.sql\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.sql\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.toml\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.toml\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.toml"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.toml"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,TOML}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.toml"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.toml\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.toml\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.toml\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.xml\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.xml\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.xml"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.xml"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,XML}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.xml"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.xml\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.xml\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.xml\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.yaml\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.yaml\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.yaml"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.yaml"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,YAML}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.yaml"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.yaml\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.yaml\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.yaml\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.yml\OpenWithProgids"; ValueType: none; ValueName: "{#RegValueName}"; Flags: deletevalue uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\.yml\OpenWithProgids"; ValueType: string; ValueName: "{#RegValueName}.yml"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.yml"; ValueType: string; ValueName: ""; ValueData: "{cm:SourceFile,YAML}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.yml"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "{#AppUserId}"; Flags: uninsdeletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.yml\DefaultIcon"; ValueType: none; Flags: deletekey; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.yml\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\{#RegValueName}.yml\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: associatewithfiles
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExeName}.exe"; ValueType: none; ValueName: ""; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExeName}.exe\DefaultIcon"; ValueType: none; Flags: deletekey
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExeName}.exe\shell\open"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExeName}.exe"""
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExeName}.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""

; "Open with Brewery": classic shell verbs on every Windows version. Zed's Windows 11 sparse
; appx package is not used: it must be signed to register. On Windows 11 these entries show
; under "Show more options".
Root: HKCU; Subkey: "Software\Classes\*\shell\{#RegValueName}"; ValueType: expandsz; ValueName: ""; ValueData: "{cm:OpenWithContextMenu,{#AppName}}"; Tasks: addcontextmenufiles; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\*\shell\{#RegValueName}"; ValueType: expandsz; ValueName: "Icon"; ValueData: "{app}\{#AppExeName}.exe"; Tasks: addcontextmenufiles
Root: HKCU; Subkey: "Software\Classes\*\shell\{#RegValueName}\command"; ValueType: expandsz; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%1"""; Tasks: addcontextmenufiles
Root: HKCU; Subkey: "Software\Classes\directory\shell\{#RegValueName}"; ValueType: expandsz; ValueName: ""; ValueData: "{cm:OpenWithContextMenu,{#AppName}}"; Tasks: addcontextmenufolders; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\directory\shell\{#RegValueName}"; ValueType: expandsz; ValueName: "Icon"; ValueData: "{app}\{#AppExeName}.exe"; Tasks: addcontextmenufolders
Root: HKCU; Subkey: "Software\Classes\directory\shell\{#RegValueName}\command"; ValueType: expandsz; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%V"""; Tasks: addcontextmenufolders
Root: HKCU; Subkey: "Software\Classes\directory\background\shell\{#RegValueName}"; ValueType: expandsz; ValueName: ""; ValueData: "{cm:OpenWithContextMenu,{#AppName}}"; Tasks: addcontextmenufolders; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\directory\background\shell\{#RegValueName}"; ValueType: expandsz; ValueName: "Icon"; ValueData: "{app}\{#AppExeName}.exe"; Tasks: addcontextmenufolders
Root: HKCU; Subkey: "Software\Classes\directory\background\shell\{#RegValueName}\command"; ValueType: expandsz; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%V"""; Tasks: addcontextmenufolders
Root: HKCU; Subkey: "Software\Classes\Drive\shell\{#RegValueName}"; ValueType: expandsz; ValueName: ""; ValueData: "{cm:OpenWithContextMenu,{#AppName}}"; Tasks: addcontextmenufolders; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Drive\shell\{#RegValueName}"; ValueType: expandsz; ValueName: "Icon"; ValueData: "{app}\{#AppExeName}.exe"; Tasks: addcontextmenufolders
Root: HKCU; Subkey: "Software\Classes\Drive\shell\{#RegValueName}\command"; ValueType: expandsz; ValueName: ""; ValueData: """{app}\{#AppExeName}.exe"" ""%V"""; Tasks: addcontextmenufolders

; Environment: bin\ holds the CLI, so `brewery` works in a new terminal.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{code:AddToPath|{app}\bin}"; Tasks: addtopath; Check: NeedsAddToPath(ExpandConstant('{app}\bin'))

; No URL protocol: registering zed:// would take it over from an installed Zed.

[Code]
function WizardNotSilent(): Boolean;
begin
  Result := not WizardSilent();
end;

// https://stackoverflow.com/a/23838239/261019
procedure Explode(var Dest: TArrayOfString; Text: String; Separator: String);
var
  i, p: Integer;
begin
  i := 0;
  repeat
    SetArrayLength(Dest, i+1);
    p := Pos(Separator,Text);
    if p > 0 then begin
      Dest[i] := Copy(Text, 1, p-1);
      Text := Copy(Text, p + Length(Separator), Length(Text));
      i := i + 1;
    end else begin
      Dest[i] := Text;
      Text := '';
    end;
  until Length(Text)=0;
end;

function PathContains(Path: string; Dir: string): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') > 0;
end;

function NeedsAddToPath(Dir: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', OrigPath)
  then begin
    Result := True;
    exit;
  end;
  Result := not PathContains(OrigPath, Dir);
end;

function AddToPath(Dir: string): string;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', OrigPath) then
    OrigPath := '';

  if Length(OrigPath) = 0 then
    Result := Dir
  else if OrigPath[Length(OrigPath)] = ';' then
    Result := OrigPath + Dir
  else
    Result := OrigPath + ';' + Dir;
end;

// Removes {app}\bin from the user's PATH. zed.iss tests `not CurUninstallStep = usUninstall`,
// which Pascal reads as `(not CurUninstallStep) = usUninstall`.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Path: string;
  InstalledPath: string;
  Parts: TArrayOfString;
  NewPath: string;
  i: Integer;
begin
  if CurUninstallStep <> usUninstall then begin
    exit;
  end;
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Path)
  then begin
    exit;
  end;
  InstalledPath := ExpandConstant('{app}\bin');
  if not PathContains(Path, InstalledPath) then begin
    exit;
  end;
  NewPath := '';
  Explode(Parts, Path, ';');
  for i:=0 to GetArrayLength(Parts)-1 do begin
    if (Parts[i] <> '') and (CompareText(Parts[i], InstalledPath) <> 0) then begin
      if NewPath <> '' then begin
        NewPath := NewPath + ';';
      end;
      NewPath := NewPath + Parts[i];
    end;
  end;
  RegWriteExpandStringValue(HKCU, 'Environment', 'Path', NewPath);
end;

// https://docs.microsoft.com/en-us/windows-server/administration/windows-commands/icacls
// https://docs.microsoft.com/en-US/windows/security/identity-protection/access-control/security-identifiers
procedure DisableAppDirInheritance();
var
  ResultCode: Integer;
  Permissions: string;
begin
  Permissions := '/grant:r "*S-1-5-18:(OI)(CI)F" /grant:r "*S-1-5-32-544:(OI)(CI)F" /grant:r "*S-1-5-11:(OI)(CI)RX" /grant:r "*S-1-5-32-545:(OI)(CI)RX"';

  Permissions := Permissions + Format(' /grant:r "*S-1-3-0:(OI)(CI)F" /grant:r "%s:(OI)(CI)F"', [GetUserNameString()]);

  Exec(ExpandConstant('{sys}\icacls.exe'), ExpandConstant('"{app}" /inheritancelevel:r ') + Permissions, '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

function SwitchHasValue(Name: string; Value: string): Boolean;
begin
  Result := CompareText(ExpandConstant('{param:' + Name + '}'), Value) = 0;
end;

// The app runs the downloaded setup with /verysilent /update=true /MERGETASKS=!desktopicon.
function IsUpdating(): Boolean;
begin
  Result := SwitchHasValue('update', 'true') and WizardSilent();
end;

// updates\versions.txt is the "ready" flag: the app runs the update helper on quit only when
// it exists, so it is written last, once every file is in place.
procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    if IsUpdating() then
    begin
      // The app downloads the setup into updates\, but do not depend on that.
      ForceDirectories(ExpandConstant('{app}\updates'));
      SaveStringToFile(ExpandConstant('{app}\updates\versions.txt'), '{#Version}' + #13#10, True);
    end
  end;
end;

// While updating, the app is still running: files go to install\ and nothing is closed.
function GetAppMutex(Param: string): string;
begin
  if IsUpdating() then
    Result := ''
  else
    Result := '{#AppMutex}';
end;

function GetInstallDir(Param: string): string;
begin
  if IsUpdating() then
    Result := ExpandConstant('{app}\install')
  else
    Result := ExpandConstant('{app}');
end;
