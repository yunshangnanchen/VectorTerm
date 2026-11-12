//! 构建期资源:把 `assets/icons/VectorTerm.ico` 编进 exe 当文件图标,再写一份 VERSIONINFO
//! (资源管理器"属性 → 详细信息"里能看到产品名/版本/版权)。
//!
//! 两条路,按机器上有没有 Windows SDK 自动选:
//!
//! 1. **有 `rc.exe`**:写一份 `.rc`,让 SDK 编出 `.res` —— 图标和版本信息都齐;
//! 2. **没有 `rc.exe`**:自己往 `.res` 里写图标资源(格式就那几个 DWORD,见 [`write_res`])。
//!    图标从此不依赖 SDK,只有版本信息会缺 —— 这种情况下会打印一条 warning。
//!
//! 不引入 `winres` / `embed-resource` 这类 crate,行为全在眼皮底下。

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `RT_ICON`:单帧图像。`RT_GROUP_ICON`:图标组(资源管理器认的是这一条)。
const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
/// 资源语言:en-US。写成 0(中性)也能用,但部分工具按语言查会落空。
const LANG_EN_US: u16 = 0x0409;

fn main() {
    println!("cargo:rerun-if-changed=assets/icons/VectorTerm.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // 交叉编译到别的 Windows 目标时资源格式仍然通用,但不折腾了
    if env::var("HOST") != env::var("TARGET") {
        return;
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest.join("assets").join("icons").join("VectorTerm.ico");
    if !ico.is_file() {
        println!(
            "cargo:warning=assets/icons/VectorTerm.ico 不存在,跳过 exe 图标(跑 tools/gen_icons.py)"
        );
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let res = out_dir.join("VectorTerm.res");

    // 1) 有 SDK 就走 rc.exe:同一个 .res 里既有图标也有版本信息。
    if let Some(rc) = find_rc() {
        let rc_file = out_dir.join("VectorTerm.rc");
        match fs::write(&rc_file, rc_source(&ico)) {
            Ok(()) => {
                match Command::new(&rc)
                    .arg("/nologo")
                    .arg("/fo")
                    .arg(&res)
                    .arg(&rc_file)
                    .output()
                {
                    Ok(o) if o.status.success() && res.is_file() => {
                        link(&manifest, &res);
                        return;
                    }
                    Ok(o) => {
                        let msg = String::from_utf8_lossy(&o.stderr);
                        println!(
                            "cargo:warning=rc.exe 失败({}),改用内置 .res 写入",
                            msg.trim()
                        );
                    }
                    Err(e) => println!("cargo:warning=调用 rc.exe 失败: {e},改用内置 .res 写入"),
                }
            }
            Err(e) => println!("cargo:warning=写 VectorTerm.rc 失败: {e},改用内置 .res 写入"),
        }
    }

    // 2) 没有 SDK(或 rc 失败):自己拼 .res。图标照样进 exe,只是没有 VERSIONINFO。
    match fs::read(&ico)
        .map_err(|e| e.to_string())
        .and_then(|b| parse_ico(&b))
    {
        Ok(frames) => match write_res(&res, &frames) {
            Ok(()) => {
                println!(
                    "cargo:warning=没找到 rc.exe(Windows SDK):图标已由 build.rs 直接写入,\
                     但 exe 没有版本信息。装 SDK 或用 build.cmd 构建可补齐"
                );
                link(&manifest, &res);
            }
            Err(e) => println!("cargo:warning=写 VectorTerm.res 失败: {e};exe 将不带图标"),
        },
        Err(e) => println!("cargo:warning=解析 VectorTerm.ico 失败: {e};exe 将不带图标"),
    }
}

/// 把 `.res` 作为一条链接参数交给链接器。
///
/// 必须传**相对包根目录**的路径:rustc 会把 `-C link-arg=` 的值按空白拆开、且不认引号,
/// 仓库路径一旦含空格(比如放在 `F:\Code\my project\` 下),绝对路径就会被切成
/// `F:\Code\my` + `project\...`,链接直接失败。link.exe 的工作目录就是包根
/// (cargo 调 rustc 时设的),所以 `target\...\VectorTerm.res` 能解析。
fn link(manifest: &Path, res: &Path) {
    let rel = res.strip_prefix(manifest).unwrap_or(res);
    let shown = rel.display().to_string();
    if shown.contains(' ') {
        println!("cargo:warning=构建目录路径含空格({shown}),链接器无法接收 .res,exe 将不带图标");
        return;
    }
    println!("cargo:rustc-link-arg={shown}");
}

// ---------------------------------------------------------------- 自己写 .res

/// 把 ICO 拆成 `(边长, PNG 字节)` 列表。
fn parse_ico(bytes: &[u8]) -> Result<Vec<(u32, Vec<u8>)>, String> {
    if bytes.len() < 6 {
        return Err("文件太短".into());
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        let e = 6 + i * 16;
        if e + 16 > bytes.len() {
            return Err(format!("第 {i} 个目录项越界"));
        }
        // 宽高各占 1 字节,0 表示 256
        let w = if bytes[e] == 0 { 256 } else { bytes[e] as u32 };
        let h = if bytes[e + 1] == 0 {
            256
        } else {
            bytes[e + 1] as u32
        };
        let size = u32::from_le_bytes(bytes[e + 8..e + 12].try_into().unwrap()) as usize;
        let off = u32::from_le_bytes(bytes[e + 12..e + 16].try_into().unwrap()) as usize;
        if off + size > bytes.len() {
            return Err(format!("第 {i} 帧数据越界"));
        }
        frames.push((w.max(h), bytes[off..off + size].to_vec()));
    }
    if frames.is_empty() {
        return Err("一帧都没有".into());
    }
    Ok(frames)
}

/// 写出一个只含图标的 `.res`(RES 格式,link.exe 直接吃)。
///
/// 文件布局:先是资源 #0 —— 一条"空资源",`DataSize=0, HeaderSize=32`,后面 24 字节
/// 零。rc.exe 编出来的 `.res` 都带这一条,cvtres 也要求它;写成纯 32 个零
/// (HeaderSize=0)会被判 `CVT1107 文件损坏`。之后才是真正的条目。
fn write_res(path: &Path, frames: &[(u32, Vec<u8>)]) -> io::Result<()> {
    let mut out = Vec::with_capacity(64 + frames.iter().map(|f| f.1.len() + 32).sum::<usize>());
    out.extend_from_slice(&0u32.to_le_bytes()); // DataSize
    out.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize(空资源的头本身)
    out.resize(32, 0);

    let mut group = Vec::with_capacity(6 + 14 * frames.len());
    group.extend_from_slice(&0u16.to_le_bytes()); // reserved
    group.extend_from_slice(&1u16.to_le_bytes()); // type = 1(图标)
    group.extend_from_slice(&(frames.len() as u16).to_le_bytes());

    for (i, (side, png)) in frames.iter().enumerate() {
        let id = (i + 1) as u16;
        push_entry(&mut out, RT_ICON, id as u32, png);
        group.push((side % 256) as u8); // 宽
        group.push((side % 256) as u8); // 高
        group.push(0); // 调色板数目
        group.push(0); // 保留
        group.extend_from_slice(&1u16.to_le_bytes()); // planes
        group.extend_from_slice(&32u16.to_le_bytes()); // 位深
        group.extend_from_slice(&(png.len() as u32).to_le_bytes());
        group.extend_from_slice(&id.to_le_bytes());
    }
    push_entry(&mut out, RT_GROUP_ICON, 1, &group);

    fs::write(path, out)
}

/// 追加一条资源条目(`IMAGE_RESOURCE_DATA_ENTRY` 前面的那 32 字节头)。
///
/// Type / Name 是"数值还是字符串"的可变槽位:第一个 WORD 是 `0xFFFF` 表示数值
/// (第二个 WORD 才是 id),否则这个 WORD 起就是 Unicode 字符串本身。写成
/// `0x8000_0000 | id` 会被 cvtres 当成字符串偏移,报 `CVT1107 文件损坏`。
fn push_entry(out: &mut Vec<u8>, ty: u32, name: u32, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // DataSize
    out.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize
    out.extend_from_slice(&ordinal(ty).to_le_bytes());
    out.extend_from_slice(&ordinal(name).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&0u16.to_le_bytes()); // MemoryFlags
    out.extend_from_slice(&LANG_EN_US.to_le_bytes()); // LanguageId
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

/// 把整数 id 编成 RES 的 "Ordinal" 形式:低 WORD 0xFFFF,高 WORD 是 id。
fn ordinal(id: u32) -> u32 {
    0xFFFF | (id << 16)
}

// ---------------------------------------------------------------- VERSIONINFO

/// `.rc` 内容:一个 ICON 资源 + 一份 VERSIONINFO。
fn rc_source(ico: &Path) -> String {
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let quad = {
        let mut parts: Vec<String> = version.split('.').map(|s| s.to_string()).collect();
        while parts.len() < 4 {
            parts.push("0".into());
        }
        parts.join(",")
    };
    // rc.exe 默认按本地代码页解析源文件;显式声明 UTF-8,中文才不会变乱码
    format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"

// 资源 id 必须是数字 1(不是名字 "VS_VERSION_INFO"),否则 GetFileVersionInfo
// 按 id 1 找不到,资源管理器"属性"里的产品名/版本全是空的。
1 VERSIONINFO
 FILEVERSION {quad}
 PRODUCTVERSION {quad}
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS 0x40004L
 FILETYPE 0x1L
 FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "VectorTerm"
      VALUE "FileDescription", "VectorTerm - Windows 终端模拟器"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "VectorTerm"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "VectorTerm.exe"
      VALUE "ProductName", "VectorTerm"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = ico.display().to_string().replace('\\', "\\\\"),
        version = version,
        quad = quad,
    )
}

/// 找 `rc.exe`:`RC` 环境变量 → PATH → vcvars 的 `WindowsSdkDir` → 默认安装目录(取最高版本)。
fn find_rc() -> Option<PathBuf> {
    if let Ok(p) = env::var("RC") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(p) = which("rc.exe") {
        return Some(p);
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(sdk) = env::var("WindowsSdkDir") {
        roots.push(PathBuf::from(sdk).join("bin"));
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Ok(pf) = env::var(var) {
            roots.push(
                PathBuf::from(pf)
                    .join("Windows Kits")
                    .join("10")
                    .join("bin"),
            );
        }
    }
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for root in roots {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let rc = dir.join("x64").join("rc.exe");
            if !rc.is_file() {
                continue;
            }
            let key: Vec<u64> = dir
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.split('.').filter_map(|p| p.parse().ok()).collect())
                .unwrap_or_default();
            if best.as_ref().map(|(k, _)| key > *k).unwrap_or(true) {
                best = Some((key, rc));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// 最小版 `which`:PATH 里逐目录找可执行文件。
fn which(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}
