; Inno Setup 6 script for the newtopo Windows installer.
; Built by packaging/windows/package.ps1, which stages the files and passes:
;   /DAppVersion=<semver>  /DNumVersion=<a.b.c.0>  /DStageDir=<dir>  /DOutputDir=<dir>

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef NumVersion
  #define NumVersion "0.0.0.0"
#endif
#ifndef StageDir
  #define StageDir "..\..\dist\.work-windows\stage"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{06BAA797-B5D7-46A7-BF4E-835B5A80B22D}
AppName=newtopo
AppVersion={#AppVersion}
AppVerName=newtopo {#AppVersion}
AppPublisher=Silas Vogt
AppPublisherURL=https://github.com/SilasVogt/newtopo
AppSupportURL=https://github.com/SilasVogt/newtopo/issues
VersionInfoVersion={#NumVersion}
DefaultDirName={autopf}\newtopo
DefaultGroupName=newtopo
DisableProgramGroupPage=yes
; Admin install to Program Files by default; the user may pick a per-user
; install instead (no admin rights needed).
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=newtopo-{#AppVersion}-windows-x64-setup
SetupIconFile=..\icons\newtopo.ico
UninstallDisplayIcon={app}\newtopo.ico
UninstallDisplayName=newtopo {#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "fileassoc"; Description: "Open .ntopo scene files with newtopo"; GroupDescription: "File associations:"

[Files]
; Executables, OCCT DLLs, the MSVC runtime DLLs, icon and licence notices.
; newtopo-import.exe must stay next to newtopo.exe and newtopo-cli.exe.
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\newtopo"; Filename: "{app}\newtopo.exe"; IconFilename: "{app}\newtopo.ico"
Name: "{autodesktop}\newtopo"; Filename: "{app}\newtopo.exe"; IconFilename: "{app}\newtopo.ico"; Tasks: desktopicon

[Registry]
; .ntopo -> newtopo.scene (HKA = HKLM for admin installs, HKCU for per-user).
Root: HKA; Subkey: "Software\Classes\.ntopo"; ValueType: string; ValueName: ""; ValueData: "newtopo.scene"; Flags: uninsdeletevalue; Tasks: fileassoc
Root: HKA; Subkey: "Software\Classes\.ntopo"; ValueType: string; ValueName: "Content Type"; ValueData: "application/x-newtopo-scene"; Flags: uninsdeletevalue; Tasks: fileassoc
Root: HKA; Subkey: "Software\Classes\.ntopo\OpenWithProgids"; ValueType: string; ValueName: "newtopo.scene"; ValueData: ""; Flags: uninsdeletevalue; Tasks: fileassoc
Root: HKA; Subkey: "Software\Classes\newtopo.scene"; ValueType: string; ValueName: ""; ValueData: "newtopo scene"; Flags: uninsdeletekey; Tasks: fileassoc
Root: HKA; Subkey: "Software\Classes\newtopo.scene\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\newtopo.ico,0"; Tasks: fileassoc
Root: HKA; Subkey: "Software\Classes\newtopo.scene\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\newtopo.exe"" ""%1"""; Tasks: fileassoc
Root: HKA; Subkey: "Software\Classes\Applications\newtopo.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "newtopo"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\newtopo.exe\SupportedTypes"; ValueType: string; ValueName: ".ntopo"; ValueData: ""

[Run]
Filename: "{app}\newtopo.exe"; Description: "{cm:LaunchProgram,newtopo}"; Flags: nowait postinstall skipifsilent
