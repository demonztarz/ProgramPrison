; Program Prison installer (Inno Setup 6). Built in CI: iscc /DAppVersion=0.3.0 installer\programprison.iss
; Installs for the current user only (no administrator prompt), like VS Code or Discord.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
; Never change AppId: Windows uses it to recognise updates and the uninstaller.
AppId={{ED45AE02-6ADC-4E9D-80CC-51C5FEA8D622}
AppName=Program Prison
AppVersion={#AppVersion}
AppVerName=Program Prison {#AppVersion}
AppPublisher=Program Prison
AppPublisherURL=https://github.com/demonztarz/ProgramPrison
AppSupportURL=https://github.com/demonztarz/ProgramPrison
VersionInfoVersion={#AppVersion}
VersionInfoDescription=Program Prison installer
DefaultDirName={localappdata}\Programs\Program Prison
DefaultGroupName=Program Prison
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\out
OutputBaseFilename=Program Prison Setup {#AppVersion}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\programprison.exe
UninstallDisplayName=Program Prison
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; The installer stops Program Prison itself (see PrepareToInstall) and starts it again.
CloseApplications=no
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\programprison.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\README.txt"; DestDir: "{app}"; DestName: "How to use Program Prison.txt"; Flags: ignoreversion isreadme

[Icons]
Name: "{autoprograms}\Program Prison"; Filename: "{app}\programprison.exe"; Comment: "Block distracting websites and programs"
Name: "{autoprograms}\How to use Program Prison"; Filename: "{app}\How to use Program Prison.txt"
Name: "{autodesktop}\Program Prison"; Filename: "{app}\programprison.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\programprison.exe"; Description: "Open Program Prison"; Flags: nowait postinstall skipifsilent

[Code]
procedure StopRunningCopies();
var
  R: Integer;
begin
  // The installed copy and the single-file download have different file names.
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM programprison.exe', '', SW_HIDE, ewWaitUntilTerminated, R);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM "Program Prison.exe"', '', SW_HIDE, ewWaitUntilTerminated, R);
  Sleep(500);
end;

// Installing or updating replaces the program file, so the running copy has to stop first.
// Blocks aren't lost: they're saved, and Program Prison starts again right after installing
// (and at every login) and carries on with them.
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopRunningCopies();
  Result := '';
end;

// Uninstalling would be an easy way out of a locked block, so it waits until the block ends.
function InitializeUninstall(): Boolean;
var
  R: Integer;
begin
  Result := True;
  if Exec(ExpandConstant('{app}\programprison.exe'), '--uninstall-check', '', SW_HIDE, ewWaitUntilTerminated, R) and (R = 2) then
  begin
    MsgBox('A locked block is running, so Program Prison can''t be uninstalled until it ends.', mbInformation, MB_OK);
    Result := False;
  end;
end;

// Before the files go: stop Program Prison, remove its website entries from the hosts file
// and its start-at-login entry. Settings stay in %APPDATA%\ProgramPrison for a reinstall.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  R: Integer;
begin
  if CurUninstallStep = usUninstall then
  begin
    Exec(ExpandConstant('{app}\programprison.exe'), '--uninstall', '', SW_HIDE, ewWaitUntilTerminated, R);
    StopRunningCopies();
  end;
end;
