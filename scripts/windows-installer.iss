; PDF Outliner Windows 설치 파일(Inno Setup 6) — scripts/package-windows.ps1이 ISCC로 컴파일한다.
; 코드서명 없음: SmartScreen "Windows의 PC 보호" 경고 → "추가 정보" → "실행"으로 설치(README 참고).
;
; 필수 /D 정의(package-windows.ps1이 넘김):
;   AppVersion         예: 0.2.1
;   SourceDir          PDF-Outliner.exe와 pdfium.dll이 든 폴더
;   OutputDir          설치 파일을 만들 폴더
;   OutputBaseFilename 예: PDF-Outliner-v0.2.1-windows-x64-setup
;   IconFile           설치 파일 아이콘(.ico)

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
; AppId는 업그레이드·제거 식별자라 절대 바꾸지 말 것.
AppId={{8F3C2A5E-6B1D-4C7A-9E2F-5D4B3A2C1E0F}
AppName=PDF Outliner
AppVersion={#AppVersion}
AppVerName=PDF Outliner {#AppVersion}
AppPublisher=bjorkii
AppPublisherURL=https://github.com/bjorkii/PDF-Outliner
DefaultDirName={autopf}\PDF Outliner
DefaultGroupName=PDF Outliner
DisableProgramGroupPage=yes
; 관리자 권한 없이 사용자 폴더에 설치할 수 있게(설치 시 "모든 사용자"를 고르면 관리자 권한 요청).
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseFilename}
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\PDF-Outliner.exe
UninstallDisplayName=PDF Outliner
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Languages]
#if FileExists(AddBackslash(CompilerPath) + "Languages\Korean.isl")
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"
#endif
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\PDF-Outliner.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\pdfium.dll"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\PDF Outliner"; Filename: "{app}\PDF-Outliner.exe"
Name: "{autodesktop}\PDF Outliner"; Filename: "{app}\PDF-Outliner.exe"; Tasks: desktopicon

[Registry]
; PDF 파일의 "연결 프로그램" 목록에 PDF Outliner를 올린다(기본 PDF 앱은 바꾸지 않음).
; 앱은 argv로 받은 파일 경로를 연다(crates/ui/src/main.rs).
Root: HKA; Subkey: "Software\Classes\PDFOutliner.pdf"; ValueType: string; ValueName: ""; ValueData: "PDF Document"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\PDFOutliner.pdf\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\PDF-Outliner.exe,0"
Root: HKA; Subkey: "Software\Classes\PDFOutliner.pdf\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\PDF-Outliner.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "PDFOutliner.pdf"; ValueData: ""; Flags: uninsdeletevalue

[Run]
Filename: "{app}\PDF-Outliner.exe"; Description: "{cm:LaunchProgram,PDF Outliner}"; Flags: nowait postinstall skipifsilent
