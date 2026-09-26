; Inno Setup script — Tulipix Windows installer.
; Compiled in CI:  iscc /DAppVersion=<ver> /DDistDir=..\..\dist packaging\windows\tulipix.iss
; DistDir is the Flutter Release folder: tulipix.exe, its DLLs, data\, and
; resources\bin\windows-x86_64\**

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef DistDir
  #define DistDir "..\..\dist"
#endif

[Setup]
AppId={{7B2A3C1E-9F41-4E7B-A6D0-tulipix00001}
AppName=Tulipix
AppVersion={#AppVersion}
AppPublisher=Tulipix
DefaultDirName={autopf}\Tulipix
DefaultGroupName=Tulipix
DisableProgramGroupPage=yes
OutputBaseFilename=tulipix-{#AppVersion}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\tulipix.exe
; The setup wizard's own icon (relative to this script's dir) so the installer
; exe isn't the generic Inno icon. Same .ico the app exe embeds.
SetupIconFile=..\..\app_flutter\windows\runner\resources\app_icon.ico
WizardStyle=modern

[Files]
Source: "{#DistDir}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Tulipix"; Filename: "{app}\tulipix.exe"
Name: "{autodesktop}\Tulipix"; Filename: "{app}\tulipix.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Run]
Filename: "{app}\tulipix.exe"; Description: "{cm:LaunchProgram,Tulipix}"; Flags: nowait postinstall skipifsilent
