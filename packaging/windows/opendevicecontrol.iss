; Inno Setup script for the Windows installer.
;
; Build (from the repository root) after building target\release\opendevicecontrol.exe
; (`cargo build --release -p opendevicecontrol`), target\release\opendevicecontrol-cli.exe
; (the same with `--features console`, copied to that name) and
; target\release\opendevicecontrol-sensors.exe
; (`cargo build --release -p opendevicecontrol-core --bin opendevicecontrol-sensors`), and
; fetching PawnIO (packaging\windows\fetch-pawnio.ps1):
;   iscc /DAppVersion=0.1.0 packaging\windows\opendevicecontrol.iss
; The installer is written to target\installer\.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define AppName "OpenDeviceControl"
#define AppExe "opendevicecontrol.exe"
#define Root "..\.."
#define RepoUrl "https://github.com/skylord123/opendevicecontrol"

[Setup]
AppId={{23FB77AD-CF30-4B0E-A92E-9FE4AB14B25E}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=OpenDeviceControl
AppPublisherURL={#RepoUrl}
AppSupportURL={#RepoUrl}/issues
AppUpdatesURL={#RepoUrl}/releases
DefaultDirName={autopf}\opendevicecontrol
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; Installs just for the current user without admin rights (USB HID access
; on Windows needs none), or for all users with them.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile={#Root}\LICENSE
SetupIconFile={#Root}\assets\opendevicecontrol.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir={#Root}\target\installer
OutputBaseFilename=opendevicecontrol-windows-x86_64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; A running opendevicecontrol is asked to quit cleanly in PrepareToInstall (below);
; the Restart Manager is the fallback.
CloseApplications=yes
RestartApplications=no
; The "Add to PATH" task changes the environment.
ChangesEnvironment=yes

[Tasks]
Name: "autostart"; Description: "Start opendevicecontrol when I log in (in the tray)"; GroupDescription: "Startup:"
Name: "sensors"; Description: "Install the hardware sensor service (CPU and motherboard sensors; installs the PawnIO driver and asks for administrator rights)"; GroupDescription: "Hardware sensors:"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "Add to PATH, to use opendevicecontrol-cli from a terminal"; GroupDescription: "Command line:"; Flags: unchecked

[Files]
Source: "{#Root}\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\target\release\opendevicecontrol-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
; The sensor service: opendevicecontrol-sensors.exe --install copies itself and the
; PawnIO modules to %ProgramFiles%\OpenDeviceControl\Sensors and runs from there.
Source: "{#Root}\target\release\opendevicecontrol-sensors.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "pawnio\PawnIO_setup.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "pawnio\modules\*.bin"; DestDir: "{app}\pawnio"; Flags: ignoreversion
Source: "pawnio\modules\COPYING"; DestDir: "{app}\pawnio"; DestName: "COPYING.txt"; Flags: ignoreversion
Source: "{#Root}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "PC hardware and device monitor with Home Assistant integration"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; The same entry opendevicecontrol's own "Start opendevicecontrol when I log in" setting uses,
; so the checkbox in the app reflects it. Removed on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "opendevicecontrol"; ValueData: """{app}\{#AppExe}"" --minimized"; Tasks: autostart; Flags: uninsdeletevalue

[Run]
; Elevated (a UAC prompt in a per-user install): installs PawnIO when it's
; missing and the service, which OpenDeviceControl then reads over a local pipe.
Filename: "{app}\opendevicecontrol-sensors.exe"; Parameters: "--install"; Verb: "runas"; Flags: shellexec runhidden waituntilterminated; Tasks: sensors; StatusMsg: "Installing the hardware sensor service..."
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Remove the sensor service (PawnIO stays: other tools may use it).
Filename: "{app}\opendevicecontrol-sensors.exe"; Parameters: "--uninstall"; Verb: "runas"; Flags: shellexec runhidden waituntilterminated; Check: SensorServiceInstalled; RunOnceId: "RemoveSensorService"
; A running opendevicecontrol is asked to quit in InitializeUninstall (below).

[UninstallDelete]
Type: files; Name: "{app}\*.log"

[Code]
// Exit codes of "opendevicecontrol-cli --quit" (versions before these codes
// return 0 when one was running, 1 when none was).
const
  QuitStopped = 0;
  QuitNotRunning = 1;
  QuitTimedOut = 2;
  QuitFailed = 3;

// Ask a running opendevicecontrol to exit cleanly (it hands the PSU fan back
// to hardware control). Returns an error message when it is still running,
// or '' when it stopped or none was running.
function QuitRunning: String;
var
  Cli: String;
  Code: Integer;
begin
  Result := '';
  Cli := ExpandConstant('{app}\opendevicecontrol-cli.exe');
  if not FileExists(Cli) then
    exit;
  if not Exec(Cli, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code) then begin
    Log('Could not run ' + Cli + ' --quit: ' + SysErrorMessage(Code));
    exit;
  end;
  case Code of
    QuitStopped: Log('OpenDeviceControl exited cleanly.');
    QuitNotRunning: Log('OpenDeviceControl was not running.');
    QuitFailed: Log('OpenDeviceControl exited, but could not hand every device back to hardware control (see its log).');
    QuitTimedOut: Result := 'OpenDeviceControl did not exit in time. Quit it from its tray icon, then try again.';
  else
    Log(Format('opendevicecontrol-cli --quit returned %d.', [Code]));
  end;
end;

// Before files from a previous install are replaced.
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := QuitRunning;
end;

// Before anything is removed.
function InitializeUninstall: Boolean;
var
  Error: String;
begin
  Error := QuitRunning;
  Result := Error = '';
  if not Result then
    SuppressibleMsgBox(Error, mbError, MB_OK, IDOK);
end;

function SensorServiceInstalled: Boolean;
begin
  Result := RegKeyExists(HKEY_LOCAL_MACHINE, 'SYSTEM\CurrentControlSet\Services\OpenDeviceControlSensors');
end;

// "Add to PATH": the user's PATH for a per-user install, the system PATH
// for an all-users install. Removed again on uninstall, but only when setup
// added it (recorded under SetupKey): an entry that was already there, or a
// skipped task, leaves PATH alone.

const
  SetupKey = 'Software\OpenDeviceControl\Setup';
  PathOwnedValue = 'AddedToPath';

function PathRoot: Integer;
begin
  if IsAdminInstallMode then
    Result := HKEY_LOCAL_MACHINE
  else
    Result := HKEY_CURRENT_USER;
end;

function PathKey: String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

function PathHas(Paths, Dir: String): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') > 0;
end;

procedure AddToPath(Dir: String);
var
  Paths: String;
begin
  if not RegQueryStringValue(PathRoot, PathKey, 'Path', Paths) then
    Paths := '';
  // Already there: added by an earlier install (still recorded) or by the
  // user (not ours to remove).
  if PathHas(Paths, Dir) then
    exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then
    Paths := Paths + ';';
  if RegWriteExpandStringValue(PathRoot, PathKey, 'Path', Paths + Dir) then
    RegWriteStringValue(PathRoot, SetupKey, PathOwnedValue, Dir);
end;

procedure RemoveFromPath(Dir: String);
var
  Paths: String;
  P: Integer;
begin
  if not RegQueryStringValue(PathRoot, PathKey, 'Path', Paths) then
    exit;
  Paths := ';' + Paths + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Paths));
  if P = 0 then
    exit;
  Delete(Paths, P, Length(Dir) + 1);
  Paths := Copy(Paths, 2, Length(Paths) - 2);
  RegWriteExpandStringValue(PathRoot, PathKey, 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Owned: String;
begin
  if CurUninstallStep = usPostUninstall then begin
    if RegQueryStringValue(PathRoot, SetupKey, PathOwnedValue, Owned) then begin
      if CompareText(Owned, ExpandConstant('{app}')) = 0 then
        RemoveFromPath(Owned);
      RegDeleteValue(PathRoot, SetupKey, PathOwnedValue);
      RegDeleteKeyIfEmpty(PathRoot, SetupKey);
      RegDeleteKeyIfEmpty(PathRoot, 'Software\OpenDeviceControl');
    end;
  end;
end;
