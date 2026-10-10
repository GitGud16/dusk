; The Dusk installer (docs/ARCHITECTURE.md, "Release"), for Inno Setup 6.
; scripts\build-release.ps1 compiles it, with /DAppVersion, /DFileVersion (the version's numbers
; alone, which Windows' file versions are) and /DStage, the folder it installs: dusk.exe,
; dusq.exe, the five FFmpeg DLLs beside them, and licenses\.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef FileVersion
  #define FileVersion AppVersion
#endif
#ifndef Stage
  #define Stage "..\target\release-stage\Dusk"
#endif

[Setup]
; One id for every version, so a new version installs over the old one.
AppId={{0D3DD162-5684-4485-8AF2-CCCCC7CFE8D7}
AppName=Dusk
AppVersion={#AppVersion}
AppVerName=Dusk {#AppVersion}
AppPublisher=The Dusk contributors
AppPublisherURL=https://github.com/GitGud16/dusk
AppSupportURL=https://github.com/GitGud16/dusk/issues
AppUpdatesURL=https://github.com/GitGud16/dusk/releases
VersionInfoVersion={#FileVersion}
VersionInfoDescription=Dusk setup
; For the current user, without administrator rights (%LOCALAPPDATA%\Programs\Dusk), unless
; the user picks every user of the computer.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\Dusk
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; Solid LZMA2, which the download target needs (docs/REQUIREMENTS.md, "Download").
Compression=lzma2/ultra64
SolidCompression=yes
OutputBaseFilename=dusk-{#AppVersion}-setup
SetupIconFile=..\dusk-app\assets\dusk.ico
UninstallDisplayIcon={app}\dusk.exe
UninstallDisplayName=Dusk
WizardStyle=modern
LicenseFile=..\LICENSE
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "projects"; Description: "Open Dusk projects (.dusk files) with Dusk"; GroupDescription: "File types:"

[Files]
Source: "{#Stage}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Dusk"; Filename: "{app}\dusk.exe"
Name: "{autodesktop}\Dusk"; Filename: "{app}\dusk.exe"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\.dusk"; ValueType: string; ValueName: ""; ValueData: "Dusk.Project"; Flags: uninsdeletevalue uninsdeletekeyifempty; Tasks: projects
Root: HKA; Subkey: "Software\Classes\Dusk.Project"; ValueType: string; ValueName: ""; ValueData: "Dusk project"; Flags: uninsdeletekey; Tasks: projects
Root: HKA; Subkey: "Software\Classes\Dusk.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\dusk.exe,0"; Tasks: projects
Root: HKA; Subkey: "Software\Classes\Dusk.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\dusk.exe"" ""%1"""; Tasks: projects

[Run]
Filename: "{app}\dusk.exe"; Description: "{cm:LaunchProgram,Dusk}"; Flags: nowait postinstall skipifsilent

; Settings, shortcuts and autosaves in %APPDATA%\Dusk and %LOCALAPPDATA%\Dusk are the user's,
; so uninstalling leaves them.
