; The Windows installer (Inno Setup 6): installs for this user, without
; administrator rights, in %LOCALAPPDATA%\Programs\Anvil, with a Start menu
; entry and an uninstaller that also removes start-at-login, what the
; notifications registered, and the app's files (the signed-in session). Anvil offers to open tel:, sip:, sips: and
; callto: links (Windows' default apps), and takes sip:, sips: and callto:
; where nothing else does.
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
; The running app's mutex (windows/runner/instance.cpp): an install by hand
; asks for Anvil to be closed first. The app updating itself waits for it to
; go before the setup starts (crates/anvil-update).
AppMutex=Anvil.Softphone

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#Bundle}\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
; The notifications' app id (lib/src/desktop/native_shell.dart), so a call's
; notification is the Start menu's Anvil.
Name: "{autoprograms}\Anvil"; Filename: "{app}\anvil.exe"; AppUserModelID: "Anvil.Softphone"
Name: "{autodesktop}\Anvil"; Filename: "{app}\anvil.exe"; AppUserModelID: "Anvil.Softphone"; Tasks: desktopicon

[Registry]
; Start at login (launch_at_startup) writes these values; they go with the app.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "Anvil"; Flags: dontcreatekey uninsdeletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"; ValueName: "Anvil"; Flags: dontcreatekey uninsdeletevalue
; The notifications register the app under its app id; those go too.
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\Anvil.Softphone"; Flags: dontcreatekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\PushNotifications\Backup\Anvil.Softphone"; Flags: dontcreatekey uninsdeletekey
; Links to call: Anvil.Url opens them (a second start hands the link to the
; running app); Windows offers Anvil for each scheme in its default apps.
Root: HKCU; Subkey: "Software\Classes\Anvil.Url"; ValueType: string; ValueData: "URL:Call with Anvil"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Anvil.Url"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Anvil.Url\DefaultIcon"; ValueType: string; ValueData: "{app}\anvil.exe,0"
Root: HKCU; Subkey: "Software\Classes\Anvil.Url\shell\open\command"; ValueType: string; ValueData: """{app}\anvil.exe"" ""%1"""
; Before Capabilities: the uninstaller undoes entries last to first, so the
; parent is checked for emptiness once Capabilities is gone.
Root: HKCU; Subkey: "Software\Anvil"; Flags: uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Anvil\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "Anvil"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Anvil\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Calls through your phone system"
Root: HKCU; Subkey: "Software\Anvil\Capabilities\URLAssociations"; ValueType: string; ValueName: "tel"; ValueData: "Anvil.Url"
Root: HKCU; Subkey: "Software\Anvil\Capabilities\URLAssociations"; ValueType: string; ValueName: "sip"; ValueData: "Anvil.Url"
Root: HKCU; Subkey: "Software\Anvil\Capabilities\URLAssociations"; ValueType: string; ValueName: "sips"; ValueData: "Anvil.Url"
Root: HKCU; Subkey: "Software\Anvil\Capabilities\URLAssociations"; ValueType: string; ValueName: "callto"; ValueData: "Anvil.Url"
Root: HKCU; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "Anvil"; ValueData: "Software\Anvil\Capabilities"; Flags: uninsdeletevalue
; A scheme nothing on this machine opens is Anvil's outright (tel: always
; has Windows' own handler, so it is chosen in the default apps).
Root: HKCU; Subkey: "Software\Classes\sip"; ValueType: string; ValueData: "URL:SIP"; Flags: uninsdeletekey; Check: Unclaimed('sip')
Root: HKCU; Subkey: "Software\Classes\sip"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Check: Unclaimed('sip')
Root: HKCU; Subkey: "Software\Classes\sip\shell\open\command"; ValueType: string; ValueData: """{app}\anvil.exe"" ""%1"""; Check: Unclaimed('sip')
Root: HKCU; Subkey: "Software\Classes\sips"; ValueType: string; ValueData: "URL:SIPS"; Flags: uninsdeletekey; Check: Unclaimed('sips')
Root: HKCU; Subkey: "Software\Classes\sips"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Check: Unclaimed('sips')
Root: HKCU; Subkey: "Software\Classes\sips\shell\open\command"; ValueType: string; ValueData: """{app}\anvil.exe"" ""%1"""; Check: Unclaimed('sips')
Root: HKCU; Subkey: "Software\Classes\callto"; ValueType: string; ValueData: "URL:callto"; Flags: uninsdeletekey; Check: Unclaimed('callto')
Root: HKCU; Subkey: "Software\Classes\callto"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Check: Unclaimed('callto')
Root: HKCU; Subkey: "Software\Classes\callto\shell\open\command"; ValueType: string; ValueData: """{app}\anvil.exe"" ""%1"""; Check: Unclaimed('callto')

[UninstallDelete]
; The app's files: the session (its tokens and the device's SIP password,
; sealed for the user) and settings, local since W2, roaming before.
Type: filesandordirs; Name: "{localappdata}\Anvil\Anvil"
Type: dirifempty; Name: "{localappdata}\Anvil"
Type: filesandordirs; Name: "{userappdata}\Anvil\Anvil"
Type: dirifempty; Name: "{userappdata}\Anvil"

[Run]
Filename: "{app}\anvil.exe"; Description: "{cm:LaunchProgram,Anvil}"; Flags: nowait postinstall skipifsilent
; The app updating itself runs this setup silently with /relaunch=1
; (crates/anvil-update): it starts again once installed.
Filename: "{app}\anvil.exe"; Flags: nowait; Check: Relaunch

[Code]
// Nothing opens |Scheme| but, perhaps, an earlier Anvil.
function Unclaimed(Scheme: String): Boolean;
var
  Command: String;
begin
  if not RegKeyExists(HKCR, Scheme) then
    Result := True
  else
    Result := RegQueryStringValue(HKCU, 'Software\Classes\' + Scheme + '\shell\open\command', '', Command)
      and (Pos('anvil.exe', Lowercase(Command)) > 0);
end;

function Relaunch: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:relaunch|0}') = '1');
end;
