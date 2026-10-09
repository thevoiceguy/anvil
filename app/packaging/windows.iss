; The Windows installer (Inno Setup 6): installs for this user, without
; administrator rights, in %LOCALAPPDATA%\Programs\Anvil, with a Start menu
; entry and an uninstaller that also removes start-at-login.
;
;   iscc /DVersion=0.1.0 /DLabel=v0.1.0 /DBundle=<release dir> /DOut=<dir> windows.iss

#ifndef Version
  #error Version is required
#endif

[Setup]
AppId={{B3F0D2A4-5C1E-4E8B-9A7D-3F6C2E1B0A95}
AppName=Anvil
AppVersion={#Version}
AppVerName=Anvil {#Version}
AppPublisher=Anvil
AppPublisherURL=https://github.com/thevoiceguy/anvil
DefaultDirName={localappdata}\Programs\Anvil
DefaultGroupName=Anvil
DisableProgramGroupPage=yes
DisableDirPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#Out}
OutputBaseFilename=Anvil-{#Label}-windows-x86_64-setup
SetupIconFile=..\windows\runner\resources\app_icon.ico
UninstallDisplayIcon={app}\anvil.exe
UninstallDisplayName=Anvil
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#Bundle}\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{autoprograms}\Anvil"; Filename: "{app}\anvil.exe"
Name: "{autodesktop}\Anvil"; Filename: "{app}\anvil.exe"; Tasks: desktopicon

[Registry]
; Start at login (launch_at_startup) writes this value; it goes with the app.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "Anvil"; Flags: dontcreatekey uninsdeletevalue

[Run]
Filename: "{app}\anvil.exe"; Description: "{cm:LaunchProgram,Anvil}"; Flags: nowait postinstall skipifsilent
; The app updating itself runs this setup silently with /relaunch=1
; (crates/anvil-update): it starts again once installed.
Filename: "{app}\anvil.exe"; Flags: nowait; Check: Relaunch

[Code]
function Relaunch: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:relaunch|0}') = '1');
end;
