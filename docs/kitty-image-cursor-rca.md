# 首行输入错位:fastfetch 的 kitty 图片与图片放置的"光标落点"差一行

> 基线提交 `7da3db5`。**本文只做定位,不含任何代码改动。**
> 结论已用 PTY 原始字节、上游 crate 源码、四组对照实验交叉验证。

---

## 0. 结论先行

三句话:

1. **不是 PowerShell 美化的问题,也不是 vectorterm 的渲染问题,是"图片放置后光标落在哪一行"的认知不一致。**
   fastfetch 认为光标停在图片**最后一行**,vectorterm 实际把它放在了最后一行**的下一行**。
2. **只差 1 行,而且只在图片存在时差。** 那一行偏差来自 `wezterm-term` 的图片放置逻辑:给 `c=`/`r=` 指定显示格数后,它仍拿**源图像素尺寸**去和**格尺寸**比,判断"要不要再下移一格",几乎必然判成"要"。
3. **wezterm 同样中招**,因为它用的是同一份 `term` crate、同一段代码。用户观察到的"wezterm 也错"正是这个原因 —— 反而是最有力的旁证。

一句话因果链:

```text
fastfetch 发 kitty 图(c=29 r=25) + 假设"光标停在图片最后一行"
        ↓
wezterm-term 把光标放到"最后一行 + 1"(y_padding_shift)
        ↓
fastfetch 后续的 \r\n + ESC[25A 补偿,多退了一行 → 文字整体比图片低 1 行
        ↓
ConPTY 不认图片,自己的行号没有这 1 行偏移
        ↓
PSReadLine 按 ConPTY 的行号定位 → 首次输入画在"上一行"(视觉上是图片那一行的位置)
        ↓
回车换行后 PSReadLine 整块重排,偏差被吸收 → 恢复正常
```

---

## 1. 现象与复现

**现象**:打开 pwsh 7(profile 里跑 fastfetch,左侧有 kitty 立绘),出现提示符后**敲第一个字符**,字符显示在提示符的**上一行**;回车换行之后再输入就正常。

**复现**(vectorterm,release):

```powershell
# 终端里执行
VectorTerm.exe --config <一份普通配置>
# shell 起来后 fastfetch 出图,提示符出现,敲 abc
```

自动化复现(三档窗口宽度都复现):

| 组 | fastfetch | 首个输入 | 结果 |
|---|---|---|---|
| A | `fastfetch`(带 kitty 图) | `abc` | **abc 落在提示符上一行** |
| B | `fastfetch --logo none` | `abc` | 同一行,正常 |
| C | 只有 oh-my-posh,无 fastfetch | `abc` | 同一行,正常 |
| D | oh-my-posh + fastfetch | `abc` | **错位**(与 A 同) |

宽度 1250 / 1500 / 1700 三档全错位 → **与窗口宽度无关**(宽度只影响另一件事,见 §5)。
图片高度 10 / 25 行都错位;40 行时因内容溢出触发滚动,现象被掩盖。

---

## 2. 证据一:fastfetch 实际发出的字节

用配置项 `session_log`(`src/config.rs` §…,把 PTY 原始输出写文件)抓到的字节:

```text
\x1b_Ga=T,f=100,t=f,c=29,r=25;QzovVXNlcnMv...bG9nby1raXR0eS5wbmc=\x1b\
\r\n
\x1b[25A          ← 光标上移 25 行
\x1b[32C          ← 右移 32 列(29 列图 + 3 格间距)
   you @ your-pc
...
```

要点:

- `a=T` = 传输并显示;`c=29,r=25` = 显示成 29 列 × 25 行;**没有 `C=1`**(`C=1` 才是"显示后不移动光标")。
- 图片之后是 `\r\n` 再 `\x1b[25A`。这是 fastfetch 的定位套路:先下移一行,再上移 `r` 行,从而回到图片**第一行**。

**这只有在"图片放置后光标停在图片最后一行(第 24 行)"时才成立**:
`24 + 1(\r\n) - 25 = 0` ✓

如果终端把光标停在"最后一行 + 1"(第 25 行):
`25 + 1 - 25 = 1` ✗ —— 文字整体低一行,图片与文字错开。

---

## 3. 证据二:vectorterm 用的 term crate 正是"最后一行 + 1"

`.wezterm-src/term/src/terminalstate/image.rs:184`(`assign_image_to_cells`):

```rust
let y_padding_shift: i64 = one_or_zero(
    draw_height as usize + cell_padding_top as usize > cell_pixel_height * height_in_cells,
);

if !params.do_not_move_cursor {
    if bottom_right {
        self.set_cursor_pos(
            &Position::Relative(width_in_cells as i64 + x_padding_shift),
            &Position::Relative(y_padding_shift),   // ← 多下移一行
        );
    }
}
```

代入本机实测值(窗口 1200×800 像素 / 107 列 × 37 行,格宽 11px、格高 21px;图片 730×1250):

| 量 | 值 | 说明 |
|---|---|---|
| `draw_height` | 1250 | 源图像素高(`r=25` 只影响格数,不影响这个) |
| `cell_pixel_height * height_in_cells` | `21 × 25 = 525` | 25 行格子的物理高 |
| 判断 | `1250 + 0 > 525` → **真** | |
| `y_padding_shift` | **1** | 光标再多走一行 |

于是:循环里的 `new_line` 已经把光标带到第 24 行,`set_cursor_pos(.., Relative(1))` 又把它推到第 25 行。**比 fastfetch 的假设正好多一行。**

同一个函数里 `x_padding_shift` 也会被算成 1(`730 > 11×29=319`),光标多右移一列;但 fastfetch 每行都用 `\r` + `\x1b[32C` 从行首重定位,列偏移被覆盖掉了 —— 所以**只有行**上的偏差会留下来。

> 判定逻辑本身对"按像素显示、不指定 c/r"的图片是对的(那种情况 `draw_height` 与格高确实要比较)。
> 错的是**指定了 `r=` 之后**:图片注定被缩放到 25 行,此时该比的是"是否有不足一格的余数",而不是拿 1250px 去比 525px。这就是 `remainder_height_cell` 存在的意义,但 padding shift 没有用它。

---

## 4. 证据三:ConPTY 的行号确实比终端少 1

让 ConPTY 自己报行号(`[Console]::CursorTop`),同一份 fastfetch 内容:

| 组 | 报出的行号 | 说明 |
|---|---|---|
| 无图(`--logo none`) | **29** | 与视觉一致 |
| 带图 | **34** | 多 5 行 |

>`img` 组多出的 5 行是另一件事:图片占掉 32 列后文字区变窄,fastfetch 自己折了几行(截图上窗口最左侧那几处 `ARE` / `Hz` / `IME` 就是折行残尾)。这一项与首行错位**无关** —— 把窗口加宽到 1700(折行消失)之后,错位依然存在,只是幅度稳定为 1 行。

---

## 5. 为什么"只有第一行",回车后就好了

- 那 1 行的偏差**一直存在**(终端文本网格比 ConPTY 的记账低一行),但**整体平移是看不出来的** —— 除非屏幕上有个"参照物"。
- **图片就是那个参照物**:图片画在网格的第 0–24 行,文字被推到第 1 行起,人眼立刻看出"文字比图片低一行"。
- 提示符是这条错位链的最后一环:PSReadLine 缓存了提示符的起始位置,首次按键时按缓存位置重绘 → 字落在"提示符上一行"。图形上表现为提示符被拆成两截,或输入的字浮在上面。
- **回车之后** PSReadLine 把整块(提示符 + 输入)按当前行号重排,不再依赖那张已经"过期"的缓存 → 看起来恢复正常。图片早已滚出视野,参照物也没了。

---

## 6. 为什么 wezterm 也一样

上游 `wezterm` main 分支 `term/src/terminalstate/image.rs` 里 `y_padding_shift` / `x_padding_shift` 与上引代码**逐字相同**(另外新增了"格像素尺寸为 0 就拒绝放图"的保护,本项目这份 20240203 的 tag 还没有)。

所以这不是 vectorterm 独有的实现差异,而是 **wezterm 系终端的共同行为**:凡是"发完 kitty 图片、再按'光标停在图片最后一行'做相对定位"的程序,都会在 wezterm / vectorterm 上错一行;在 kitty 本体上不会(kitty 的光标语义与 fastfetch 的假设一致)。

这解释了为什么换终端也没用 —— **问题在 fastfetch 与 wezterm 系图片放置语义的接缝上**,不在 Shell 7 的美化,也不在 vectorterm 的渲染管线。

---

## 7. 影响面

| 受影响 | 说明 |
|---|---|
| 所有 kitty 图片 | 只要图片原始像素高 > 显示行数 × 格高(几乎总是,因为源图是超采样大图),`y_padding_shift` 必然为 1 |
| sixel / iTerm2 图片 | 同一个函数、同一段 padding shift → 同样多一行 |
| 只影响"图片之后紧跟的相对定位" | 用绝对定位(CSI row;col H)的程序不受影响,因为行号由程序显式给出 |
| 用户实际损失 | 仅"首行输入视觉错位" + "文字整体比图片低一行";不影响输入内容、不影响命令执行,回车即自愈 |

**不是 bug 的部分**:图片本身的位置正确、渲染正确、缩放正确(29 列 × 25 行),没有撕裂或偏移 —— 错的只有"图片之后光标算在哪"。

---

## 8. 修法选项(本轮不实施)

按代价从低到高:

1. **用户侧立刻可用**:把 fastfetch 的 logo 换成字符画 —— `~/.config/fastfetch/config.jsonc` 里已经写好备用方案 A:

   ```jsonc
   "logo": { "source": "C:/Users/you/.config/fastfetch/logo-girl.txt",
             "type": "file-raw", "padding": { "left": 1, "right": 2 } },
   ```

   `file-raw` 是纯文本(半块字符),不触发图片协议 → 没有这一行偏差。代价是分辨率降到 29×25 字符格。

2. **最干净**:在 `assign_image_to_cells` 里修 padding shift —— 当 `params.rows` / `params.columns` 明确给了格数时,余数已由 `remainder_*_cell` 表达,不应再加 shift:

   ```rust
   let y_padding_shift = one_or_zero(
       params.rows.is_none()
           && draw_height + cell_padding_top as usize > cell_pixel_height * height_in_cells,
   );
   ```

   这对本项目是 `.wezterm-src/term` 的打补丁(该目录是浅克隆的上游源码,已有本地补丁先例,见 `wezterm-term` 的 scrollback 补丁)。同时值得给 wezterm 上游提一个 issue / PR,否则每次升级 tag 都要重新打。

3. **不改上游的迂回**:在 vectorterm 侧解析完一段含图片的输出后,把光标上移一行。**不建议** —— 等于把上游的语义错误硬编码进渲染层,遇到 correctly-implemented 的图片序列(带 `C=1`)会反向出错。

### 8.1 实测补充:方案 1 的"只缩图、不换字符画"变体(已采用)

方案 1 不必退化成字符画 —— 只要**把 PNG 的像素高度压到 `行数 × 格高` 以下**,`y_padding_shift` 的判定自然为假,图片照旧是 PNG。

量测方法:用**纯色块** PNG(屏幕上的位置正好等于它的格子框)精确算行号,避开"文字顶端不在格线上"的误差;量两次重复一致。

| 源 PNG | 文字顶端相对图片顶端 | 判定 |
|---|---|---|
| 730×1250 | +0.85 格 | 低一行 |
| 300×500 | 0.0 格 | 对齐 |

同一张立绘的两个尺寸 A/B(内容相同,只差像素高,各跑两次):`450px → text_top=41`、`1250px → text_top=58`,稳定复现。

实测参数:字号 16 时格高约 20px,阈值 `25×20=500`;取 **18px/行 → 450px**,留 10% 余量,字号降一两档(→14)也不复发。`~/.config/fastfetch/make-logo.py` 的 `KITTY_PIXELS_PER_ROW` 已从 50 改为 18(注释里本来写的就是"压到行数×19px",常数却是 50,两者矛盾 —— 这就是问题起点)。

**"让终端自己按 PNG 调整行数"走不通**:kitty 协议里 `c=`/`r=` 由**发送方**给出。删掉 fastfetch 的 `width`/`height` 后,抓到的字节仍是

```text
\x1b_Ga=T,f=100,t=f,c=29,r=25;...\x1b\
```

与手写宽高**逐字节相同**,对错位零影响;真让它自己按像素算则是 `1250÷20=62` 行,窗口根本放不下。

代价:字号调到 13 以下会复发(阈值掉到 460 以下),那时重跑 `make-logo.py` 换更小的行高即可。


---

## 9. 复现脚本(在 `tools/diagnostics/`,已入库)

| 脚本 | 用途 |
|---|---|
| `tools/diagnostics/bugprobe.py` | 四组对照(A 图 / B 无图 / C 仅 oh-my-posh / D 真实组合),2.5s 自动敲 `abc`,3.2s 导帧 |
| `tools/diagnostics/bugprobe_width.py` | 三档窗口宽度,证明与宽度无关 |
| `tools/diagnostics/bugprobe_height.py` | 三档图片高度(10 / 25 / 40 行) |
| `tools/diagnostics/bugprobe_log.py` | `session_log` 抓 PTY 原始字节 → `target/pty_img.bin` / `pty_off.bin` |
| `tools/diagnostics/bugprobe_cursorpos.py` | 让 ConPTY 自报 `CursorTop`,对比 29 vs 34 |
| `tools/diagnostics/rowproj.py` | 文字行像素投影,量行距与错位像素数 |

导出的帧:`target/bug_A_ff_img.bmp`(错位)、`target/bug_B_ff_noimg.bmp`(正常)、`target/bug_w1700.bmp`(加宽仍错位)。
