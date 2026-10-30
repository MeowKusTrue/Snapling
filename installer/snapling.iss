; ISCC installer\snapling.iss after `cargo build --release`; tools\package.py does both.
#define Exe "..\target\release\snapling.exe"

[Setup]
AppId={{7C1E4F52-9B0A-4E36-A0D1-5B3C8E2F6A17}
AppName=Snapling
AppVersion={#GetVersionNumbersString(Exe)}
AppVerName=Snapling
DefaultDirName={userpf}\Snapling
DisableDirPage=no
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
AppMutex=Local\Snapling.SingleInstance
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\assets\brand\snapling.ico
UninstallDisplayIcon={app}\snapling.exe
OutputDir=..\target\dist
OutputBaseFilename=Snapling-Setup
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
ru.Autostart=Запускать вместе с Windows
en.Autostart=Start with Windows
ru.Desktop=Значок на рабочем столе
en.Desktop=Desktop icon
ru.Launch=Запустить Snapling
en.Launch=Start Snapling
ru.WipeData=Удалить настройки, историю и скачанные модели?%n%nСохранённые снимки и ключи API останутся.
en.WipeData=Delete settings, history and downloaded models?%n%nSaved screenshots and API keys stay.

[Tasks]
Name: "autostart"; Description: "{cm:Autostart}"
Name: "desktop"; Description: "{cm:Desktop}"; Flags: unchecked

[Files]
Source: "{#Exe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"
Source: "..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"

[Icons]
Name: "{userprograms}\Snapling"; Filename: "{app}\snapling.exe"
Name: "{userdesktop}\Snapling"; Filename: "{app}\snapling.exe"; Tasks: desktop

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Snapling"; ValueData: """{app}\snapling.exe"""; Tasks: autostart
; Autostart may have been turned on in the settings instead: the entry goes on uninstall anyway.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Snapling"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\snapling.exe"; Description: "{cm:Launch}"; Flags: nowait postinstall skipifsilent

[Code]
procedure CurUninstallStepChanged(Step: TUninstallStep);
begin
  if (Step = usPostUninstall) and not UninstallSilent and
     (MsgBox(CustomMessage('WipeData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES) then
  begin
    DelTree(ExpandConstant('{userappdata}\Snapling'), True, True, True);
    DelTree(ExpandConstant('{localappdata}\Snapling'), True, True, True);
  end;
end;
