; Trek Inno Setup Installer Script
; Defaults to user folder: {userappdata}\Programs\Trek (no admin privileges needed)

#ifndef AppVersion
#define AppVersion "0.4.5"
#endif

#ifndef BinDir
#define BinDir "..\target\release"
#endif

[Setup]
AppName=Trek
AppVersion={#AppVersion}
AppPublisher=Trek
DefaultDirName={userappdata}\Programs\Trek
DefaultGroupName=Trek
DisableProgramGroupPage=yes
OutputBaseFilename=Trek-Setup-x86_64
Compression=lzma
SolidCompression=yes
PrivilegesRequired=lowest
OutputDir=..\target\installer
SetupIconFile=..\assets\brand\trek.ico
UninstallDisplayIcon={app}\trek.exe

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#BinDir}\trek.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\trek-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
; If trek-update exists
Source: "{#BinDir}\trek-update.exe"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "Uninstall-Trek.ps1"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\Trek"; Filename: "{app}\trek.exe"
Name: "{autodesktop}\Trek"; Filename: "{app}\trek.exe"; Tasks: desktopicon

[Code]
var
  FinishPage: TWizardPage;
  Option1Radio, Option2Radio, Option3Radio: TRadioButton;

procedure InitializeWizard;
begin
  FinishPage := CreateCustomPage(wpInstalling, 'Trek is Ready!', 'Installation completed successfully. Please choose how to finish:');
  
  Option1Radio := TRadioButton.Create(WizardForm);
  Option1Radio.Parent := FinishPage.Surface;
  Option1Radio.Caption := 'Finish and open up Trek';
  Option1Radio.Left := ScaleX(20);
  Option1Radio.Top := ScaleY(30);
  Option1Radio.Width := ScaleX(300);
  Option1Radio.Checked := True;

  Option2Radio := TRadioButton.Create(WizardForm);
  Option2Radio.Parent := FinishPage.Surface;
  Option2Radio.Caption := 'Finish and open up folder';
  Option2Radio.Left := ScaleX(20);
  Option2Radio.Top := ScaleY(70);
  Option2Radio.Width := ScaleX(300);

  Option3Radio := TRadioButton.Create(WizardForm);
  Option3Radio.Parent := FinishPage.Surface;
  Option3Radio.Caption := 'Finish and close installer';
  Option3Radio.Left := ScaleX(20);
  Option3Radio.Top := ScaleY(110);
  Option3Radio.Width := ScaleX(300);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if CurStep = ssPostInstall then
  begin
    if Option1Radio.Checked then
      Exec(ExpandConstant('{app}\trek.exe'), '', ExpandConstant('{app}'), SW_SHOW, ewNoWait, ResultCode)
    else if Option2Radio.Checked then
      Exec('explorer.exe', ExpandConstant('"{app}"'), '', SW_SHOW, ewNoWait, ResultCode);
  end;
end;
