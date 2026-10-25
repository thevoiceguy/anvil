; The Windows installer (Inno Setup 6): installs for this user, without
; administrator rights, in %LOCALAPPDATA%\Programs\Anvil, with a Start menu
; entry and an uninstaller that also removes start-at-login, what the
; notifications registered, and the app's files (the signed-in session). Anvil offers to open tel:, sip:, sips: and
; callto: links (Windows' default apps), and takes sip:, sips: and callto:
; where nothing else does.
;
;   iscc /DVersion=0.1.0 /DLabel=v0.1.0 /DBundle=<release dir> /DOut=<dir> windows.iss
;
; A company's profile (docs/MANAGED_INSTALLS.md §2.3): the setup takes
; /PROFILE=<directory, .zip or https URL> and the single fields /SERVER=,
; /LOGINDOMAIN=, /SIGNIN= and /LOCKSERVER=, puts the profile in
; {app}\profile (profile.ps1), and names the shortcuts, the notifications'
; app id and the uninstaller after its brand. A setup run without them, as
; the app updating itself runs it, keeps the profile already there. A
; profile that cannot be read fails the install (exit code 7).
;
; The app renames the shortcuts listed in {app}\profile\shortcuts.json when
; FCP's brand differs (docs/MANAGED_INSTALLS.md §2.4) and lists them under
; their new names; the next install and the uninstaller go by that list.
;
; brand.py builds a branded setup: /DProfile=<profile .zip> inside it,
; /DName=<the brand's name> and /DOutName=<the file's name>.

#ifndef Version
  #error Version is required
#endif

[Setup]
AppId={{B3F0D2A4-5C1E-4E8B-9A7D-3F6C2E1B0A95}
#ifdef Name
AppName={#Name}
AppVerName={#Name} {#Version}
#else
AppName=Anvil
AppVerName=Anvil {#Version}
#endif
AppVersion={#Version}
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
#ifdef OutName
OutputBaseFilename={#OutName}
#else
OutputBaseFilename=Anvil-{#Label}-windows-x86_64-setup
#endif
SetupIconFile=..\windows\runner\resources\app_icon.ico
UninstallDisplayIcon={code:UninstallIcon}
UninstallDisplayName={code:BrandName}
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
; The profile step, run from {tmp} before anything is installed.
Source: "profile.ps1"; Flags: dontcopy
#ifdef Profile
Source: "{#Profile}"; DestName: "embedded-profile.zip"; Flags: dontcopy
#endif

[Icons]
; The notifications' app id (lib/src/desktop/native_shell.dart), so a call's
; notification is the Start menu's entry. Named, and with the icon, of the
; profile's brand when there is one.
Name: "{autoprograms}\{code:ShortcutName}"; Filename: "{app}\anvil.exe"; AppUserModelID: "Anvil.Softphone"; Check: not HasBrandIcon
Name: "{autoprograms}\{code:ShortcutName}"; Filename: "{app}\anvil.exe"; AppUserModelID: "Anvil.Softphone"; IconFilename: "{code:BrandIcon}"; Check: HasBrandIcon
Name: "{autodesktop}\{code:ShortcutName}"; Filename: "{app}\anvil.exe"; AppUserModelID: "Anvil.Softphone"; Tasks: desktopicon; Check: not HasBrandIcon
Name: "{autodesktop}\{code:ShortcutName}"; Filename: "{app}\anvil.exe"; AppUserModelID: "Anvil.Softphone"; IconFilename: "{code:BrandIcon}"; Tasks: desktopicon; Check: HasBrandIcon

[Registry]
; Start at login (launch_at_startup) writes these values; they go with the app.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "Anvil"; Flags: dontcreatekey uninsdeletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"; ValueName: "Anvil"; Flags: dontcreatekey uninsdeletevalue
; The notifications register the app under its app id; those go too. A
; brand's name and icon are registered now, so they are right before the
; app first starts (it registers them again at every start).
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\Anvil.Softphone"; Flags: dontcreatekey uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\Anvil.Softphone"; ValueType: string; ValueName: "DisplayName"; ValueData: "{code:BrandName}"; Check: HasBrandName
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\Anvil.Softphone"; ValueType: string; ValueName: "IconUri"; ValueData: "{code:BrandIcon}"; Check: HasBrandIcon
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
; The profile, which the installer put there but did not install as a file.
Type: filesandordirs; Name: "{app}\profile"
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
var
  // What profile.ps1 said: the brand's name, its file-safe form, its icon.
  ProfileName, ProfileFile, ProfileIcon: String;
  // The shortcuts the previous install made (shortcuts.json).
  MadeBefore: TStringList;

function BrandName(Param: String): String;
begin
  if ProfileName = '' then Result := 'Anvil' else Result := ProfileName;
end;

function ShortcutName(Param: String): String;
begin
  if ProfileFile = '' then Result := 'Anvil' else Result := ProfileFile;
end;

function BrandIcon(Param: String): String;
begin
  Result := ProfileIcon;
end;

function HasBrandName: Boolean;
begin
  Result := (ProfileName <> '') and (ProfileName <> 'Anvil');
end;

function HasBrandIcon: Boolean;
begin
  Result := ProfileIcon <> '';
end;

function UninstallIcon(Param: String): String;
begin
  if ProfileIcon <> '' then Result := ProfileIcon else Result := ExpandConstant('{app}\anvil.exe');
end;

// A command-line value, without quotes that would end its argument.
function Option(Name: String): String;
begin
  Result := ExpandConstant('{param:' + Name + '|}');
  StringChangeEx(Result, '"', '', True);
end;

function Quoted(Value: String): String;
begin
  Result := '"' + Value + '"';
end;

// A local /PROFILE= is taken from the current directory, else beside the
// setup (an Intune package's folder).
function ProfileSource: String;
var
  Given: String;
begin
  Given := Option('PROFILE');
  Result := Given;
  if (Given = '') or (Pos('://', Given) > 0) then Exit;
  Result := ExpandFileName(Given);
  if not FileExists(Result) and not DirExists(Result) then
    Result := AddBackslash(ExpandConstant('{src}')) + Given;
end;

// shortcuts.json, one path a line as WriteShortcuts and the app write it.
function ReadShortcuts(List: TStringList): Boolean;
var
  Lines: TArrayOfString;
  I: Integer;
  Line: String;
begin
  Result := LoadStringsFromFile(ExpandConstant('{app}\profile\shortcuts.json'), Lines);
  if not Result then Exit;
  for I := 0 to GetArrayLength(Lines) - 1 do
  begin
    Line := Trim(Lines[I]);
    if (Length(Line) > 1) and (Line[1] = '"') and (Pos('.lnk"', Lowercase(Line)) > 0) then
    begin
      Delete(Line, 1, 1);
      Line := Copy(Line, 1, Pos('"', Line) - 1);
      StringChangeEx(Line, '\\', '\', True);
      List.Add(Line);
    end;
  end;
end;

procedure ReadMadeBefore;
begin
  MadeBefore := TStringList.Create;
  if not ReadShortcuts(MadeBefore) then
  begin
    // An install from before profiles made these.
    MadeBefore.Add(ExpandConstant('{autoprograms}\Anvil.lnk'));
    MadeBefore.Add(ExpandConstant('{autodesktop}\Anvil.lnk'));
  end;
end;

// Run before anything is installed, so the shortcuts and the registry take
// the brand; a profile that cannot be read stops the install.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Args, Ini, Embedded, Value: String;
  Code: Integer;
begin
  Result := '';
  ExtractTemporaryFile('profile.ps1');
  Embedded := '';
#ifdef Profile
  ExtractTemporaryFile('embedded-profile.zip');
  Embedded := ExpandConstant('{tmp}\embedded-profile.zip');
#endif
  ReadMadeBefore;
  Ini := ExpandConstant('{tmp}\profile.ini');
  Args := '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File '
    + Quoted(ExpandConstant('{tmp}\profile.ps1'))
    + ' -App ' + Quoted(ExpandConstant('{app}')) + ' -Out ' + Quoted(Ini);
  // Only what is given: an empty quoted argument is lost on the way.
  Value := ProfileSource;
  if Value <> '' then Args := Args + ' -Source ' + Quoted(Value);
  if Embedded <> '' then Args := Args + ' -Embedded ' + Quoted(Embedded);
  Value := Option('SERVER');
  if Value <> '' then Args := Args + ' -Server ' + Quoted(Value);
  Value := Option('LOGINDOMAIN');
  if Value <> '' then Args := Args + ' -LoginDomain ' + Quoted(Value);
  Value := Option('SIGNIN');
  if Value <> '' then Args := Args + ' -SignIn ' + Quoted(Value);
  Value := Option('LOCKSERVER');
  if Value <> '' then Args := Args + ' -LockServer ' + Quoted(Value);
  Log('The profile step: powershell.exe ' + Args);
  if not Exec('powershell.exe', Args, '', SW_HIDE, ewWaitUntilTerminated, Code) then
  begin
    Result := 'The profile could not be installed: PowerShell did not start (' + SysErrorMessage(Code) + ').';
    Exit;
  end;
  Value := GetIniString('profile', 'error', '', Ini);
  if (Code <> 0) or (Value <> '') then
  begin
    if Value = '' then Value := 'exit code ' + IntToStr(Code);
    Result := 'The profile could not be installed: ' + Value;
    Exit;
  end;
  ProfileName := GetIniString('profile', 'name', '', Ini);
  ProfileFile := GetIniString('profile', 'file', '', Ini);
  ProfileIcon := GetIniString('profile', 'icon', '', Ini);
  Log('The profile: name ' + ProfileName + ', icon ' + ProfileIcon);
end;

function JsonString(Value: String): String;
begin
  StringChangeEx(Value, '\', '\\', True);
  StringChangeEx(Value, '"', '\"', True);
  Result := '"' + Value + '"';
end;

// The shortcuts this install made, for the next install and for the app
// (docs/MANAGED_INSTALLS.md §2.4); the ones a renamed brand left behind go.
procedure WriteShortcuts;
var
  Made, Candidates: TStringList;
  I: Integer;
  Lines: TArrayOfString;
begin
  Candidates := TStringList.Create;
  Candidates.Add(ExpandConstant('{autoprograms}\') + ShortcutName('') + '.lnk');
  Candidates.Add(ExpandConstant('{autodesktop}\') + ShortcutName('') + '.lnk');
  Made := TStringList.Create;
  try
    for I := 0 to Candidates.Count - 1 do
      if FileExists(Candidates[I]) then Made.Add(Candidates[I]);
    for I := 0 to MadeBefore.Count - 1 do
      if (Made.IndexOf(MadeBefore[I]) < 0) and FileExists(MadeBefore[I]) then
      begin
        Log('A shortcut the brand renamed goes: ' + MadeBefore[I]);
        DeleteFile(MadeBefore[I]);
      end;
    // UTF-8, for a brand or a user's folder beyond ASCII.
    SetArrayLength(Lines, Made.Count + 4);
    Lines[0] := '{';
    Lines[1] := '  "shortcuts": [';
    for I := 0 to Made.Count - 1 do
    begin
      Lines[I + 2] := '    ' + JsonString(Made[I]);
      if I < Made.Count - 1 then Lines[I + 2] := Lines[I + 2] + ',';
    end;
    Lines[Made.Count + 2] := '  ]';
    Lines[Made.Count + 3] := '}';
    ForceDirectories(ExpandConstant('{app}\profile'));
    SaveStringsToUTF8File(ExpandConstant('{app}\profile\shortcuts.json'), Lines, False);
  finally
    Made.Free;
    Candidates.Free;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then WriteShortcuts;
end;

// The shortcuts as the app renamed them, which the uninstall log does not
// know; before {app}\profile goes.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Made: TStringList;
  I: Integer;
begin
  if CurUninstallStep <> usUninstall then Exit;
  Made := TStringList.Create;
  try
    if ReadShortcuts(Made) then
      for I := 0 to Made.Count - 1 do
        if FileExists(Made[I]) then
        begin
          Log('A shortcut goes: ' + Made[I]);
          DeleteFile(Made[I]);
        end;
  finally
    Made.Free;
  end;
end;

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
