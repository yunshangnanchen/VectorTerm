#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""检查 README.md 与 docs/*.md 里的本地链接 / 标题锚点。

- `#锚点`        -> 在当前文档里找标题
- `文件.md#锚点` -> 先确认文件存在,再到那份文档里找标题
- 其余相对路径   -> 确认文件 / 目录存在
锚点用与 GitHub 一致的简化 slug 规则(去掉反引号与标点、空格转连字符)。

用法: python target/check_md_links.py   (问题数非 0 时退出 1)
"""
import pathlib
import re
import sys

# GitHub Actions 的 windows-latest 上 Python 把重定向的 stdout 当成 cp1252,
# 直接 print 中文会抛 UnicodeEncodeError —— 那时脚本其实一条链接都还没查。
# 脚本自己固定 UTF-8,比在 workflow 里设 PYTHONIOENCODING 更靠得住:
# 本地直接跑、CI 里跑、别人拿去跑都是同一份输出。
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8", errors="backslashreplace")

root = pathlib.Path(".")
docs = [root / "README.md", *sorted((root / "docs").glob("*.md"))]


def slug(h):
    s = h.strip().lower().replace("`", "")
    out = []
    for ch in s:
        if ch.isalnum() or ch in (" ", "-", "_") or ord(ch) > 0x2E80:
            out.append(ch)
    return "".join(out).replace(" ", "-")


def heads_of(path):
    text = path.read_text(encoding="utf-8")
    return {
        slug(m.group(2))
        for m in re.finditer(r"^(#{1,6})\s+(.*)$", text, re.M)
    }


heads_by_file = {p.resolve(): heads_of(p) for p in docs}
bad = []

for p in docs:
    text = p.read_text(encoding="utf-8")
    for m in re.finditer(r"\[[^\]]*\]\(([^)]+)\)", text):
        target = m.group(1).strip()
        if target.startswith(("http://", "https://", "mailto:")):
            continue
        line = text.count("\n", 0, m.start()) + 1
        if target.startswith("#"):
            if target[1:] not in heads_by_file[p.resolve()]:
                bad.append((p, line, "anchor", target))
            continue
        if "#" in target:
            filepart, anchor = target.split("#", 1)
        else:
            filepart, anchor = target, None
        dest = (p.parent / filepart).resolve() if filepart else p.resolve()
        if not dest.exists():
            bad.append((p, line, "file", target))
            continue
        if anchor and dest.suffix == ".md":
            key = dest.resolve()
            if key not in heads_by_file:
                heads_by_file[key] = heads_of(dest)
            if anchor not in heads_by_file[key]:
                bad.append((p, line, "anchor", target))

all_heads = set().union(*heads_by_file.values())
print("文档数:", len(docs))
print("标题数:", len(all_heads))
print("问题:", len(bad))
for p, line, kind, t in bad:
    print(f"   {p}:{line} {kind} {t}")
sys.exit(1 if bad else 0)
