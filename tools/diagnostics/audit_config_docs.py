#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""配置文档一致性审计(只读)。

对照三份事实来源,检查文档有没有漏项 / 过时说法:

  A. settings-gui/src/schema.rs 里声明的每个 Field(点分路径)
     -> 是否出现在 docs/config-manual.md 的 §19 全量模板 / §22 默认值总表,
        以及 config.example.lua 里。
  B. src/config.rs 真正解析的每个 Lua 键
     -> 是否至少在 config-manual.md 正文(§1-§18)或示例配置里被提到。
        (只提示,不作硬性失败:wezterm 兼容别名集中在 §17.2 说明。)
  C. 已移除的功能有没有在文档里留下"还能用"的过时说法。

用法: python target/audit_config_docs.py
退出码 0 = A、C 全过;B 的提示不影响退出码。
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SCHEMA = ROOT / "settings-gui/src/schema.rs"
CONFIG_RS = ROOT / "src/config.rs"
MANUAL = ROOT / "docs/config-manual.md"
EXAMPLE = ROOT / "config.example.lua"
DOCS = [ROOT / "README.md", *sorted((ROOT / "docs").glob("*.md"))]


def read(p: Path) -> str:
    return p.read_text(encoding="utf-8")


def schema_fields(src: str):
    """从 Field { ... } 块里抓 path。"""
    return re.findall(r"Field\s*\{.*?\bpath:\s*\"([^\"]+)\"", src, re.S)


def rust_keys(src: str):
    """抓 config.rs 里真正从 Lua 表读的键名。"""
    keys = set()
    # tbl.get("key") / t.get::<Option<T>>("key")
    for m in re.finditer(
        r'\.get(?:::<[^>]*>)?\(\s*"([A-Za-z_][A-Za-z0-9_]*)"\s*\)', src
    ):
        keys.add(m.group(1))
    # str_field(&t, "key", ...) / opt_f64_field / usize_field ...
    for m in re.finditer(
        r'\b(?:opt_)?(?:str|f64|usize|bool|int)_field\s*\([^,]+,\s*'
        r'"([A-Za-z_][A-Za-z0-9_]*)"',
        src,
    ):
        keys.add(m.group(1))
    # opt_str(&c, "selection_bg") 之类
    for m in re.finditer(
        r'\bopt_str\s*\([^,]+,\s*"([A-Za-z_][A-Za-z0-9_]*)"', src
    ):
        keys.add(m.group(1))
    # for (key, slot) in [ ("click_to_focus", &mut f.xxx), ... ]
    for m in re.finditer(r'\(\s*"([a-z_][a-z0-9_]*)"\s*,\s*&mut\s+', src):
        keys.add(m.group(1))
    return keys


def has_field(text: str, path: str) -> bool:
    """配置模板 / 示例里出现 `key =` 即算覆盖。允许 1~2 层注释:
    `-- key =` 是注释掉的示例,`-- -- key =` 是注释块里的可选写法。"""
    last = path.split(".")[-1]
    return (
        re.search(
            r"(?m)^\s*(?:--\s*){0,2}" + re.escape(last) + r"\s*=", text
        )
        is not None
    )


def has_default_row(text: str, path: str) -> bool:
    """默认值总表:完整点分路径,或表格行里带反引号的键名(允许
    `| `a` / `b` |` 这种合并行)。"""
    last = path.split(".")[-1]
    if path in text:
        return True
    return any(
        line.startswith("|") and re.search(r"`" + re.escape(last) + r"`", line)
        for line in text.splitlines()
    )


def main():
    schema = read(SCHEMA)
    manual = read(MANUAL)
    example = read(EXAMPLE)

    i19 = manual.find("## 19 全量配置模板")
    i20 = manual.find("## 20 常见配方")
    i22 = manual.find("## 22 默认值总表")
    body = manual[:i19] if i19 > 0 else manual
    sec19 = manual[i19:i20] if i19 > 0 and i20 > i19 else ""
    sec22 = manual[i22:] if i22 > 0 else ""

    fields = schema_fields(schema)
    print(f"schema 字段数: {len(fields)}")

    miss19 = [p for p in fields if not has_field(sec19, p)]
    miss22 = [p for p in fields if not has_default_row(sec22, p)]
    missex = [p for p in fields if not has_field(example, p)]

    def show(title, items):
        print(f"\n=== {title} ({len(items)}) ===")
        for p in items:
            print("  ", p)

    show("§19 全量模板未包含", miss19)
    show("§22 默认值总表未包含", miss22)
    show("config.example.lua 未包含", missex)

    # B: 代码解析键 -> 手册正文 / 示例(提示性)
    keys = rust_keys(read(CONFIG_RS))
    corpus = body + "\n" + sec19 + "\n" + sec22 + "\n" + example
    uncovered = sorted(k for k in keys if not re.search(r"(?m)`" + re.escape(k) + r"`|(?<![\w])" + re.escape(k) + r"\s*=", corpus))
    print(f"\n=== config.rs 解析键 {len(keys)} 个;手册/示例均未出现 ({len(uncovered)},仅提示) ===")
    print("  " + (" ".join(uncovered) if uncovered else "无"))

    # C: 过时说法
    stale = {
        "files-gui.exe 仍会构建": r"还会构建第二个可执行文件\s*\*\*`files-gui\.exe`",
        "src/bin/files_gui.rs 仍存在": r"`src/bin/files_gui\.rs`",
        "全屏文件管理器": r"全屏文件管理器",
        "/files 开全屏": r"开全屏文件管理器",
        "pane 尺寸调整暂未实现": r"尺寸调整.{0,8}暂未实现",
    }
    hits = []
    for p in DOCS:
        t = read(p)
        for label, pat in stale.items():
            for m in re.finditer(pat, t):
                line = t.count("\n", 0, m.start()) + 1
                hits.append(f"{p.relative_to(ROOT)}:{line} {label}")
    print(f"\n=== 过时说法 ({len(hits)}) ===")
    for h in hits:
        print("  ", h)

    ok = not any((miss19, miss22, missex, hits))
    print("\n结果:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
