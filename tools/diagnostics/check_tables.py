import re, pathlib, sys

# 和另两个审计脚本一样:runner 的 stdout 是 cp1252,print 中文会崩。
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8", errors="backslashreplace")

lines = pathlib.Path("docs/config-manual.md").read_text(encoding="utf-8").splitlines()

def cells(line):
    s = line.strip()
    s = s.replace("\\|", "\x00")
    s = s.strip("|")
    return [c for c in s.split("|")]

issues = []
i = 0
in_code = False
while i < len(lines):
    ln = lines[i]
    if ln.strip().startswith("```"):
        in_code = not in_code
        i += 1
        continue
    if not in_code and ln.strip().startswith("|") and i + 1 < len(lines) and re.match(r"^\|[\s:\-|]+\|$", lines[i+1].strip()):
        header = cells(ln)
        j = i + 2
        while j < len(lines) and lines[j].strip().startswith("|"):
            row = cells(lines[j])
            if len(row) != len(header):
                issues.append((j + 1, len(header), len(row), lines[j][:60]))
            j += 1
        i = j
        continue
    i += 1

print("表格列数异常:", len(issues))
for it in issues:
    print("  行%d 期望%d列 实际%d列: %s" % it)
# 以前这里只打印不退出,CI 的 "if ($LASTEXITCODE -ne 0)" 永远为真不了 ——
# 那一步等于空跑。现在真有问题就把流水线拦下来。
sys.exit(1 if issues else 0)
