# =====================================================================
# 注册 / 撤销「在此处打开 VectorTerm」资源管理器右键菜单(仅当前用户)。
#
#   .\context-menu.ps1                 # 给本仓库编译出来的 exe 注册
#   .\context-menu.ps1 -Remove         # 撤销
#   .\context-menu.ps1 -ExePath D:\apps\VectorTerm.exe
#
# 安装包(installer\VectorTerm.iss)里有同样的条目,勾了「附加任务 → 右键菜单」
# 就不必再跑这个脚本;这个脚本是给"直接用 target\release 里 exe"的开发场景用的。
#
# 写的是 HKCU\Software\Classes,不需要管理员。Windows 11 的右键菜单默认只显示
# 一级菜单,新加的 verb 落在「显示更多选项」里(Shift + 右键可直达经典菜单)。
# =====================================================================
param(
    [string]$ExePath,
    [switch]$Remove
)

$ErrorActionPreference = 'Stop'

function Write-Step($msg) { Write-Host "[context-menu] $msg" }

$verbs = @(
    @{ Key = 'Directory\Background'; Arg = '%V'; Multi = $false },  # 文件夹内空白处
    @{ Key = 'Directory';            Arg = '%1'; Multi = $true  },  # 文件夹图标
    @{ Key = 'Drive';                Arg = '%1'; Multi = $true  }   # 盘符
)

$base = 'HKCU:\Software\Classes'

if ($Remove) {
    foreach ($v in $verbs) {
        $key = Join-Path $base ($v.Key + '\shell\VectorTerm')
        if (Test-Path $key) {
            Remove-Item -Path $key -Recurse -Force
            Write-Step "已删除 $($v.Key) 的菜单项"
        }
    }
    Write-Step '完成。重新打开资源管理器窗口即可看到效果。'
    exit 0
}

# 没给路径就按"当前编译产物 → 已安装位置"的顺序找一遍
if (-not $ExePath) {
    $candidates = @(
        (Join-Path $PSScriptRoot 'target\release\VectorTerm.exe'),
        (Join-Path $PSScriptRoot 'target\debug\VectorTerm.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\VectorTerm\VectorTerm.exe'),
        (Join-Path $env:ProgramFiles 'VectorTerm\VectorTerm.exe')
    )
    $ExePath = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if (-not $ExePath) {
    throw '找不到 VectorTerm.exe,请用 -ExePath 指定完整路径'
}
if (-not (Test-Path $ExePath)) {
    throw "路径不存在:$ExePath"
}
$ExePath = (Resolve-Path $ExePath).Path
Write-Step "目标程序:$ExePath"

foreach ($v in $verbs) {
    $key = Join-Path $base ($v.Key + '\shell\VectorTerm')
    $cmdKey = Join-Path $key 'command'
    New-Item -Path $cmdKey -Force | Out-Null
    Set-ItemProperty -Path $key -Name '(default)' -Value '在此处打开 VectorTerm'
    Set-ItemProperty -Path $key -Name 'Icon' -Value "`"$ExePath`",0"
    if ($v.Multi) {
        # 选中多个文件夹时只认得第一个路径,干脆不显示
        Set-ItemProperty -Path $key -Name 'MultiSelectModel' -Value 'Single'
    }
    # 参数结尾的 `.` 是绕开命令行转义的:盘根 `%1` 是 `D:\`,直接拼成 `--cwd "D:\"`
    # 会让 Windows 把 `\"` 当转义引号,整条命令行都拆错。程序侧会把补的点剪掉。
    Set-ItemProperty -Path $cmdKey -Name '(default)' -Value "`"$ExePath`" --cwd `"$($v.Arg)\.`""
    Write-Step "已注册 $($v.Key)"
}

Write-Step '完成。在文件夹空白处 / 文件夹上 / 盘符上右键即可看到菜单项。'
Write-Step 'Windows 11 若只在「显示更多选项」里见到它,属正常。'
