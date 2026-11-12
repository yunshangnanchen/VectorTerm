# 内嵌字体

这些字体用 `include_bytes!` 编进 `VectorTerm.exe`，运行时不依赖系统字体目录。
口径与 wezterm 一致：JetBrains Mono 作主字体，Nerd Font Symbols 作图标回退，
Noto Color Emoji 作彩色 emoji 回退。

| 文件 | 家族 / 用途 | 来源 | 许可证 |
|---|---|---|---|
| `JetBrainsMono-{Regular,Bold,Italic,BoldItalic}.ttf` | JetBrains Mono，正文四个样式槽 | wezterm `assets/fonts/` | `LICENSE_OFL.txt`（SIL OFL 1.1） |
| `SymbolsNerdFontMono-Regular.ttf` | Nerd Font Symbols，PUA 图标 | wezterm `assets/fonts/` | `LICENSE_OFL.txt` + `LICENSE_POWERLINE_EXTRA.txt` |
| `NotoColorEmoji.ttf` | Noto Color Emoji，彩色 emoji（CBDT/CBLC） | googlefonts/noto-emoji `2D/fonts/` | `LICENSE_NOTO_EMOJI.txt`（SIL OFL 1.1） |

来源地址：

- <https://github.com/wezterm/wezterm/tree/main/assets/fonts>
- <https://github.com/googlefonts/noto-emoji/blob/main/2D/fonts/NotoColorEmoji.ttf>

## 为什么 emoji 这份不是 wezterm 仓库里的那个文件

wezterm 打包的是 Noto Color Emoji 的 **COLRv1** 构建（矢量渐变，4.8 MB）：
base glyph 没有轮廓，颜色由 PaintGlyph / PaintSolid / PaintLinearGradient /
PaintTransform / PaintComposite 等十来种 Paint 结构叠出来。要在终端里复现，
等于自写一套矢量彩色字体渲染器。

这里改用同一字体家族的官方 **CBDT** 构建（10.7 MB）：每个 emoji 就是一张
PNG，运行时按 `CBLC` 的索引从 `CBDT` 取出、解码、缩到当前字号即可
（见 `src/emoji.rs`）。显示效果一致，代价只是二进制大几 MB。

CBDT 字体没有 `glyf` 轮廓，`ab_glyph` 读不到字形，所以它**不进普通文本
回退链**，由渲染器单独分派：遇到彩色 emoji 时发一条 `Frame::image`，
CPU / GPU 两条后端都从图片缓存取 RGBA 位图。

目前按**单个码点**取位图,不做 harfbuzz  shaping:ZWJ 组合 emoji(👨‍👩‍👧)、
肤色修饰符、国旗双字母序列会显示成各自的单字图,而不是合成一个字形。
普通单码点 emoji 不受影响。

## 分发注意

SIL OFL 1.1 要求随字体一起提供版权声明与许可证文本。发布 `VectorTerm.exe` 时
请把本目录的三个 `LICENSE_*.txt` 一起放进发行包（或 `licenses/` 目录）。
