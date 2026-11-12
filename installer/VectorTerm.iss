; =====================================================================
; VectorTerm 安装包脚本(Inno Setup 6.5+,推荐 7)
;
; 界面全程简体中文(单语言,不弹语言选择框),翻译文件 ChineseSimplified.isl
; 随本目录带一份 —— 只装了英文版 Inno Setup 的机器也能编出中文向导。
; 许可页显示 LICENSE.zh-CN.txt(中文参考译本),完整条款是仓库根的 LICENSE。
;
; 编译(不要直接双击,版本号与输出目录由 build.cmd 传进来):
;   build.cmd installer
; 手动编译:
;   ISCC.exe /DMyAppVersion=0.1.0 installer\VectorTerm.iss
;
; 文件必须是 **UTF-8 with BOM**,否则 ISCC 按 ANSI 解码,中文会变乱码。
; =====================================================================

#define MyAppName "VectorTerm"
#define MyAppExeName "VectorTerm.exe"
#define MySettingsExeName "vectorterm-settings.exe"
#define MyAppPublisher "VectorTerm contributors"
#define MyAppComment "Windows 终端模拟器:wezterm-term 核心 + Kitty 风格光标尾迹 + Lua 配置"

; build.cmd 从 Cargo.toml 读出真实版本后覆盖这个默认值
#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

; Release 产物路径。默认就是仓库里的 target\release\VectorTerm.exe;
; 换过 CARGO_TARGET_DIR / VECTORTERM_TARGET_DIR 时 build.cmd 会用 /D 覆盖。
#ifndef MySourceExe
  #define MySourceExe "..\target\release\VectorTerm.exe"
#endif
#ifndef MySourceSettingsExe
  #define MySourceSettingsExe "..\target\release\vectorterm-settings.exe"
#endif

[Setup]
; AppId 一旦发布就不能再改:升级、卸载、覆盖安装全靠它认人
AppId={{7C4E1F52-9B3D-4A86-8E27-6D5A2C9B3F14}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppComments={#MyAppComment}
VersionInfoVersion={#MyAppVersion}
VersionInfoCompany={#MyAppPublisher}
VersionInfoDescription={#MyAppComment}
VersionInfoProductName={#MyAppName}
VersionInfoProductVersion={#MyAppVersion}

; 默认按当前用户安装到 %LOCALAPPDATA%\Programs\VectorTerm,不需要管理员;
; 想装到 Program Files 就在向导的"安装模式"页选"为所有用户安装"。
DefaultDirName={autopf}\VectorTerm
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
AllowNoIcons=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline

ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763

; 安装向导里显示中文译本;英文原文(LICENSE)也一并装进 {app}
LicenseFile=..\LICENSE.zh-CN.txt
SetupIconFile=..\assets\icons\VectorTerm.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppName}

OutputDir=..\target\installer
OutputBaseFilename={#MyAppName}-{#MyAppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ShowLanguageDialog=no
SetupLogging=yes

; 覆盖安装时请正在运行的 VectorTerm 让路,装完自行恢复不了就提示重启
CloseApplications=yes
RestartApplications=no

[Languages]
; 只用简体中文,所以 Setup 里的 ShowLanguageDialog=no 不会弹选择框。
; 用仓库内自带的翻译文件而不是 compiler:Languages\... —— 别人的 Inno Setup
; 可能根本没装中文包(6.x 默认只有英文),那样编译会直接失败。
Name: "zh"; MessagesFile: "ChineseSimplified.isl"

[Tasks]
; 默认勾选:PATH 里有了 vectorterm,任意终端直接敲就能用;配置管理器 vectorterm-settings
; 也在同一目录,一并加入 PATH。
Name: "addtopath"; Description: "将 VectorTerm 加入 PATH 环境变量(任意终端可直接运行 vectorterm 与 vectorterm-settings)"; GroupDescription: "附加任务:"
; 资源管理器右键菜单:文件夹空白处 / 文件夹图标 / 盘符右键都能直接在这里开终端。
; 写进 HKCU\Software\Classes(装给所有用户时是 HKLM),卸载时随 key 一起删干净。
Name: "contextmenu"; Description: "在资源管理器右键菜单加入「在此处打开 VectorTerm」(文件夹、文件夹空白处、盘符)"; GroupDescription: "附加任务:"
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务:"; Flags: unchecked

[Files]
Source: "{#MySourceExe}"; DestDir: "{app}"; Flags: ignoreversion
; 配置管理器(与终端本体独立的图形化配置工具)
Source: "{#MySourceSettingsExe}"; DestDir: "{app}"; Flags: ignoreversion
; 默认配置只在缺失时放一份(升级不会覆盖本地的改动)。
; 这份是"出厂默认",想长期自定义请拷到 %USERPROFILE%\.config\VectorTerm\config.lua,
; 那个位置优先级更高 —— 所以卸载时删掉它不会带走用户真正的配置。
Source: "..\config\config.lua"; DestDir: "{app}\config"; Flags: onlyifdoesntexist
; 内置主题(config\*.lua 里除 config.lua 之外的文件)随版本升级同步覆盖:
; 它们是随程序分发的只读预设,不是用户配置,所以不用 onlyifdoesntexist。
; 在 config.lua 里写 theme = "deepseek" 即可套用,见手册 §2.5。
Source: "..\config\*.lua"; DestDir: "{app}\config"; Excludes: "config.lua"; Flags: ignoreversion
Source: "..\config.example.lua"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\LICENSE.zh-CN.txt"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
; 链接了 wezterm 的 MIT 代码,分发二进制时必须带这份声明
Source: "..\THIRD-PARTY-NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
; 内嵌字体(JetBrains Mono / Symbols Nerd Font / Noto Color Emoji)的 SIL OFL
; 要求随二进制附上许可全文,单独放在 licenses\ 免得和程序文件混在一起。
Source: "..\assets\fonts\LICENSE_*.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\assets\fonts\README.md"; DestDir: "{app}\licenses"; DestName: "FONTS.md"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\docs\config-manual.md"; DestDir: "{app}\docs"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{%USERPROFILE}"; Comment: "{#MyAppComment}"
Name: "{autoprograms}\{#MyAppName} 配置管理器"; Filename: "{app}\{#MySettingsExeName}"; WorkingDir: "{%USERPROFILE}"; Comment: "图形化管理 VectorTerm 配置"
Name: "{autoprograms}\卸载 {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{%USERPROFILE}"; Tasks: desktopicon

[Registry]
; Win+R 里直接敲 vectorterm 也能启动(与 PATH 无关的那一条)
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#MyAppExeName}"; ValueType: string; ValueName: ""; ValueData: "{app}\{#MyAppExeName}"; Flags: uninsdeletekey
; 配置管理器同样进 App Paths,Win+R 敲 vectorterm-settings 也能开
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#MySettingsExeName}"; ValueType: string; ValueName: ""; ValueData: "{app}\{#MySettingsExeName}"; Flags: uninsdeletekey
; ---- 资源管理器右键菜单 ----
; 三处入口共用一套 verb:Directory\Background 是文件夹内空白处(%V = 当前目录),
; Directory 是文件夹图标(%1 = 被点的那个文件夹),Drive 是盘符(%1 = D:\)。
; 参数统一用 --cwd 传进去(见 src/main.rs 的 parse_cwd_arg)。末尾那个 `\.` 不是笔误:
; 盘根(右键 D:\)时 %V 就以反斜杠结尾,直接拼成 `--cwd "D:\"` 会让 Windows 命令行
; 解析把 `\"` 当成转义引号 —— 这条参数变 `D:"`,后面的参数还会被并进来,整条命令行
; 都拆错。补个 `.` 收尾解析就正常,程序侧 tidy_cwd_arg 会把补的部分剪掉。
; MultiSelectModel=Single:选中多个文件夹时不出现(我们只认一个路径)。
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\VectorTerm"; ValueType: string; ValueName: ""; ValueData: "在此处打开 VectorTerm"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\VectorTerm"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\VectorTerm\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" --cwd ""%V\."""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\VectorTerm"; ValueType: string; ValueName: ""; ValueData: "在此处打开 VectorTerm"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\VectorTerm"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\VectorTerm"; ValueType: string; ValueName: "MultiSelectModel"; ValueData: "Single"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\VectorTerm\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" --cwd ""%1\."""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Drive\shell\VectorTerm"; ValueType: string; ValueName: ""; ValueData: "在此处打开 VectorTerm"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Drive\shell\VectorTerm"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Drive\shell\VectorTerm"; ValueType: string; ValueName: "MultiSelectModel"; ValueData: "Single"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Drive\shell\VectorTerm\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" --cwd ""%1\."""; Tasks: contextmenu

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "立即运行 {#MyAppName}"; Flags: nowait postinstall skipifsilent

[Code]
{ Inno 7 的脚本引擎已经预置了 HWND_BROADCAST,别再定义同名常量;
  统一加 WT_ 前缀,免得跟着上游预置表一起漂。 }
const
  WT_WM_SETTINGCHANGE = $001A;
  WT_HWND_BROADCAST   = $FFFF;
  WT_SMTO_ABORTIFHUNG = $0002;
  EnvSystemKey        = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  { 记录"装之前 PATH 末尾有几个分号",卸时装回去。为什么需要它见 TrailingSemicolons }
  LastPathKey         = 'Software\VectorTerm';

{ 通知已经在跑的 explorer / 终端:环境变量变了。
  SendMessageTimeoutW 的 lParam 是 LPARAM,这里直接传字符串 —— Pascal Script
  把 String 编成宽字符指针,与 W 版 ABI 对得上。 }
function SendMessageTimeout(hWnd: HWND; Msg: UINT; wParam: WPARAM; lParam: String;
  fuFlags: UINT; uTimeout: UINT; var lpdwResult: UINT_PTR): LRESULT;
  external 'SendMessageTimeoutW@user32.dll stdcall';

function PathRootKey(): Integer;
begin
  { 装到 Program Files 时改系统 PATH,装到当前用户目录时改用户 PATH }
  if IsAdminInstallMode() then
    Result := HKEY_LOCAL_MACHINE
  else
    Result := HKEY_CURRENT_USER;
end;

function PathSubKey(): String;
begin
  if IsAdminInstallMode() then
    Result := EnvSystemKey
  else
    Result := 'Environment';
end;

function BroadcastEnvironment(): Boolean;
var
  Res: UINT_PTR;
begin
  { 0 表示有窗口超时没理;这不是错误,值已经写进注册表了 }
  Result := SendMessageTimeout(WT_HWND_BROADCAST, WT_WM_SETTINGCHANGE, 0, 'Environment',
    WT_SMTO_ABORTIFHUNG, 5000, Res) <> 0;
end;

function StripTrailingSlash(const S: String): String;
begin
  Result := S;
  while (Length(Result) > 3) and (Result[Length(Result)] = '\') do
    Delete(Result, Length(Result), 1);
end;

{ 末尾的分号数。PATH 是按 ';' 切开再拼回去的,切的时候末尾空段会被自然丢掉 ——
  原来 "...;Tools;" 移除后只剩 "...;Tools",少了一个字符。数量本身没有语义
  (现代 Windows 忽略空段),但用户拿工具逐字节对比时会以为卸不干净,
  所以安装前把个数记下,卸载时原数补上。 }
function TrailingSemicolons(const S: String): Integer;
var
  I: Integer;
begin
  Result := 0;
  I := Length(S);
  while (I > 0) and (S[I] = ';') do
  begin
    Inc(Result);
    Dec(I);
  end;
end;

{ PATH 是分号分隔的一串,必须整段比较,不能子串包含 ——
  C:\Tools 和 C:\ToolsExtra 不能算同一个。 }
function PathHasDir(const Paths, Dir: String): Boolean;
var
  Rest, Item, Target: String;
  P: Integer;
begin
  Result := False;
  Target := Uppercase(StripTrailingSlash(Trim(Dir)));
  Rest := Paths;
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P > 0 then
    begin
      Item := Copy(Rest, 1, P - 1);
      Delete(Rest, 1, P);
    end
    else
    begin
      Item := Rest;
      Rest := '';
    end;
    if Uppercase(StripTrailingSlash(Trim(Item))) = Target then
    begin
      Result := True;
      Exit;
    end;
  end;
end;

procedure AddToPath(const Dir: String);
var
  Paths: String;
begin
  if not RegQueryStringValue(PathRootKey(), PathSubKey(), 'Path', Paths) then
    Paths := '';
  if PathHasDir(Paths, Dir) then
  begin
    Log('PATH 里已经有 ' + Dir + ',跳过');
    Exit;
  end;
  { 趁 Paths 还是原始值,先把末尾分号数存下来 }
  RegWriteDWordValue(HKEY_CURRENT_USER, LastPathKey, 'TrailingSemicolons',
    TrailingSemicolons(Paths));
  if (Paths <> '') and (Paths[Length(Paths)] <> ';') then
    Paths := Paths + ';';
  Paths := Paths + Dir;
  if RegWriteExpandStringValue(PathRootKey(), PathSubKey(), 'Path', Paths) then
  begin
    Log('已加入 PATH: ' + Dir);
    if not BroadcastEnvironment() then
      Log('广播 WM_SETTINGCHANGE 超时(不影响注册表里的值)');
  end
  else
    Log('写 PATH 失败: ' + Dir);
end;

procedure RemoveFromPath(const Dir: String);
var
  Paths, Rest, Item, Kept, Target: String;
  P: Integer;
  Trail: Cardinal;
begin
  if not RegQueryStringValue(PathRootKey(), PathSubKey(), 'Path', Paths) then
    Exit;
  Target := Uppercase(StripTrailingSlash(Trim(Dir)));
  Rest := Paths;
  Kept := '';
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P > 0 then
    begin
      Item := Copy(Rest, 1, P - 1);
      Delete(Rest, 1, P);
    end
    else
    begin
      Item := Rest;
      Rest := '';
    end;
    { 空段是 PATH 里合法存在的脏数据(比如 "a;;b"),原样留着,只摘掉自己那一段 }
    if (Item <> '') and (Uppercase(StripTrailingSlash(Trim(Item))) = Target) then
      Continue;
    if Kept = '' then
      Kept := Item
    else
      Kept := Kept + ';' + Item;
  end;
  if Kept = Paths then
  begin
    Log('PATH 里没有 ' + Dir + ',无需清理');
    RegDeleteValue(HKEY_CURRENT_USER, LastPathKey, 'TrailingSemicolons');
    Exit;
  end;
  { 把安装前的末尾分号原数补回去;没有记录时(比如从旧版本升级上来)
    退回原逻辑,按当前值末尾是否带分号判断 }
  if Kept <> '' then
  begin
    if RegQueryDWordValue(HKEY_CURRENT_USER, LastPathKey, 'TrailingSemicolons', Trail) then
    begin
      while Trail > 0 do
      begin
        Kept := Kept + ';';
        Trail := Trail - 1;
      end;
    end
    else if Paths[Length(Paths)] = ';' then
      Kept := Kept + ';';
  end;
  if RegWriteExpandStringValue(PathRootKey(), PathSubKey(), 'Path', Kept) then
  begin
    Log('已从 PATH 移除: ' + Dir);
    BroadcastEnvironment();
  end
  else
    Log('清理 PATH 失败: ' + Dir);
  RegDeleteValue(HKEY_CURRENT_USER, LastPathKey, 'TrailingSemicolons');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    if WizardIsTaskSelected('addtopath') then
      AddToPath(ExpandConstant('{app}'))
    else
      Log('用户没有勾选"加入 PATH"');
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;

{ 许可页默认选中"我同意",少点一次;不想接受的人照样能改选。
  条款本身没变,只是把默认选项挪到"接受"这一侧。 }
procedure InitializeWizard();
begin
  WizardForm.LicenseAcceptedRadio.Checked := True;
end;
