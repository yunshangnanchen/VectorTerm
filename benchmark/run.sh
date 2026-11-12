#!/usr/bin/env bash
# VectorTerm 性能基准一键复现(Git Bash / MSYS)。Windows 下完整版见 run.ps1。
# 用法: bash benchmark/run.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
data="$root/benchmark/data"
cfg="$root/benchmark/configs"
cd "$root"
mkdir -p "$data"

# All cargo work goes through build.cmd (MSVC env + cc-rs fix), so there is
# no need to duplicate PATH / CRATE_CC_NO_DEFAULTS here.
export VECTORTERM_BENCH_MIB=2
build() { MSYS_NO_PATHCONV=1 cmd.exe /d /c "build.cmd $*"; }

echo "=== 1/4 生成固定素材 ==="
esc=$'\033'
unit="${esc}[38;5;196mW${esc}[0m${esc}[48;5;24;38;5;231mM${esc}[0m"
line=""
for _ in $(seq 1 100); do line+="$unit"; done
: > "$data/bench-screen.txt"
for _ in $(seq 1 60); do printf '%s\r\n' "$line" >> "$data/bench-screen.txt"; done
printf 'bench-screen.txt: %s bytes\n' "$(wc -c < "$data/bench-screen.txt")"

if [[ ! -s "$data/bench-payload.txt" ]]; then
  line2="The quick brown fox jumps over the lazy dog 0123456789 abcdefghijklmnop"
  : > "$data/bench-payload.txt"
  for _ in $(seq 1 28000); do printf '%s\r\n' "$line2" >> "$data/bench-payload.txt"; done
fi

echo "=== 2/4 编译 release ==="
build --release
build test --release --no-run

echo "=== 3/4 进程内基准 ==="
stamp="$(date +%Y%m%d-%H%M%S)"
build test --release bench_ -- --ignored --nocapture 2>&1 | tee "$data/run-$stamp.txt"

echo "=== 4/4 真实窗口满载帧率 ==="
exe="$root/target/release/VectorTerm.exe"
: > "$data/profile-$stamp.txt"
for size in 960x640 1920x1080; do
  for mode in gpu cpu; do
    for i in 1 2; do
      res="$(VECTORTERM_PROFILE_FRAMES=300 VECTORTERM_PROFILE_SIZE="$size" \
        timeout 120 "$exe" --config "$cfg/bench-$mode.lua" 2>&1 \
        | grep -E '\[profile\]' | tr '\n' ' ' || true)"
      echo "$size $mode run$i $res" | tee -a "$data/profile-$stamp.txt"
    done
  done
done

echo "完成。CSV -> benchmark/data/bench-frames.csv、bench-parse.csv"
