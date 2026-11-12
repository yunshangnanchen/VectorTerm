#!/bin/bash
# VectorTerm 引导脚本:拉取 wezterm-term 及其仓库内依赖(浅克隆 + sparse-checkout)
set -e
TAG=20240203-110809-5046fc22
DIRS="term termwiz bidi wezterm-dynamic color-types wezterm-blob-leases wezterm-input-types filedescriptor vtparse"

# 上游补丁(幂等):git apply --check 通过说明尚未打过,打一次即可。
# 目前只有 wezterm-term 的 with_phys_lines 回绕越界修复。
apply_patches() {
  for p in patches/*.patch; do
    [ -f "$p" ] || continue
    if git -C .wezterm-src apply --check "$p" >/dev/null 2>&1; then
      git -C .wezterm-src apply "$p"
      echo "已应用上游补丁:$p"
    fi
  done
}

if [ -f .wezterm-src/term/src/lib.rs ]; then
  echo ".wezterm-src 已就绪,检查上游补丁"
  apply_patches
  exit 0
fi

rm -rf .wezterm-src
git clone --depth 1 --filter=blob:none --sparse --branch "$TAG" https://github.com/wezterm/wezterm .wezterm-src
(
  cd .wezterm-src
  git sparse-checkout set $DIRS
  rm -f Cargo.toml Cargo.lock
)
apply_patches
echo "wezterm-term 源码就绪 (tag $TAG)"
