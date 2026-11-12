#!/usr/bin/env python3
"""从 Nerd Font 字体文件生成 `src/nerdfonts.rs`。

wezterm 的 `wezterm.nerdfonts` 是一张"字形名 → 私用区字符"的大表,来自
Nerd Fonts 仓库的 `glyphnames.json`。VectorTerm 不联网构建,改从**已安装的字体**
本身取数据:TrueType 的 `post` 表存了每个字形的名字(`md-weather_night`),
`cmap` 表存了码位,两者一拼就是同一份映射,而且和用户屏幕上真正渲染的那版
字体完全一致。

字形名转成 wezterm 的键名只做一次替换:`-` → `_`。于是 `md-weather_night`
变成 `md_weather_night`,和 `nf.md_weather_night` 对得上。

用法:
    python tools/gen_nerdfonts.py                    # 自动找本机 MesloLGLNerdFont
    python tools/gen_nerdfonts.py --font X.ttf       # 指定字体
    python tools/gen_nerdfonts.py --out src/x.rs     # 指定输出
"""

from __future__ import annotations

import argparse
import glob
import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_OUT = os.path.join(ROOT, "src", "nerdfonts.rs")

# Windows 用户级 / 系统级字体目录;也允许相对当前仓库
FONT_GLOBS = [
    os.path.expandvars(r"%LOCALAPPDATA%\Microsoft\Windows\Fonts\*NerdFont-Regular.ttf"),
    os.path.expandvars(r"%WINDIR%\Fonts\*NerdFont-Regular.ttf"),
    "*.ttf",
]


def table_dir(data: bytes) -> dict[str, tuple[int, int]]:
    count = struct.unpack_from(">H", data, 4)[0]
    out = {}
    for i in range(count):
        off = 12 + i * 16
        tag = data[off : off + 4].decode("latin1")
        out[tag] = struct.unpack_from(">II", data, off + 8)
    return out


def glyph_names(data: bytes, tables: dict[str, tuple[int, int]]) -> list[str | None]:
    """`post` 表 2.0:前 32 字节头 + numGlyphs + glyphNameIndex + Pascal 字符串池。"""
    if "post" not in tables:
        raise SystemExit("字体没有 post 表,读不到字形名")
    off, length = tables["post"]
    if struct.unpack_from(">I", data, off)[0] != 0x00020000:
        raise SystemExit("post 表不是 2.0 版(字体未带字形名),无法生成")
    count = struct.unpack_from(">H", data, off + 32)[0]
    index = struct.unpack_from(">" + "H" * count, data, off + 34)
    pos = off + 34 + 2 * count
    pool: list[str] = []
    while pos < off + length:
        n = data[pos]
        pos += 1
        pool.append(data[pos : pos + n].decode("latin1"))
        pos += n
    # 0..257 是 Mac 标准顺序,字体文件里不重复存;这里只需要自定义的那部分
    return [pool[i - 258] if i >= 258 else None for i in index]


def codepoints(data: bytes, tables: dict[str, tuple[int, int]]) -> dict[int, int]:
    """gid → 码位。同一个字形挂在多个码位时取最小的那个(和字体习惯一致)。"""
    base = tables["cmap"][0]
    count = struct.unpack_from(">H", data, base + 2)[0]
    subtables = []
    for i in range(count):
        _pid, _eid, sub = struct.unpack_from(">HHI", data, base + 4 + i * 8)
        fmt = struct.unpack_from(">H", data, base + sub)[0]
        if fmt in (4, 12):
            subtables.append((fmt, base + sub))
    if not subtables:
        raise SystemExit("字体没有 format 4 / 12 的 cmap 子表")
    # format 12 覆盖 BMP 之外的平面,优先用它
    fmt, off = next((s for s in subtables if s[0] == 12), subtables[0])

    cp_to_gid: dict[int, int] = {}
    if fmt == 12:
        groups = struct.unpack_from(">I", data, off + 12)[0]
        for i in range(groups):
            start, end, gid = struct.unpack_from(">III", data, off + 16 + i * 12)
            for cp in range(start, end + 1):
                cp_to_gid[cp] = gid + (cp - start)
    else:
        seg_x2 = struct.unpack_from(">H", data, off + 6)[0]
        segs = seg_x2 // 2
        ends = struct.unpack_from(">" + "H" * segs, data, off + 14)
        starts = struct.unpack_from(">" + "H" * segs, data, off + 16 + seg_x2)
        deltas = struct.unpack_from(">" + "h" * segs, data, off + 16 + seg_x2 * 2)
        ro_base = off + 16 + seg_x2 * 3
        ros = struct.unpack_from(">" + "H" * segs, data, ro_base)
        for i in range(segs):
            for cp in range(starts[i], ends[i] + 1):
                if cp == 0xFFFF:
                    continue
                if ros[i] == 0:
                    gid = (cp + deltas[i]) & 0xFFFF
                else:
                    addr = ro_base + 2 * i + ros[i] + 2 * (cp - starts[i])
                    gid = struct.unpack_from(">H", data, addr)[0]
                    if gid:
                        gid = (gid + deltas[i]) & 0xFFFF
                if gid:
                    cp_to_gid[cp] = gid

    gid_to_cp: dict[int, int] = {}
    for cp, gid in cp_to_gid.items():
        if gid not in gid_to_cp or cp < gid_to_cp[gid]:
            gid_to_cp[gid] = cp
    return gid_to_cp


def find_font() -> str:
    for pattern in FONT_GLOBS:
        hits = sorted(glob.glob(pattern))
        if hits:
            return hits[0]
    raise SystemExit("没找到 Nerd Font,请用 --font 指定 .ttf")


def main() -> int:
    ap = argparse.ArgumentParser(description="生成 src/nerdfonts.rs")
    ap.add_argument("--font", help="Nerd Font 的 .ttf 路径")
    ap.add_argument("--out", default=DEFAULT_OUT, help="输出文件")
    args = ap.parse_args()

    path = args.font or find_font()
    with open(path, "rb") as f:
        data = f.read()
    tables = table_dir(data)
    names = glyph_names(data, tables)
    gid_to_cp = codepoints(data, tables)

    table: dict[str, int] = {}
    for gid, name in enumerate(names):
        if not name:
            continue
        cp = gid_to_cp.get(gid)
        if cp is None:
            continue
        table[name.replace("-", "_")] = cp

    # 私用区之外的不算图标(比如 .notdef / 连字辅助字形),一并剔掉
    table = {
        k: v for k, v in table.items() if 0xE000 <= v <= 0xF8FF or v >= 0xF0000
    }
    if not table:
        raise SystemExit("一个图标都没解出来,字体可能不是 Nerd Font")

    lines = [
        "//! Nerd Fonts 字形表(由 `tools/gen_nerdfonts.py` 生成,勿手改)。",
        "//!",
        f"//! 源字体:`{os.path.basename(path)}`。",
        f"//! 共 {len(table)} 个字形,键名与 wezterm 的 `wezterm.nerdfonts.*` 一致:",
        "//! 字体里叫 `md-weather_night`,这里就是 `md_weather_night`。",
        "",
        "/// 按名字升序排列,查询走二分。",
        f"static GLYPHS: [(&str, u32); {len(table)}] = [",
    ]
    for name in sorted(table):
        lines.append(f'    ("{name}", 0x{table[name]:05X}),')
    lines += [
        "];",
        "",
        "/// 按名字取字形。未收录的名字返回 `None`(调用方会给空串)。",
        "pub fn lookup(name: &str) -> Option<char> {",
        "    let i = GLYPHS.binary_search_by(|(n, _)| (*n).cmp(name)).ok()?;",
        "    char::from_u32(GLYPHS[i].1)",
        "}",
        "",
        "#[cfg(test)]",
        "mod tests {",
        "    use super::*;",
        "",
        "    #[test]",
        "    fn table_is_sorted_for_binary_search() {",
        "        assert!(GLYPHS.windows(2).all(|w| w[0].0 < w[1].0), \"表必须严格升序\");",
        "    }",
        "",
        "    #[test]",
        "    fn well_known_glyphs_resolve() {",
        "        // 这几个是状态栏示例里用到的,码位取自 Nerd Fonts 的 Material Design 段",
        "        assert_eq!(lookup(\"md_weather_night\"), Some('\\u{f0594}'));",
        "        assert_eq!(lookup(\"md_battery\"), Some('\\u{f0079}'));",
        "        assert_eq!(lookup(\"md_sleep\"), Some('\\u{f04b2}'));",
        "        assert_eq!(lookup(\"no_such_glyph_at_all\"), None);",
        "    }",
        "}",
        "",
    ]
    with open(args.out, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines))

    print(f"{path}\n  -> {args.out}\n  {len(table)} 个字形, {os.path.getsize(args.out) // 1024} KB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
