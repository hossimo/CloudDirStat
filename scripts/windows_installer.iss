; Windows installer (Inno Setup 7). It asks whether to install for all users (Program Files,
; needs administrator rights) or only the current user (%LOCALAPPDATA%\Programs, no admin).
; Unattended: /ALLUSERS or /CURRENTUSER, plus /VERYSILENT.
;
; Build (the release workflow does this for each Windows target):
;   iscc /DVersion=1.2.3 /DArch=x64 scripts\windows_installer.iss
; Arch is x64 or arm64. SourceDir (the folder holding both .exe files) defaults to
; target\release; relative paths start from this script's folder. The installer lands in dist\.

#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif
#ifndef OutputName
  #define OutputName "CloudDirStat-" + Version + "-windows-" + Arch + "-setup"
#endif

[Setup]
; Never change AppId: Windows and winget use it to find an existing install to upgrade or remove.
AppId={{C9221479-2A18-4342-8440-E6F9FC3FB7AB}
AppName=CloudDirStat
AppVersion={#Version}
AppVerName=CloudDirStat {#Version}
UninstallDisplayName=CloudDirStat
AppPublisher=Downright Tech
AppPublisherURL=https://github.com/hossimo/CloudDirStat
AppSupportURL=https://github.com/hossimo/CloudDirStat/issues
AppUpdatesURL=https://github.com/hossimo/CloudDirStat/releases
VersionInfoVersion={#Version}
DefaultDirName={autopf}\CloudDirStat
DisableProgramGroupPage=yes
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog commandline
ChangesEnvironment=yes
SetupArchitecture=x64
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#endif
UninstallDisplayIcon={app}\clouddirstat-gui.exe
SetupIconFile=..\icons\clouddirstat.ico
WizardStyle=modern dynamic
Compression=lzma2/max
SolidCompression=yes
OutputDir=..\dist
OutputBaseFilename={#OutputName}

[Tasks]
Name: path; Description: "Add the command line (clouddirstat) to PATH"
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\clouddirstat-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\clouddirstat.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\CloudDirStat"; Filename: "{app}\clouddirstat-gui.exe"
Name: "{autodesktop}\CloudDirStat"; Filename: "{app}\clouddirstat-gui.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\clouddirstat-gui.exe"; Description: "{cm:LaunchProgram,CloudDirStat}"; Flags: nowait postinstall skipifsilent

[Code]
// PATH lives in a different registry key for all users than for one user.
function PathRoot: Integer;
begin
  if IsAdminInstallMode then Result := HKLM else Result := HKCU;
end;

function PathKey: String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

// The folder list padded with ';' on both ends, so every entry can be found as ';entry;'.
function PaddedPath: String;
begin
  if not RegQueryStringValue(PathRoot, PathKey, 'Path', Result) then Result := '';
  Result := ';' + Result + ';';
end;

function InPath(Path, Dir: String): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', Uppercase(Path)) > 0;
end;

procedure SetPath(Path: String);
begin
  while Pos(';;', Path) > 0 do StringChangeEx(Path, ';;', ';', True);
  if (Length(Path) > 0) and (Path[1] = ';') then Delete(Path, 1, 1);
  if (Length(Path) > 0) and (Path[Length(Path)] = ';') then Delete(Path, Length(Path), 1);
  // REG_EXPAND_SZ keeps entries like %USERPROFILE%\bin working.
  RegWriteExpandStringValue(PathRoot, PathKey, 'Path', Path);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Path: String;
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('path') then
  begin
    Path := PaddedPath;
    if not InPath(Path, ExpandConstant('{app}')) then SetPath(Path + ExpandConstant('{app}'));
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Path, Dir: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    Path := PaddedPath;
    Dir := ExpandConstant('{app}');
    if InPath(Path, Dir) then
    begin
      Delete(Path, Pos(';' + Uppercase(Dir) + ';', Uppercase(Path)), Length(Dir) + 1);
      SetPath(Path);
    end;
  end;
end;
