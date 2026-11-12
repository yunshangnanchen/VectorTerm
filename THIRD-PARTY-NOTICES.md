# 第三方组件许可说明

VectorTerm 自身按 **PolyForm Noncommercial 1.0.0** 授权(见 `LICENSE`)。
但它编译时链接、运行依赖了大量第三方组件,这些组件**仍按各自许可分发** ——
本项目的许可不覆盖它们,也不会收紧它们原本给你的权利。

分发 `VectorTerm.exe`(本项目的二进制)时,请把本文件与 `LICENSE` 一起带上。

--------------------------------------------------------------------------------

## 一、绝大多数依赖:MIT 或 Apache-2.0

`Cargo.lock` 里约 200 个包声明为 `MIT OR Apache-2.0` 及其等价写法
(`Apache-2.0 OR MIT`、`MIT/Apache-2.0`、`MIT OR Apache-2.0 OR Zlib` 等),
意思是**两种(或三种)许可任选其一遵守**。典型代表:`winit`、`wgpu`、`naga`、
`softbuffer`、`windows-sys`、`serde`、`mlua`、`luajit` 绑定、`image`、`tokio`。

选择 MIT 时义务是保留版权声明与许可声明;选择 Apache-2.0 时义务是保留
`LICENSE`/`NOTICE` 并提供一份许可副本。原文可从各 crate 的仓库、
crates.io 页面或本地 `~/.cargo/registry/src/<名字>-<版本>/` 取得。

## 二、需要单独注意的许可

| 许可 | 组件(举例) |
|---|---|
| `Unicode-3.0` / `Unicode-DFS-2016` | `icu_*` 系列、`finl_unicode`、`unicode-*` |
| `BSL-1.0`(Boost) | `clipboard-win`、`error-code` |
| `Zlib` | `foldhash`、`bytemuck`、`cursor-icon`(多选一时) |
| `BSD-3-Clause` / `BSD-2-Clause` | `exr`、`lebe` |
| `0BSD` | `adler`、`adler2` |
| `ISC` | `libloading` |
| `CC0-1.0` | `hexf-parse` |
| `Apache-2.0 WITH LLVM-exception` | `ar_archive_writer` |
| `Unlicense OR MIT` | `memchr` 等 |

`Cargo.lock` 记录了每个组件的确切版本;要核对某个包的条款,查它的 `Cargo.toml`
里的 `license` 字段即可。

## 三、wezterm(MIT)

终端的核心状态机、转义序列处理、布局与大量行为语义来自 wezterm 的
`wezterm-term` / `termwiz`(本仓库把用到的部分放在 `.wezterm-src/`)。
它采用 MIT 许可,**分发二进制时必须随附下列声明**:

```text
MIT License

Copyright (c) 2018-Present Wez Furlong

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## 四、字体

程序**内嵌**了下面几款字体(`src/builtin_fonts.rs` 用 `include_bytes!` 编进
`VectorTerm.exe`),所以机器上一款字体都没装也能正常显示:

| 字体 | 用途 | 许可 |
|---|---|---|
| JetBrains Mono(Regular / Bold / Italic / BoldItalic) | 默认等宽正文 | SIL OFL 1.1 |
| Symbols Nerd Font Mono | Nerd Font 图标区(PUA) | SIL OFL 1.1(含 Powerline Extra 字形,MIT) |
| Noto Color Emoji(CBDT 构建) | 彩色 emoji | SIL OFL 1.1 |

SIL OFL 1.1 允许随软件再分发,条件是**随附版权声明与许可全文**,且不得单独
以字体名义售卖。全文放在 `assets/fonts/`:

- `LICENSE_OFL.txt` —— JetBrains Mono 与 Symbols Nerd Font Mono
- `LICENSE_NOTO_EMOJI.txt` —— Noto Color Emoji
- `LICENSE_POWERLINE_EXTRA.txt` —— Powerline Extra 字形(MIT)

安装包会把这些文件装到 `{app}\licenses\`;单独分发 `VectorTerm.exe` 时请把它们
一并带上。字体来源与替换原因见 `assets/fonts/README.md`。

渲染时仍然**优先用系统里已安装的字体**(配置项 `font.family`),内嵌字体是兜底 ——
显式指定字体文件路径时也不走内嵌那份。
