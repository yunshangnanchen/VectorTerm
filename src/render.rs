//! 渲染:ab_glyph 字形栅格化 + softbuffer 像素绘制 + 终端状态快照

use crate::config::{
    AppConfig, CursorAnimation, CursorBlinkMode, CursorBlinkStyle, CursorConfig,
    CursorShapeOverride, ImageFill, NerdFontMode,
};
use crate::cursor::{CellRect, Point2, TrailDraw};
use crate::font::{self, FaceInfo, FamilyFaces, FontIndex};
use crate::frame::{Frame, Highlight, Item};
use ab_glyph::{Font, FontArc, FontVec, Glyph, PxScale, ScaleFont};
use anyhow::Context;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use termwiz::image::ImageData;
use termwiz::surface::line::CellRef;
use termwiz::surface::{CursorShape, CursorVisibility};
use wezterm_term::color::{ColorAttribute, ColorPalette, SrgbaTuple};
use wezterm_term::{Blink, Intensity, Terminal, Underline};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor_fg: Rgb,
    pub cursor_bg: Rgb,
}

#[derive(Clone)]
pub struct SnapCell {
    pub text: String,
    /// 该字形占的列数:1 = 半角,2 = 全角(CJK/emoji),0 = 全角字符的续格。
    /// 绘制时按 span * cell_w 把字形横向铺满,避免"汉字之间像有空格"。
    pub span: u8,
    pub fg: Rgb,
    pub bg: Rgb,
    /// SGR 4 / 4:2 / 4:3(花)/ 4:4(点)/ 4:5(虚线)
    pub underline: Underline,
    pub underline_color: Rgb,
    pub strike: bool,
    /// SGR 7 附加的斜体(无专用字形时靠合成倾斜)
    pub italic: bool,
    /// SGR 1 粗体(有 Bold face 时用粗体字形,否则合成加粗)
    pub bold: bool,
    /// SGR 5 / 6 闪烁
    pub blink: bool,
    /// SGR 53 上划线
    pub overline: bool,
    pub invisible: bool,
    /// 挂在这一格上的图片(iTerm2 / Kitty / sixel)。
    /// 一张图横跨多格,每格带自己的纹理坐标,渲染时据此反推整图矩形。
    pub images: Vec<SnapImage>,
}

/// 单元格上的一层图像。`z` 是 Kitty 的 z_index:< 0 画在文字下面,>= 0 画在上面。
#[derive(Clone)]
pub struct SnapImage {
    /// `ImageData` 的内容哈希,解码缓存与 GPU 纹理都按它索引
    pub hash: [u8; 32],
    pub data: Arc<ImageData>,
    /// 本格左上角在图里的纹理坐标(0..1)
    pub tl: (f32, f32),
    /// 本格右下角在图里的纹理坐标(0..1)
    pub br: (f32, f32),
    pub z: i32,
    /// (left, top, right, bottom) 像素;右侧/底部是"本格没被图覆盖的部分"
    pub pad: (u16, u16, u16, u16),
}

pub struct ScreenSnapshot {
    pub rows: Vec<Vec<SnapCell>>,
    /// 每行是否因折行而接续下一行(文本复制时这两行之间不插 \n)
    pub wrapped: Vec<bool>,
    /// 屏上是否存在闪烁单元格:有则驱动定时重绘
    pub has_blink: bool,
    /// 亚行滚动偏移(0..1 行):内容整体上移这么多,用于平滑滚动
    pub scroll_frac: f32,
    /// 终端自己假定的格子像素尺寸(来自 `TerminalSize`)。图片落格时 wezterm
    /// 用"终端格子"算 padding,而渲染器按字体度量出来的格子可能不是同一个值,
    /// 所以要把 padding 换算到渲染坐标系里。未知时为 (0, 0)。
    pub term_cell_px: (f64, f64),
}

pub struct CursorInfo {
    pub col: usize,
    pub row: usize,
    pub shape: CursorShape,
    pub visible: bool,
}

/// 光标本体的视觉样式,由 `cursor.*` 配置派生。
///
/// 挂在 `Renderer` 上而不是塞进 `build_frame` 的参数里:那个入口被主循环、
/// benchmark、GPU 一致性测试和选区测试共用,加一长串参数要把二十多个调用点
/// 全改一遍,而且默认值散落各处容易漂移。
#[derive(Debug, Clone)]
pub struct CursorVisual {
    pub shape: CursorShapeOverride,
    pub width: f32,
    pub height: f32,
    pub align_x: Option<f32>,
    pub align_y: Option<f32>,
    pub thickness: f32,
    pub alpha: f32,
    pub blink: CursorBlinkMode,
    pub blink_duty: f32,
    pub blink_style: CursorBlinkStyle,
    pub anim: CursorAnimation,
    pub anim_ms: f32,
    pub anim_amount: f32,
}

impl From<&CursorConfig> for CursorVisual {
    fn from(c: &CursorConfig) -> Self {
        Self {
            shape: c.shape,
            width: c.width,
            height: c.height,
            align_x: c.align_x,
            align_y: c.align_y,
            thickness: c.thickness,
            alpha: c.alpha,
            blink: c.blink,
            blink_duty: c.blink_duty,
            blink_style: c.blink_style,
            anim: c.anim,
            anim_ms: c.anim_ms,
            anim_amount: c.anim_amount,
        }
    }
}

pub fn rgb(t: SrgbaTuple) -> Rgb {
    let (r, g, b, _) = t.to_srgb_u8();
    Rgb { r, g, b }
}

fn lerp_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb {
        r: m(a.r, b.r),
        g: m(a.g, b.g),
        b: m(a.b, b.b),
    }
}

/// 按配置准备背景层。失败只打印原因并跳过该层,不影响终端启动。
fn load_background(cfg: &AppConfig) -> Vec<crate::frame::BgLayer> {
    use crate::frame::BgLayer;
    use crate::images::DecodedImage;

    // 什么都没配:纯不透明底色,不用挂背景层
    if cfg.background.is_plain() {
        return Vec::new();
    }

    // 整窗不透明度由窗口层(层叠窗口统一 alpha)完成,背景层保持不透明,
    // 否则壁纸会被衰减两次、露出下面的纯色。
    let alpha = 1.0f32;
    let mut layers = Vec::new();

    // 渐变在下:烤成 1×256 / 256×1 的小图,由 stretch 填充铺满整窗,
    // 这样两条后端都只需实现"画一张图"。
    if let Some(g) = &cfg.background.gradient {
        let start = crate::term::color_from_str(&g.start);
        let end = crate::term::color_from_str(&g.end);
        match (start, end) {
            (Some(a), Some(b)) => {
                let (w, h) = if g.vertical { (1u32, 256u32) } else { (256, 1) };
                let n = (w * h) as usize;
                let mut px = Vec::with_capacity(n * 4);
                for i in 0..n {
                    let t = i as f32 / (n - 1).max(1) as f32;
                    let c = lerp_rgb(a, b, t);
                    px.extend_from_slice(&[c.r, c.g, c.b, 255]);
                }
                layers.push(BgLayer {
                    image: Arc::new(DecodedImage {
                        width: w,
                        height: h,
                        pixels: Arc::new(px),
                    }),
                    fill: ImageFill::Stretch,
                    alpha,
                });
            }
            _ => crate::wlog!(
                "背景:渐变颜色无法解析({:?} → {:?}),已跳过渐变",
                g.start,
                g.end
            ),
        }
    }

    // 壁纸在上:半透明时能透出下面的渐变
    if let Some(path) = &cfg.background.image {
        match crate::background::load(path, &cfg.background) {
            Ok(img) => {
                let blur = if cfg.background.blur > 0.0 {
                    format!(" 模糊 {:.0}px", cfg.background.blur)
                } else {
                    String::new()
                };
                crate::wlog!(
                    "背景:{} {}×{} / {} 填充{} / 不透明度 {:.2}",
                    path.display(),
                    img.width,
                    img.height,
                    cfg.background.fill.as_str(),
                    blur,
                    alpha
                );
                layers.push(BgLayer {
                    image: img,
                    fill: cfg.background.fill,
                    alpha,
                });
            }
            Err(e) => crate::wlog!("背景:加载图片失败({e}),保持纯色"),
        }
    }

    layers
}

/// 锁内快速快照:把 wezterm-term 的可见屏幕转换为渲染结构。
///
/// `scroll` 是相对屏幕底部的回看行数,可以是小数(平滑滚动时的亚行位置):
/// 整数部分决定取哪几行,小数部分由渲染阶段整体上移像素实现。
///
/// 光标默认"只要没在回看就显示";视口整体被抬起时(参见
/// `view_geom_of`)调用方要显式给出 `show_cursor`,因为此时
/// `scroll > 0` 但用户并没有回看。
pub fn snapshot(
    term: &Terminal,
    cols: usize,
    scroll: f32,
) -> (ScreenSnapshot, CursorInfo, Palette) {
    snapshot_at(term, cols, scroll, scroll <= 0.001)
}

/// 与 [`snapshot`] 相同,但可以单独指定是否画光标本体。
pub fn snapshot_at(
    term: &Terminal,
    cols: usize,
    scroll: f32,
    show_cursor: bool,
) -> (ScreenSnapshot, CursorInfo, Palette) {
    let pal = term.palette();
    let palette = Palette {
        fg: rgb(pal.foreground),
        bg: rgb(pal.background),
        cursor_fg: rgb(pal.cursor_fg),
        cursor_bg: rgb(pal.cursor_bg),
    };

    let screen = term.screen();
    let rows = screen.physical_rows.max(1);
    let cols = cols.max(1).min(screen.physical_cols.max(1));
    let sb = screen.scrollback_rows();
    let scroll = scroll.max(0.0).min(sb.saturating_sub(rows) as f32);
    // 视口上边界取 ceil:比如 3.2 行 → 取偏移 4 的视图,再整体上移 0.8 行,
    // 于是更早的一行正好从顶部滑进来(方向与"往回翻"一致)。
    let base = scroll.ceil() as usize;
    let frac = base as f32 - scroll;
    let top = sb.saturating_sub(rows + base);
    // frac > 0 时多取一行填住底部空出来的那部分
    let want = (rows + usize::from(frac > 0.002)).min(sb - top);

    let blank = SnapCell {
        text: String::new(),
        span: 1,
        fg: palette.fg,
        bg: palette.bg,
        underline: Underline::None,
        underline_color: palette.fg,
        strike: false,
        italic: false,
        bold: false,
        blink: false,
        overline: false,
        invisible: false,
        images: Vec::new(),
    };

    let mut has_blink = false;
    let mut out_rows: Vec<Vec<SnapCell>> = Vec::with_capacity(want);
    let mut out_wrapped: Vec<bool> = Vec::with_capacity(want);
    screen.with_phys_lines(top..(top + want), |lines| {
        for line in lines {
            out_wrapped.push(line.last_cell_was_wrapped());
            let mut row: Vec<SnapCell> = Vec::with_capacity(cols);
            for cellref in line.visible_cells() {
                if row.len() >= cols {
                    break;
                }
                // 宽字符占多列:首列携带字形,后续列用同样的属性空白占位
                let w = cellref.width().max(1);
                let mut snap = convert_cell(cellref, &pal);
                // 全角字符记录自己跨了几列,渲染时按此宽度横向铺满单元格
                snap.span = w as u8;
                has_blink |= snap.blink;
                // 续格(span = 0)不画字形,也不重复挂图:图片是按格存的,
                // 续格复制一份会让同一张图在同一位置被收集两次
                let cont = SnapCell {
                    span: 0,
                    text: String::new(),
                    images: Vec::new(),
                    ..snap.clone()
                };
                row.push(snap);
                for _ in 1..w {
                    row.push(cont.clone());
                }
            }
            row.truncate(cols);
            while row.len() < cols {
                row.push(blank.clone());
            }
            out_rows.push(row);
        }
    });
    while out_rows.len() < rows {
        out_rows.push(vec![blank.clone(); cols]);
        out_wrapped.push(false);
    }

    let cp = term.cursor_pos();
    // 视口整体抬起 `base` 行时,光标在画面上也跟着下移同样多行
    let cursor_row = (cp.y.max(0) as usize).saturating_add(base);
    // 回看历史时把光标本体藏起来(它指向的其实是屏幕底部的行)
    let cursor = CursorInfo {
        col: cp.x.min(cols - 1),
        row: cursor_row.min(rows.saturating_sub(1)),
        shape: cp.shape,
        visible: cp.visibility == CursorVisibility::Visible && show_cursor && cursor_row < rows,
    };

    // 终端假定的格子像素尺寸:图片 padding 就是在这一坐标系里算的
    let tsize = term.get_size();
    let term_cell_px = if tsize.cols > 0 && tsize.rows > 0 {
        (
            tsize.pixel_width as f64 / tsize.cols as f64,
            tsize.pixel_height as f64 / tsize.rows as f64,
        )
    } else {
        (0.0, 0.0)
    };

    (
        ScreenSnapshot {
            rows: out_rows,
            wrapped: out_wrapped,
            has_blink,
            scroll_frac: frac,
            term_cell_px,
        },
        cursor,
        palette,
    )
}

fn convert_cell(c: CellRef<'_>, pal: &ColorPalette) -> SnapCell {
    let a = c.attrs();
    let fg_attr = a.foreground();
    let mut fg = pal.resolve_fg(fg_attr);
    let mut bg = pal.resolve_bg(a.background());

    // 粗体对 0-7 号色增亮到 8-15(常见终端行为)
    if a.intensity() == Intensity::Bold {
        if let ColorAttribute::PaletteIndex(i) = fg_attr {
            if i < 8 {
                fg = pal.resolve_fg(ColorAttribute::PaletteIndex(i + 8));
            }
        }
    }
    // 暗淡:向背景混合
    if a.intensity() == Intensity::Half {
        fg = fg.interpolate(bg, 0.45);
    }
    if a.reverse() {
        std::mem::swap(&mut fg, &mut bg);
    }

    let underline = a.underline();
    let underline_color = match a.underline_color() {
        ColorAttribute::Default => fg,
        c => pal.resolve_fg(c),
    };
    let hyperlink = a.hyperlink().is_some();

    // 空白格不分配字符串(每帧上千个格子,避免无谓分配)
    let raw = c.str();
    let text = if raw == " " {
        String::new()
    } else {
        raw.to_string()
    };

    // 图像附件:iTerm2 用 set_image 覆盖本格,Kitty 用 attach_image 叠多层(按 z 排序)
    let images = match a.images() {
        Some(list) => list
            .into_iter()
            .map(|im| {
                let (pl, pt, pr, pb) = im.padding();
                SnapImage {
                    hash: im.image_data().hash(),
                    data: im.image_data().clone(),
                    tl: (im.top_left().x.into_inner(), im.top_left().y.into_inner()),
                    br: (
                        im.bottom_right().x.into_inner(),
                        im.bottom_right().y.into_inner(),
                    ),
                    z: im.z_index(),
                    pad: (pl, pt, pr, pb),
                }
            })
            .collect(),
        None => Vec::new(),
    };

    SnapCell {
        text,
        span: 1,
        fg: rgb(fg),
        bg: rgb(bg),
        // OSC 8 超链接默认带下划线,方便识别可点击区域
        underline: if underline == Underline::None && hyperlink {
            Underline::Single
        } else {
            underline
        },
        underline_color: rgb(underline_color),
        strike: a.strikethrough(),
        italic: a.italic(),
        bold: a.intensity() == Intensity::Bold,
        blink: a.blink() != Blink::None,
        overline: a.overline(),
        invisible: a.invisible(),
        images,
    }
}

// ---------------------------------------------------------------------------
// 字体:家族 -> 四个样式槽(Regular / Bold / Italic / BoldItalic)
//
// 对标 wezterm 的做法:字体按"家族 + 字重 + 斜体"选出具体 face,缺字时沿
// 回退链找下一个字体,而不是让主字体硬撑。wezterm 的 cell 宽高来自字体自身
// 的标称尺寸,字形只在**超出格子**时缩小,不会为了填满格子而拉伸 —— 这样
// 汉字和 Nerd Font 图标都能保持原始比例。
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
struct StyleBits {
    bold: bool,
    italic: bool,
}

impl StyleBits {
    fn bit(self) -> u8 {
        (self.bold as u8) | ((self.italic as u8) << 1)
    }
}

struct CachedGlyph {
    w: u32,
    h: u32,
    left: i32,
    top: i32,
    /// 该字形在 ox 处的自然步进(像素)
    advance: f32,
    /// 来源字体在该尺寸下的升部(基线以上高度)
    ascent: f32,
    /// 来源字体在该尺寸下的降部(通常为负)
    descent: f32,
    /// 家族没有 Bold face 时用"再描一遍"的方式合成粗体
    synthetic_bold: bool,
    data: Vec<u8>,
}

/// 一个彩色 emoji 在当前字号下的绘制信息。位图本体放在 `images` 缓存里,
/// 这里只留 key 和几何。
#[derive(Clone, Copy)]
struct EmojiCell {
    key: [u8; 32],
    w: f64,
    h: f64,
    /// 位图顶边到基线的距离
    bearing_y: f64,
}

fn emoji_key(ch: char, em_bits: u32) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[0..5].copy_from_slice(b"EMOJI");
    k[5..9].copy_from_slice(&(ch as u32).to_le_bytes());
    k[9..13].copy_from_slice(&em_bits.to_le_bytes());
    k
}

/// 字形在网格里的排版角色
#[derive(Clone, Copy, PartialEq, Eq)]
enum GlyphRole {
    /// Powerline 分隔符(U+E0B0..U+E0B7):纯几何色块,整体压进格子。
    Separator,
    /// 制表符 / 方块元素:按 em 盒映射到格子再裁掉溢出,保持线宽比例。
    BoxGlyph,
    /// 私有区图标(Nerd Font):等比塞进格子并居中。
    Icon,
    /// 普通文字与 CJK:保持自然大小,只在超格时缩小。
    Text,
}

fn glyph_role(ch: char) -> GlyphRole {
    match ch as u32 {
        // Powerline 分隔符(实心/空心三角、半圆):水平方向要与相邻格拼死
        0xE0B0..=0xE0B7 => GlyphRole::Separator,
        // 制表符(细/粗/双线/圆角/虚线)
        0x2500..=0x257F => GlyphRole::BoxGlyph,
        // 网点阴影(░▒▓)。它们是图案不是实心块,不能当矩形铺满,
        // 仍按字形画;▀▄█▌▐ 这些实心块走 `block_rects` 的几何路径。
        0x2591..=0x2593 => GlyphRole::BoxGlyph,
        _ if font::is_private_use(ch) => GlyphRole::Icon,
        _ => GlyphRole::Text,
    }
}

/// 状态栏图标的目标高度 = 数字字高 × 这个倍数。略高于数字,看着不憋屈。
const ICON_RATIO: f64 = 1.10;
/// 墨迹高过数字的这个倍数,就判定它"画满了 em 框",重新栅格化小一档。
const ICON_OVERSIZE: f64 = 1.12;

/// 块元素在格子里的填充区域(比例坐标,原点在格子左上角)。
type BlockRect = (f64, f64, f64, f64);

/// 实心块元素(▀ ▄ █ ▌ ▐ 及象限块)的几何。
///
/// 返回 `None` 表示不是实心块 —— 目前只有 U+2591~U+2593 那三档网点阴影,
/// 它们本来就是图案,继续走字形。
///
/// **为什么不画字形**:字体把这些块画在 em 盒里,而 em 盒(ascent + descent,
/// Meslo 是 1.36em)和终端格子(被 `line_height` 压到 0.886em)根本不是
/// 一回事。按 em 盒等比映射过去,`█` 的上下各留一条 1/4 格的缝,`▄` 也
/// 凑不出正好半格。半块字符画(DSH、chafa、timg 之类)全靠相邻两行的
/// `▀` 和 `▄` 拼成一整块,只要有一条缝,整幅画就变成横条纹。
///
/// 直接填矩形还有个好处:边界统一取整到像素(见 [`Renderer::draw_text`]),
/// `▀` 的底边和下一行 `▄` 的顶边算出来是同一个 y,拼接口不可能错位。
fn block_rects(ch: char) -> Option<&'static [BlockRect]> {
    let r: &'static [BlockRect] = match ch {
        // 上半块 + 下半侧按 1/8 步进的横条
        '\u{2580}' => &[(0.0, 0.0, 1.0, 0.5)], // ▀ upper half
        '\u{2581}' => &[(0.0, 0.875, 1.0, 1.0)], // ▁ lower one eighth
        '\u{2582}' => &[(0.0, 0.75, 1.0, 1.0)], // ▂ lower one quarter
        '\u{2583}' => &[(0.0, 0.625, 1.0, 1.0)], // ▃ lower three eighths
        '\u{2584}' => &[(0.0, 0.5, 1.0, 1.0)], // ▄ lower half
        '\u{2585}' => &[(0.0, 0.375, 1.0, 1.0)], // ▅ lower five eighths
        '\u{2586}' => &[(0.0, 0.25, 1.0, 1.0)], // ▆ lower three quarters
        '\u{2587}' => &[(0.0, 0.125, 1.0, 1.0)], // ▇ lower seven eighths
        '\u{2588}' => &[(0.0, 0.0, 1.0, 1.0)], // █ full block
        // 左半侧按 1/8 步进的竖条
        '\u{2589}' => &[(0.0, 0.0, 0.875, 1.0)], // ▉ left seven eighths
        '\u{258a}' => &[(0.0, 0.0, 0.75, 1.0)],  // ▊ left three quarters
        '\u{258b}' => &[(0.0, 0.0, 0.625, 1.0)], // ▋ left five eighths
        '\u{258c}' => &[(0.0, 0.0, 0.5, 1.0)],   // ▌ left half
        '\u{258d}' => &[(0.0, 0.0, 0.375, 1.0)], // ▍ left three eighths
        '\u{258e}' => &[(0.0, 0.0, 0.25, 1.0)],  // ▎ left one quarter
        '\u{258f}' => &[(0.0, 0.0, 0.125, 1.0)], // ▏ left one eighth
        '\u{2590}' => &[(0.5, 0.0, 1.0, 1.0)],   // ▐ right half
        // 2591/2592/2593 是 25%/50%/75% 网点,不是实心块
        '\u{2594}' => &[(0.0, 0.0, 1.0, 0.125)], // ▔ upper one eighth
        '\u{2595}' => &[(0.875, 0.0, 1.0, 1.0)], // ▕ right one eighth
        // 象限块:一个字符 = 2x2 里的一到三个小方块
        '\u{2596}' => &[(0.0, 0.5, 0.5, 1.0)], // ▖ lower left
        '\u{2597}' => &[(0.5, 0.5, 1.0, 1.0)], // ▗ lower right
        '\u{2598}' => &[(0.0, 0.0, 0.5, 0.5)], // ▘ upper left
        '\u{2599}' => &[
            // ▙ upper left + lower left + lower right
            (0.0, 0.0, 0.5, 0.5),
            (0.0, 0.5, 1.0, 1.0),
        ],
        '\u{259a}' => &[
            // ▚ upper left + lower right
            (0.0, 0.0, 0.5, 0.5),
            (0.5, 0.5, 1.0, 1.0),
        ],
        '\u{259b}' => &[
            // ▛ upper left + upper right + lower left
            (0.0, 0.0, 1.0, 0.5),
            (0.0, 0.5, 0.5, 1.0),
        ],
        '\u{259c}' => &[
            // ▜ upper left + upper right + lower right
            (0.0, 0.0, 1.0, 0.5),
            (0.5, 0.5, 1.0, 1.0),
        ],
        '\u{259d}' => &[(0.5, 0.0, 1.0, 0.5)], // ▝ upper right
        '\u{259e}' => &[
            // ▞ upper right + lower left
            (0.5, 0.0, 1.0, 0.5),
            (0.0, 0.5, 0.5, 1.0),
        ],
        '\u{259f}' => &[
            // ▟ upper right + lower right + lower left
            (0.5, 0.0, 1.0, 0.5),
            (0.0, 0.5, 1.0, 1.0),
        ],
        _ => return None,
    };
    Some(r)
}

/// 一个字体及其 PxScale。
///
/// 注意 ab_glyph 的 `PxScale` **不是 em 大小**,而是"ascender 到 descender 的
/// 像素高度"(`h_scale_factor = scale / height_unscaled`)。
/// Consolas 的 asc-desc 刚好 ≈ 1em,所以字号直接传进去看着正常;
/// 微软雅黑的 asc-desc 是 1.32em,若照搬字号,汉字只按 0.76em 渲染 ——
/// 每个字在自己的两格里留出一道缝,看起来就像汉字之间插了空格。
/// 因此这里统一换算成"em = em_px"的等价 PxScale。
struct FontSlot {
    font: FontArc,
    scale: PxScale,
}

impl FontSlot {
    /// 同一个 face 在另一个像素尺寸下的 scale。
    ///
    /// 状态栏有自己的字号(`status_bar.font_scale`)时,wezterm 的做法是**按目标
    /// 尺寸重新栅格化字形**,而不是把终端字号的位图拉伸 —— 拉伸会让笔画发糊、
    /// 颜色变淡(抗锯齿边缘被插值糊开)。这里给出"按比例换算后的 PxScale",
    /// 让 `rasterize_in` 能按真实尺寸出位图。
    fn at(&self, factor: f32) -> PxScale {
        if (factor - 1.0).abs() < 0.001 {
            return self.scale;
        }
        PxScale {
            x: self.scale.x * factor,
            y: self.scale.y * factor,
        }
    }

    fn new(font: FontArc, em_px: f32) -> Self {
        let h = font.height_unscaled();
        let upem = font.units_per_em().unwrap_or(0.0);
        let scale = if h > 0.0 && upem > 0.0 {
            PxScale::from(em_px * h / upem)
        } else {
            PxScale::from(em_px)
        };
        Self { font, scale }
    }

    fn from_face(face: &FaceInfo, em_px: f32) -> anyhow::Result<Self> {
        // 内嵌字体没有磁盘文件,字节就在 exe 里;其余仍按路径读
        let data = match crate::builtin_fonts::bytes(&face.path) {
            Some(b) => b.to_vec(),
            None => std::fs::read(&face.path)
                .with_context(|| format!("读取字体 {}", face.path.display()))?,
        };
        let font = FontVec::try_from_vec_and_index(data, face.index)
            .with_context(|| format!("解析字体 {}#{}", face.path.display(), face.index))?;
        Ok(Self::new(FontArc::new(font), em_px))
    }
}

struct FamilySlot {
    name: String,
    regular: FontSlot,
    bold: Option<FontSlot>,
    italic: Option<FontSlot>,
    bold_italic: Option<FontSlot>,
}

impl FamilySlot {
    /// 按请求的样式挑最合适的 face;没有对应 face 时回落到 Regular,
    /// 并告诉调用方要不要合成粗体。
    fn pick(&self, style: StyleBits) -> (&FontSlot, bool) {
        let found = if style.bold && style.italic {
            self.bold_italic
                .as_ref()
                .or(self.bold.as_ref())
                .or(self.italic.as_ref())
        } else if style.bold {
            self.bold.as_ref()
        } else if style.italic {
            self.italic.as_ref()
        } else {
            None
        };
        match found {
            Some(slot) => (slot, false),
            None => (&self.regular, style.bold),
        }
    }
}

fn index_for(dirs: &[PathBuf], cache: &mut Option<Arc<FontIndex>>) -> Arc<FontIndex> {
    cache
        .get_or_insert_with(|| {
            if dirs.is_empty() {
                FontIndex::system_cached()
            } else {
                Arc::new(FontIndex::load(dirs))
            }
        })
        .clone()
}

/// 解析一个字体 spec(路径或家族名)成四个样式槽
fn resolve_family(
    spec: &str,
    dirs: &[PathBuf],
    index: &mut Option<Arc<FontIndex>>,
) -> Option<FamilyFaces> {
    let path = Path::new(spec);
    if path.is_file() {
        return FontIndex::resolve_file(path);
    }
    index_for(dirs, index).resolve(spec)
}

/// 解析单个样式覆盖(如 `bold = "Consolas Bold"`),直接拿匹配到的那一个 face
fn resolve_face(
    spec: &str,
    dirs: &[PathBuf],
    index: &mut Option<Arc<FontIndex>>,
) -> Option<FaceInfo> {
    let path = Path::new(spec);
    if path.is_file() {
        return FontIndex::face_from_file(path);
    }
    index_for(dirs, index).face_from_spec(spec)
}

fn load_family(
    faces: &FamilyFaces,
    em_px: f32,
    load_styles: bool,
    bold: Option<FaceInfo>,
    italic: Option<FaceInfo>,
    bold_italic: Option<FaceInfo>,
) -> anyhow::Result<FamilySlot> {
    let load = |f: &FaceInfo| FontSlot::from_face(f, em_px);
    // 回退字体/图标字体只用 Regular,避免把同家族的 19MB TTC 读四遍
    if !load_styles {
        return Ok(FamilySlot {
            name: faces.name.clone(),
            regular: load(&faces.regular)?,
            bold: None,
            italic: None,
            bold_italic: None,
        });
    }
    Ok(FamilySlot {
        name: faces.name.clone(),
        regular: load(&faces.regular)?,
        bold: match bold {
            Some(f) => Some(load(&f)?),
            None => faces.bold.as_ref().map(load).transpose()?,
        },
        italic: match italic {
            Some(f) => Some(load(&f)?),
            None => faces.italic.as_ref().map(load).transpose()?,
        },
        bold_italic: match bold_italic {
            Some(f) => Some(load(&f)?),
            None => faces.bold_italic.as_ref().map(load).transpose()?,
        },
    })
}

/// 标签栏里的一项(标题与状态由 main 组装)
#[derive(Clone, Default)]
pub struct TabBarItem {
    pub title: String,
    pub active: bool,
    pub hovered: bool,
    /// 鼠标是否停在这一项的关闭按钮上
    pub close_hovered: bool,
    /// `format-tab-title` 给的带色文字。None = 用内置的"序号: 标题"
    pub spans: Option<Vec<crate::status::Span>>,
    /// `format-tab-title` 给的标签底色 / 前景色,覆盖 `colors.tab_bar.active|inactive`
    pub bg: Option<Rgb>,
    pub fg: Option<Rgb>,
}

/// 标签栏绘制参数。几何来自 `tabbar::layout`,这里只负责画。
pub struct TabBarDraw<'a> {
    pub items: &'a [TabBarItem],
    pub layout: &'a crate::tabbar::Layout,
    pub colors: &'a crate::config::TabBarColors,
    pub fancy: bool,
    pub show_index: bool,
    pub at_bottom: bool,
    /// 圆角半径(像素);None = 按行高自动
    pub corner_radius: Option<f64>,
    /// 鼠标是否停在"新建标签"按钮上
    pub new_tab_hovered: bool,
    /// 鼠标停在哪个窗口按钮上(只在 INTEGRATED_BUTTONS 下有按钮)
    pub window_hovered: Option<crate::tabbar::WindowButton>,
    /// `integrated_title_button_color`:None = "Auto"(跟随标签栏前景色)
    pub button_color: Option<Rgb>,
    /// `integrated_title_button_style = "Gnome"`:圆形按钮底
    pub button_gnome_style: bool,
    /// 窗口当前是否最大化:最大化按钮要画成"还原"双框
    pub maximized: bool,
    /// 左侧状态栏(`window:set_left_status`),已解析成带颜色的片段
    pub left_status: &'a [crate::status::Span],
    /// 右侧状态栏(`window:set_right_status` / `status_bar` 合成的文字)
    pub right_status: &'a [crate::status::Span],
    /// 右侧状态栏的字号倍数(对应 `status_bar.font_scale`,1.0 = 和终端一样大)
    pub status_scale: f64,
    /// 右侧状态栏的左右留白,物理像素(对应 `status_bar.padding`)
    pub status_pad: f64,
    /// 右侧状态栏相邻片段之间的额外间距,物理像素(对应 `status_bar.item_spacing`)
    pub status_gap: f64,
    /// 右侧状态栏里一个空格占几个字宽(对应 `status_bar.space_width`)
    pub status_space: f64,
}

/// 按**显示宽度**折行。等价于 wezterm 里那句 `textwrap::fill(message, width)`:
/// 优先在空格处断,单个词比整行还宽时按格硬切。
fn wrap_cells(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut used = 0usize;

    for word in text.split(' ') {
        let mut word = word;
        loop {
            let w = crate::tabbar::text_cells(word);
            let sep = usize::from(used > 0);
            if used + sep + w <= width {
                if sep == 1 {
                    line.push(' ');
                    used += 1;
                }
                line.push_str(word);
                used += w;
                break;
            }
            if used > 0 {
                // 当前行放不下,换行后重排这个词
                out.push(std::mem::take(&mut line));
                used = 0;
                continue;
            }
            // 单词自身超宽:切下能放下的前缀,剩下的一行行消化
            let mut cut = 0usize;
            let mut cw = 0usize;
            for (i, ch) in word.char_indices() {
                let w = crate::tabbar::char_width(ch);
                if cw + w > width {
                    break;
                }
                cw += w;
                cut = i + ch.len_utf8();
            }
            if cut == 0 {
                cut = word.len();
            }
            line.push_str(&word[..cut]);
            out.push(std::mem::take(&mut line));
            word = &word[cut..];
            used = 0;
            if word.is_empty() {
                break;
            }
        }
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

/// 关闭确认的版式。行列用"格"计量 —— wezterm 的确认框是 termwiz overlay,
/// 排版全在等宽网格上做,像素版式是它的直接翻版:
///
/// * 正文宽度取窗口列数的 80%,左边界落在 10% 的列上
/// * 正文按宽度折行,整块(正文 + 空一行 + 按钮行)垂直居中
/// * 按钮行紧跟正文,` [Y]es ` 占 7 格、空 8 格、` [N]o ` 占 6 格
pub struct ConfirmLayout {
    /// 被清成背景色的内容区上下沿(像素)
    pub top: f64,
    pub bottom: f64,
    pub cell_w: f64,
    pub cell_h: f64,
    /// 折好的正文(每行已 trim_end)
    pub lines: Vec<String>,
    /// 正文左起的列
    pub x_col: usize,
    /// 正文第一行、按钮行各自的行号(相对内容区顶)
    pub top_row: usize,
    pub button_row: usize,
    /// 两个选项的起始列与宽度(格)
    pub choice_col: [usize; 2],
    pub choice_w: [usize; 2],
}

impl ConfirmLayout {
    pub fn col_of(&self, x: f64) -> usize {
        (x / self.cell_w.max(1.0)).floor().max(0.0) as usize
    }

    pub fn row_of(&self, y: f64) -> usize {
        ((y - self.top) / self.cell_h.max(1.0)).floor().max(0.0) as usize
    }

    pub fn col_x(&self, col: usize) -> f64 {
        col as f64 * self.cell_w
    }

    pub fn row_y(&self, row: usize) -> f64 {
        self.top + row as f64 * self.cell_h
    }
}

/// 确认界面要画的内容
pub struct DialogDraw<'a> {
    /// 提示语(wezterm 是单行带 🛑 的那句)
    pub message: &'a str,
    /// 两个选项的文案,下标 0 = `[Y]es`、1 = `[N]o`
    pub choices: [&'a str; 2],
    /// 反显的是哪一项。wezterm 初始是 `None`:没有焦点、没有默认按钮
    pub active: Option<usize>,
    pub palette: &'a Palette,
}

pub struct Renderer {
    /// [0] 主字体,其余是用户配置的回退链
    families: Vec<FamilySlot>,
    /// 画 PUA 图标用的 Nerd Font(优先级高于普通回退链)
    icon_families: Vec<FamilySlot>,
    icon_mode: NerdFontMode,
    icons_loaded: bool,
    index: Option<Arc<FontIndex>>,
    font_dirs: Vec<PathBuf>,
    em_px: f32,
    pub cell_w: f64,
    /// 主字体的自然字距(未取整)。状态栏按它排版,和 wezterm 的 shaper 一致。
    pub advance_w: f64,
    pub cell_h: f64,
    pub ascent: f64,
    pub pad_x: f64,
    pub pad_y: f64,
    /// 标签栏占掉的顶部像素(0 = 无标签栏)
    pub content_top: f64,
    /// 标签栏占掉的底部像素(标签栏在下方时非 0)
    pub content_bottom: f64,
    /// 字形位图表;`cache` 把 (字符, 样式) 映射到这里的下标
    glyphs: Vec<CachedGlyph>,
    cache: HashMap<(char, u8), Option<u32>>,
    /// 非正文字号(状态栏 `font_scale`)的字形缓存:键里带字号百分比,
    /// 同一字符在不同字号下各有一份**按该尺寸真实栅格化**的位图。
    sized_cache: HashMap<(char, u8, u32), Option<u32>>,
    /// 图形协议(iTerm2 / Kitty / sixel)的解码结果,按内容哈希缓存
    pub images: crate::images::ImageCache,
    /// 内嵌的彩色 emoji 字体(CBDT 位图);None = 字体损坏/被裁掉
    emoji: Option<crate::emoji::EmojiFont>,
    /// 某字符在当前字号下有没有彩色位图;含 None,避免每帧反复查 cmap
    emoji_cache: HashMap<(char, u32), Option<EmojiCell>>,
    /// 关掉后连 iTerm2 / sixel 的图也不画(Kitty 侧由终端配置直接不回应)
    images_enabled: bool,
    /// 自定义背景层(渐变在下、壁纸在上),每帧灌进 `Frame::bg`
    bg: Vec<crate::frame::BgLayer>,
    /// 自定义纯色背景(覆盖调色板的 bg)
    bg_color: Option<Rgb>,
    /// 光标本体样式;默认整格方块、不闪、不动,与旧版本逐像素一致
    pub cursor_style: CursorVisual,
    /// 当前闪烁周期相位(0..1),主循环每帧下发
    pub cursor_blink_phase: f32,
    /// 常驻动画的累计时间(秒),主循环每帧下发
    pub cursor_anim_t: f64,
}

impl Renderer {
    pub fn new(cfg: &AppConfig, scale: f64) -> anyhow::Result<Self> {
        let em_px = cfg.fonts.size * scale as f32;
        let dirs = cfg.fonts.dirs.clone();
        let mut index: Option<Arc<FontIndex>> = None;

        // 主字体:路径或家族名
        let primary = match resolve_family(&cfg.fonts.family, &dirs, &mut index) {
            Some(f) => f,
            None => {
                crate::wlog!("字体:找不到 {:?},回退到 Consolas", cfg.fonts.family);
                resolve_family("Consolas", &dirs, &mut index)
                    .or_else(|| FontIndex::resolve_file(Path::new("C:/Windows/Fonts/consola.ttf")))
                    .context("连默认字体 Consolas 都加载失败")?
            }
        };

        let mut pick_face = |spec: &Option<String>, what: &str| -> Option<FaceInfo> {
            let s = spec.as_deref()?;
            let face = resolve_face(s, &dirs, &mut index);
            if face.is_none() {
                crate::wlog!("字体:找不到 {what} 字体 {s:?}");
            }
            face
        };
        let bold = pick_face(&cfg.fonts.bold, "bold");
        let italic = pick_face(&cfg.fonts.italic, "italic");
        let bold_italic = pick_face(&cfg.fonts.bold_italic, "bold_italic");

        let mut families = vec![load_family(
            &primary,
            em_px,
            true,
            bold,
            italic,
            bold_italic,
        )?];

        // 用户配置的文本回退链(按顺序;解析失败的只警告,不阻塞启动)
        for spec in &cfg.fonts.fallback {
            match resolve_family(spec, &dirs, &mut index) {
                Some(f) => match load_family(&f, em_px, false, None, None, None) {
                    Ok(slot) => {
                        if !families
                            .iter()
                            .any(|x| x.name.eq_ignore_ascii_case(&slot.name))
                        {
                            families.push(slot);
                        }
                    }
                    Err(e) => crate::wlog!("字体:回退字体 {spec:?} 加载失败:{e}"),
                },
                None => crate::wlog!("字体:找不到回退字体 {spec:?}"),
            }
        }

        // 图标字体:显式指定时立即加载;auto 时等第一次缺字再扫描(避免启动卡顿)
        let icon_mode = cfg.fonts.nerd_font.clone();
        let mut icon_families = Vec::new();
        let mut icons_loaded = matches!(icon_mode, NerdFontMode::Off);
        if let NerdFontMode::Family(name) = &icon_mode {
            if let Some(faces) = resolve_family(name, &dirs, &mut index) {
                if let Ok(slot) = load_family(&faces, em_px, false, None, None, None) {
                    icon_families.push(slot);
                }
            } else {
                crate::wlog!("字体:找不到 Nerd Font 家族 {name:?}");
            }
            icons_loaded = true;
        }

        // 格宽/行高/基线都以主字体为准(终端是等宽网格)
        let (cell_w, advance_w, cell_h, ascent) = {
            let main = &families[0].regular;
            let sf = main.font.as_scaled(main.scale);
            let advance = sf.h_advance(main.font.glyph_id('M')).max(1.0);
            let natural_h = (sf.ascent() - sf.descent()).max(1.0) as f64;
            // 网格宽度必须取整(不然文字和光标对不齐);但状态栏不是网格,
            // 用未取整的字距排版 —— 这也正是 wezterm 的做法(它按 shaper 的
            // advance 摆字)。取整会让每个字符多出零点几像素,一行几十个字
            // 累积起来就是"0px 间距也显得松"。
            let advance_w = (advance as f64 * cfg.fonts.cell_width).max(0.1);
            let cell_w = advance_w.round().max(1.0);
            let cell_h = match cfg.fonts.line_height {
                Some(m) => (natural_h * m).round().max(1.0),
                // 自动:字体自身行高,但夹在 1.0em~1.6em。
                // Meslo 等 Nerd Font 的 hhea 是 1.55em,会显得行距偏大,
                // 想要更紧凑就在配置里显式写 line_height(例如 0.85)
                None => natural_h
                    .clamp(em_px as f64, em_px as f64 * 1.6)
                    .round()
                    .max(1.0),
            };
            // 行高比字体本身高时,把多出来的空间均分到上下,字形垂直居中
            let leading = cell_h - natural_h;
            (cell_w, advance_w, cell_h, sf.ascent() as f64 + leading / 2.0)
        };

        Ok(Self {
            families,
            icon_families,
            icon_mode,
            icons_loaded,
            index,
            font_dirs: dirs,
            em_px,
            cell_w,
            advance_w,
            cell_h,
            ascent,
            pad_x: cfg.padding * scale,
            pad_y: cfg.padding * scale,
            content_top: 0.0,
            content_bottom: 0.0,
            glyphs: Vec::new(),
            cache: HashMap::new(),
            sized_cache: HashMap::new(),
            images: crate::images::ImageCache::new(cfg.images.max_memory_mb),
            emoji: crate::emoji::EmojiFont::load(crate::builtin_fonts::EMOJI_BYTES),
            emoji_cache: HashMap::new(),
            images_enabled: cfg.images.enabled,
            bg: load_background(cfg),
            bg_color: cfg
                .background
                .color
                .as_deref()
                .and_then(crate::term::color_from_str),
            cursor_style: CursorVisual::from(&cfg.cursor),
            cursor_blink_phase: 0.0,
            cursor_anim_t: 0.0,
        })
    }

    /// 第一次遇到缺失字形时,扫描系统字体目录找 Nerd Font 并加载。
    /// 放在这里而不是 `new()`:大多数会话根本不会打印图标,没必要启动就扫 800MB 字体目录。
    fn ensure_icons(&mut self) {
        if self.icons_loaded {
            return;
        }
        self.icons_loaded = true;
        if matches!(self.icon_mode, NerdFontMode::Off) {
            return;
        }
        let names: Vec<String> = match &self.icon_mode {
            NerdFontMode::Family(name) => vec![name.clone()],
            NerdFontMode::Auto => index_for(&self.font_dirs, &mut self.index).nerd_families(),
            NerdFontMode::Off => return,
        };
        for name in names.into_iter().take(4) {
            if self
                .families
                .iter()
                .any(|f| f.name.eq_ignore_ascii_case(&name))
            {
                continue;
            }
            let Some(faces) = resolve_family(&name, &self.font_dirs, &mut self.index) else {
                continue;
            };
            match load_family(&faces, self.em_px, false, None, None, None) {
                Ok(slot) => self.icon_families.push(slot),
                Err(e) => crate::wlog!("字体:Nerd Font {name:?} 加载失败:{e}"),
            }
        }
    }

    pub fn compute_grid(&self, w: u32, h: u32) -> (usize, usize) {
        let cols = ((((w as f64 - 2.0 * self.pad_x) / self.cell_w).floor() as i64).max(2)) as usize;
        let usable = h as f64 - self.content_top - self.content_bottom - 2.0 * self.pad_y;
        let rows = ((usable / self.cell_h).floor() as i64).max(2) as usize;
        (cols, rows)
    }

    pub fn cell_rect(&self, col: usize, row: usize) -> CellRect {
        CellRect {
            x: self.pad_x + col as f64 * self.cell_w,
            y: self.pad_y + self.content_top + row as f64 * self.cell_h,
            w: self.cell_w,
            h: self.cell_h,
        }
    }

    /// 带额外像素偏移的 `cell_rect`(分屏时 pane 的左上角相对内容区原点偏移)。
    pub fn cell_rect_off(&self, col: usize, row: usize, ox: f64, oy: f64) -> CellRect {
        CellRect {
            x: self.pad_x + ox + col as f64 * self.cell_w,
            y: self.pad_y + self.content_top + oy + row as f64 * self.cell_h,
            w: self.cell_w,
            h: self.cell_h,
        }
    }

    /// 取字形位图的下标(按需栅格化并缓存)
    fn glyph(&mut self, ch: char, style: StyleBits) -> Option<u32> {
        let key = (ch, style.bit());
        if let Some(id) = self.cache.get(&key) {
            return *id;
        }
        let mut g = self.rasterize(ch, style, 1.0);
        // 第一次缺字:可能是 Nerd Font 图标,按需扫描并加载图标字体后重试
        if g.is_none() && !self.icons_loaded {
            self.ensure_icons();
            g = self.rasterize(ch, style, 1.0);
        }
        let id = g.map(|g| {
            self.glyphs.push(g);
            (self.glyphs.len() - 1) as u32
        });
        self.cache.insert(key, id);
        id
    }

    /// 取"按 `factor` 倍字号真实栅格化"的字形下标。
    ///
    /// 状态栏的字号与正文不同时走这里:位图是按目标尺寸重新画的,不是把正文
    /// 位图拉伸。`factor == 1.0` 时直接复用正文缓存,状态栏默认字号就和正文
    /// 像素级一致。
    fn glyph_scaled(&mut self, ch: char, style: StyleBits, factor: f64) -> Option<u32> {
        // 千分位取整当键:0.05 一档的滑杆值不会把缓存打碎
        let fkey = (factor * 1000.0).round().clamp(1.0, 8000.0) as u32;
        if fkey == 1000 {
            return self.glyph(ch, style);
        }
        let key = (ch, style.bit(), fkey);
        if let Some(id) = self.sized_cache.get(&key) {
            return *id;
        }
        let factor = fkey as f32 / 1000.0;
        let mut g = self.rasterize(ch, style, factor);
        if g.is_none() && !self.icons_loaded {
            self.ensure_icons();
            g = self.rasterize(ch, style, factor);
        }
        let id = g.map(|g| {
            self.glyphs.push(g);
            (self.glyphs.len() - 1) as u32
        });
        self.sized_cache.insert(key, id);
        id
    }

    /// 测试用:按字号取字形位图尺寸
    #[cfg(test)]
    fn glyph_size_at(&mut self, ch: char, style: StyleBits, factor: f64) -> Option<(u32, u32)> {
        let id = self.glyph_scaled(ch, style, factor)?;
        self.glyphs.get(id as usize).map(|g| (g.w, g.h))
    }

    /// 测试用:取字形位图的详情
    #[cfg(test)]
    fn glyph_info(&mut self, ch: char, style: StyleBits) -> Option<&CachedGlyph> {
        let id = self.glyph(ch, style)?;
        self.glyphs.get(id as usize)
    }

    /// 供 GPU 后端构建图集:取字形位图与尺寸
    pub fn glyph_bitmap(&self, id: u32) -> Option<(&[u8], u32, u32)> {
        self.glyphs
            .get(id as usize)
            .map(|g| (g.data.as_slice(), g.w, g.h))
    }

    /// 取一个字符的彩色 emoji 位图(第一次见时解码、缩放并塞进图片缓存)。
    /// 返回 None 表示这不是彩色 emoji,照旧走单色字形链。
    fn emoji_cell(&mut self, ch: char) -> Option<EmojiCell> {
        if !crate::emoji::maybe_emoji(ch) {
            return None;
        }
        let bits = self.em_px.to_bits();
        if let Some(hit) = self.emoji_cache.get(&(ch, bits)) {
            // 每帧碰一次,免得 LRU 把正在显示的 emoji 淘汰掉
            if let Some(c) = hit {
                let _ = self.images.get(&c.key);
            }
            return *hit;
        }
        let cell = self.emoji.as_ref().and_then(|f| f.rasterize(ch, self.em_px));
        let out = cell.map(|g| {
            let key = emoji_key(ch, bits);
            let c = EmojiCell {
                key,
                w: g.image.width as f64,
                h: g.image.height as f64,
                bearing_y: g.bearing_y as f64,
            };
            self.images.insert(key, std::sync::Arc::new(g.image));
            c
        });
        self.emoji_cache.insert((ch, bits), out);
        out
    }

    /// 按 `factor` 倍字号栅格化一个字形(1.0 = 终端正文字号)。
    fn rasterize(&self, ch: char, style: StyleBits, factor: f32) -> Option<CachedGlyph> {
        let symbol = font::is_private_use(ch);
        // 主字体永远优先(如果主字体本身就是 Nerd Font,图标直接从这里来)
        if let Some(g) = rasterize_in(&self.families[0], ch, style, factor) {
            return Some(g);
        }
        // PUA 图标:接着找 Nerd Font,而不是先走文本回退链 ——
        // 某些 CJK 字体的私用区里也有字形,会让图标变成乱码方块
        if symbol {
            for family in &self.icon_families {
                if let Some(g) = rasterize_in(family, ch, style, factor) {
                    return Some(g);
                }
            }
        }
        // 文本回退链(CJK、符号等)
        for family in &self.families[1..] {
            if let Some(g) = rasterize_in(family, ch, style, factor) {
                return Some(g);
            }
        }
        // 非 PUA 的符号(如 ⏻ U+23FB、⬢ U+2B22)也允许图标字体兜底
        if !symbol {
            for family in &self.icon_families {
                if let Some(g) = rasterize_in(family, ch, style, factor) {
                    return Some(g);
                }
            }
        }
        None
    }

    /// 生成一帧的绘制指令:CPU 与 GPU 两个后端消费同一份列表。
    #[allow(clippy::too_many_arguments)]
    pub fn build_frame(
        &mut self,
        frame: &mut Frame,
        w: u32,
        h: u32,
        snap: &ScreenSnapshot,
        cursor: &CursorInfo,
        palette: &Palette,
        trail: Option<TrailDraw>,
        trail_color: Rgb,
        blink_on: bool,
    ) {
        self.build_frame_with_selection(
            frame,
            w,
            h,
            snap,
            cursor,
            palette,
            trail,
            trail_color,
            blink_on,
            None,
            true,
        );
    }

    /// 与 `build_frame` 相同,额外在**背景层之后、字形层之前**画半透明选区。
    /// 顺序很关键:高亮在字下面,不会把文字染成一块糊的颜色;CPU/GPU 两路
    /// 都从同一份显示列表取指令,因此切换后端选区位置完全一致。
    ///
    /// `begin` 为 false 时不清空 frame、不重设 clear/bg(分屏时后续 pane
    /// 追加到同一份显示列表),调用方负责在第一个 pane 前 begin。
    #[allow(clippy::too_many_arguments)]
    pub fn build_frame_with_selection(
        &mut self,
        frame: &mut Frame,
        w: u32,
        h: u32,
        snap: &ScreenSnapshot,
        cursor: &CursorInfo,
        palette: &Palette,
        trail: Option<TrailDraw>,
        trail_color: Rgb,
        blink_on: bool,
        highlight: Option<Highlight>,
        begin: bool,
    ) {
        // 内容区原点:标题/标签栏占掉的顶部空间 + 内边距。
        // 分屏时 pad_x/pad_y 已被调用方临时改成包含 pane 偏移。
        let origin_y = self.pad_y + self.content_top;
        // 自定义纯色背景覆盖调色板的 bg
        let clear = self.bg_color.unwrap_or(palette.bg);
        if begin {
            frame.begin(w, h, origin_y as f32, clear);
            // 背景层(渐变在下、壁纸在上)交给两条后端自己画
            frame.bg.extend(self.bg.iter().cloned());
            // 标签栏在下方时,内容到它上沿为止;图片会越过网格,靠这个收边
            frame.clip_bottom = (h as f64 - self.content_bottom).max(0.0) as f32;
        }

        // 平滑滚动的亚行偏移:内容整体上移不到一行(0..cell_h),第一行可以
        // 是负坐标,由画布裁剪。像素级截断不影响观感,而逐行跳变会"一格一格"。
        let shift = -(snap.scroll_frac as f64) * self.cell_h;

        // 图片:wezterm-term 把一张图切成了一格一块,每块带自己的纹理坐标。
        // 这里按 uv 反推出整图矩形,每张图每帧只发一个四边形 —— 逐格绘制会
        // 在格线处留下接缝。
        let mut placements: Vec<([u8; 32], Arc<ImageData>, [f32; 4], i32)> = Vec::new();

        // 第一遍:先铺满所有单元格背景。
        // 注意必须与第二遍分离:CJK 等宽字符的字形会溢出到右侧的续格,
        // 若逐格"先填背景再画字形",相邻续格的背景会把字形的右半部分覆盖掉。
        for (r, row) in snap.rows.iter().enumerate() {
            let y = origin_y + r as f64 * self.cell_h + shift;
            if y >= h as f64 {
                break;
            }
            let mut cx = self.pad_x;
            for cell in row.iter() {
                if cx < w as f64 && cx + self.cell_w > 0.0 {
                    // 默认底色(= 调色板 bg)的处理:
                    // - 既没有背景层也没有自定义纯色:照常铺,和 clear 同色,
                    //   看不出来但保证覆盖
                    // - 有壁纸/渐变/自定义纯色:必须让位,否则每格一个不透明
                    //   矩形会把整张背景盖死(清屏色已经铺满,不会露底)
                    // 整窗不透明度由窗口层(层叠窗口统一 alpha)负责,
                    // 这里不用再管 alpha。
                    let default_bg = cell.bg == palette.bg;
                    let draw_bg = !default_bg || (self.bg.is_empty() && self.bg_color.is_none());
                    if draw_bg {
                        frame.rect(cx, y, self.cell_w, self.cell_h, cell.bg, 1.0);
                    }
                    if self.images_enabled {
                        for im in &cell.images {
                            let Some(quad) =
                                image_quad(cx, y, self.cell_w, self.cell_h, im, snap.term_cell_px)
                            else {
                                continue;
                            };
                            let anchor = (quad[0].round() as i32, quad[1].round() as i32);
                            let dup = placements.iter().any(|p| {
                                p.0 == im.hash
                                    && p.3 == im.z
                                    && p.2[0].round() as i32 == anchor.0
                                    && p.2[1].round() as i32 == anchor.1
                            });
                            if !dup {
                                placements.push((im.hash, im.data.clone(), quad, im.z));
                            }
                        }
                    }
                }
                cx += self.cell_w;
            }
        }

        // 选区高亮:整段一行一个矩形,而不是逐格(减少实例数);坐标已由
        // main 从绝对物理行号换算到当前视口行,亚行滚动时跟着 shift 一起动。
        if let Some(hl) = highlight {
            let cols = snap.rows.first().map_or(0, Vec::len);
            if cols > 0 {
                let ((c0, r0), (c1, r1)) = hl.normalized();
                let last_row = snap.rows.len().saturating_sub(1);
                for r in r0..=r1.min(last_row) {
                    let y = origin_y + r as f64 * self.cell_h + shift;
                    if y >= h as f64 || y + self.cell_h <= 0.0 {
                        continue;
                    }
                    let a = if r == r0 { c0 } else { 0 }.min(cols - 1);
                    let b = if r == r1 { c1 } else { cols - 1 }.min(cols - 1);
                    if a > b {
                        continue;
                    }
                    frame.rect(
                        self.pad_x + a as f64 * self.cell_w,
                        y,
                        (b - a + 1) as f64 * self.cell_w,
                        self.cell_h,
                        hl.color,
                        hl.alpha,
                    );
                }
            }
        }

        // 负 z_index 的图压在文字下面(部分应用的背景图)
        self.push_images(frame, &placements, true);

        // 第二遍:绘制字形、下划线、删除线、上划线
        for (r, row) in snap.rows.iter().enumerate() {
            let y = origin_y + r as f64 * self.cell_h + shift;
            if y >= h as f64 {
                break;
            }
            let mut cx = self.pad_x;
            let th = (self.cell_h / 14.0).ceil().max(1.0);
            for cell in row.iter() {
                if cx < w as f64 && cx + self.cell_w > 0.0 {
                    // 闪烁关闭的相位:只留背景,不画字形
                    if !cell.invisible && !(cell.blink && !blink_on) && !cell.text.is_empty() {
                        self.draw_text(
                            frame,
                            &cell.text,
                            cx,
                            y + self.ascent,
                            cell.fg,
                            StyleBits {
                                bold: cell.bold,
                                italic: cell.italic,
                            },
                            cell.span,
                        );
                    }
                    if cell.overline {
                        frame.rect(cx, y + 1.0, self.cell_w, th, cell.fg, 1.0);
                    }
                    if cell.underline != Underline::None {
                        push_underline(
                            frame,
                            cx,
                            y + self.ascent + 1.0,
                            self.cell_w,
                            cell.underline,
                            cell.underline_color,
                            th,
                        );
                    }
                    if cell.strike {
                        let sy = y + self.cell_h * 0.52;
                        let sth = (self.cell_h / 16.0).ceil().max(1.0);
                        frame.rect(cx, sy, self.cell_w, sth, cell.fg, 1.0);
                    }
                }
                cx += self.cell_w;
            }
        }

        // 其余图片画在文字上面(Kitty 的 z_index >= 0,也是 iTerm2 / sixel 的默认)
        self.push_images(frame, &placements, false);

        // 尾迹:一个被拉伸/剪切的半透明四边形;当前光标矩形区域挖空,由光标本体保持清晰
        if let Some(t) = trail {
            let mask = self.cell_rect(cursor.col, cursor.row);
            frame.trail(
                t.corners,
                trail_color,
                t.opacity,
                [
                    mask.x as f32,
                    mask.y as f32,
                    (mask.x + mask.w) as f32,
                    (mask.y + mask.h) as f32,
                ],
            );
        }

        // 光标本体立即绘制在目标位置(不滑行,与 kitty 一致)
        if cursor.visible {
            self.draw_static_cursor(frame, snap, cursor, palette);
        }
    }

    /// 把收集到的图片按 z_index 分层发出:负 z 在文字下,其余在文字上。
    /// 解码失败(坏图 / 不认识的编码)就跳过,不会留下一块空矩形。
    fn push_images(
        &mut self,
        frame: &mut Frame,
        placements: &[([u8; 32], Arc<ImageData>, [f32; 4], i32)],
        below: bool,
    ) {
        let mut chosen: Vec<&([u8; 32], Arc<ImageData>, [f32; 4], i32)> =
            placements.iter().filter(|p| (p.3 < 0) == below).collect();
        // 同一层内按 z 升序,后面的盖住前面的
        chosen.sort_by_key(|p| p.3);
        for (key, data, dst, _) in chosen {
            if self.images.decode(data).is_none() {
                continue;
            }
            frame.image(
                *key,
                dst[0] as f64,
                dst[1] as f64,
                dst[2] as f64,
                dst[3] as f64,
            );
        }
    }

    /// 标签栏绘制:作为叠加层追加到显示列表末尾。
    ///
    /// `frame.overlay_at` 之后的内容不参与内容区裁剪(CPU 的 clip_top /
    /// GPU 的 scissor),所以标签栏不会被整条切掉;两条后端仍然共用同一份
    /// 绘制指令,切换后端外观一致。
    pub fn build_tab_bar(&mut self, frame: &mut Frame, w: u32, _h: u32, draw: &TabBarDraw<'_>) {
        frame.overlay_at = Some(frame.items.len());
        let bar = draw.layout.bar;
        let content = draw.layout.content;
        let default_bg = Rgb {
            r: 12,
            g: 12,
            b: 12,
        };
        let bar_bg = crate::term::color_from_str(&draw.colors.background).unwrap_or(default_bg);
        // 背景铺满整条栏(含靠窗口边框那侧的留白),四角交给 DWM 裁圆;
        // 标签/按钮只画在 content 带里,四周留白才均匀。
        frame.rect(0.0, bar.y, w as f64, bar.h, bar_bg, 1.0);

        let pad = (self.cell_w * 0.55).round().max(2.0);
        let baseline = content.y + (content.h - self.cell_h) / 2.0 + self.ascent;
        let edge = crate::term::color_from_str(&draw.colors.edge);
        let default_fg = Rgb {
            r: 220,
            g: 220,
            b: 220,
        };

        // 圆角半径:fancy 才圆角;显式配置优先,否则按标签栏高度推一个
        // 肉眼能看出来的值(约等于 Windows 11 标签页的观感)
        let radius = if draw.fancy {
            draw.corner_radius
                .unwrap_or((content.h * 0.36).round())
                .clamp(0.0, content.h * 0.45)
        } else {
            0.0
        };

        for (i, item) in draw.items.iter().enumerate() {
            let Some(r) = draw.layout.tabs.get(i) else {
                continue;
            };
            let (bg_s, fg_s) = if item.active {
                (&draw.colors.active.bg, &draw.colors.active.fg)
            } else if item.hovered {
                (
                    &draw.colors.inactive_hover.bg,
                    &draw.colors.inactive_hover.fg,
                )
            } else {
                (&draw.colors.inactive.bg, &draw.colors.inactive.fg)
            };
            let bg = item
                .bg
                .or_else(|| crate::term::color_from_str(bg_s))
                .unwrap_or(bar_bg);
            let fg = item
                .fg
                .or_else(|| crate::term::color_from_str(fg_s))
                .unwrap_or(default_fg);
            // fancy 模式左右各留 1px 缝,看起来像独立的块;扁平模式连成一片
            let gap = if draw.fancy { 1.0 } else { 0.0 };
            let inner_w = (r.w - gap * 2.0).max(1.0);
            let bx = r.x + gap;
            frame.round_rect(bx, r.y, inner_w, r.h, radius, bg, 1.0);
            // wezterm 的 fancy tab:只有背向内容区的两角是圆的,朝内容区
            // 那条边补成直角,视觉上和终端连成一片。活动/非活动标签一致,
            // 不做成四角全圆的"胶囊",否则贴着窗口圆角时会被裁得很难看。
            if radius >= 1.0 {
                let sq = radius.min(r.h * 0.5);
                let y = if draw.at_bottom { r.y } else { r.y + r.h - sq };
                frame.rect(bx, y, inner_w, sq, bg, 1.0);
            }
            if item.active {
                // 活动标签的强调边:顶部栏画在下沿,底部栏画在上沿
                let th = (self.cell_h * 0.07).round().max(2.0);
                let y = if draw.at_bottom { r.y } else { r.y + r.h - th };
                frame.rect(bx, y, inner_w, th, fg, 1.0);
            } else if !draw.fancy {
                if let Some(c) = edge {
                    frame.rect(r.x + r.w - 1.0, r.y, 1.0, r.h, c, 1.0);
                }
            }

            // 标题:序号 + 标题,按单元格宽度裁剪(中文占两格)。
            // 有关闭按钮时右边要给它让位,否则标题会被按钮压住
            let close = draw.layout.closes.get(i).copied().flatten();
            let text_right = match close {
                Some(c) => c.x - pad * 0.3,
                None => r.x + r.w - pad * 0.4,
            };
            let cap = crate::tabbar::title_capacity(
                (text_right - r.x - pad).max(0.0),
                self.cell_w,
                draw.show_index,
            );
            match &item.spans {
                // `format-tab-title` 给了文字:原样画(它自己带颜色、自己管内容)
                Some(spans) => {
                    self.draw_rich(frame, spans, r.x + pad, text_right, baseline, fg);
                }
                None => {
                    let title = crate::tabbar::fit_title_cells(&item.title, cap);
                    let text = if draw.show_index {
                        format!("{}: {}", i + 1, title)
                    } else {
                        title
                    };
                    self.draw_label(
                        frame,
                        &text,
                        r.x + pad,
                        text_right,
                        baseline,
                        fg,
                        StyleBits {
                            bold: item.active,
                            italic: false,
                        },
                    );
                }
            }

            // 关闭按钮:悬停时整个按钮底变亮,点击只关这一个标签
            if let Some(c) = close {
                if item.close_hovered {
                    let cr = (c.h * 0.3).round().max(2.0);
                    frame.round_rect(c.x, c.y, c.w, c.h, cr, fg, 0.22);
                }
                let x = c.x + ((c.w - self.cell_w) / 2.0).max(0.0);
                let baseline = c.y + (c.h - self.cell_h) / 2.0 + self.ascent;
                self.draw_label(
                    frame,
                    "×",
                    x,
                    c.x + c.w,
                    baseline,
                    fg,
                    StyleBits {
                        bold: false,
                        italic: false,
                    },
                );
            }
        }

        // 新建标签按钮
        if let Some(r) = draw.layout.new_tab {
            let (bg_s, fg_s) = if draw.new_tab_hovered {
                (&draw.colors.new_tab_hover.bg, &draw.colors.new_tab_hover.fg)
            } else {
                (&draw.colors.new_tab.bg, &draw.colors.new_tab.fg)
            };
            let bg = crate::term::color_from_str(bg_s).unwrap_or(bar_bg);
            let fg = crate::term::color_from_str(fg_s).unwrap_or(default_fg);
            let gap = if draw.fancy { 1.0 } else { 0.0 };
            frame.round_rect(
                r.x + gap,
                r.y,
                (r.w - gap * 2.0).max(1.0),
                r.h,
                radius,
                bg,
                1.0,
            );
            let x = r.x + ((r.w - self.cell_w) / 2.0).max(0.0);
            self.draw_label(
                frame,
                "+",
                x,
                r.x + r.w,
                baseline,
                fg,
                StyleBits {
                    bold: false,
                    italic: false,
                },
            );
        }

        // 窗口控制按钮(只有 window_decorations 带 INTEGRATED_BUTTONS 才有)
        self.draw_window_buttons(frame, draw, bar_bg, default_fg);

        // 左右状态栏:wezterm 把它们和标签放在同一行,右状态栏浮到最右。
        // 窗口很窄时 `tabbar::layout` 会把它们的占位压小,这里按最终宽度截断 ——
        // 左边丢尾巴、右边丢开头(时间比日期重要)。
        if let Some(r) = draw.layout.left_status {
            let max = (r.w / self.cell_w).floor() as usize;
            let spans = crate::status::head_to_cells(draw.left_status, max);
            self.draw_rich(frame, &spans, r.x, r.x + r.w, baseline, default_fg);
        }
        if let Some(r) = draw.layout.right_status {
            // 状态栏可以有一套自己的字号与左右留白(`status_bar.font_scale` /
            // `padding`),和标签栏正文互不影响 —— 想"字小一点、离右边缘远一点"
            // 就调那两个键,不用改终端字号。
            let scale = draw.status_scale.clamp(0.5, 2.0);
            let pad = draw.status_pad.max(0.0);
            let gap = draw.status_gap.max(0.0);
            let space = draw.status_space.clamp(0.0, 2.0);
            let cw = self.advance_w * scale;
            let avail = (r.w - pad * 2.0).max(0.0);
            // 片段之间的间距也占位置:先按字宽收一次,多出来的宽度再按
            // 实际片段数收一次(片段数只有截断后才知道)。空格按
            // `space_width` 折算,所以收窄空格之后能塞下更多字。
            let mut max = (avail / cw).max(0.0);
            let mut spans = crate::status::tail_to_units(draw.right_status, max, space);
            let mut w = crate::status::width_units(&spans, space) * cw
                + gap * spans.len().saturating_sub(1) as f64;
            if w > avail && spans.len() > 1 {
                let room = (avail - gap * spans.len().saturating_sub(1) as f64).max(0.0);
                max = (room / cw).max(0.0);
                spans = crate::status::tail_to_units(draw.right_status, max, space);
                w = crate::status::width_units(&spans, space) * cw
                    + gap * spans.len().saturating_sub(1) as f64;
            }
            let x = (r.x + r.w - pad - w).max(r.x);
            // 垂直居中:按缩放后的字高算基线
            let baseline = r.y + (r.h - self.cell_h * scale) / 2.0 + self.ascent * scale;
            self.draw_rich_scaled(
                frame,
                &spans,
                x,
                (r.x + r.w - pad).max(x),
                baseline,
                default_fg,
                scale,
                gap,
                space,
            );
        }
    }

    /// 画一段带颜色的文字:逐字符推进,超出 `max_x` 截止。
    ///
    /// 每个片段的底色先铺一次矩形再叠字,顺序和终端里画反显一样 ——
    /// 先铺底、后画字形,字形边缘不会和底色打架。
    fn draw_rich(
        &mut self,
        frame: &mut Frame,
        spans: &[crate::status::Span],
        x: f64,
        max_x: f64,
        baseline: f64,
        default_fg: Rgb,
    ) {
        self.draw_rich_scaled(frame, spans, x, max_x, baseline, default_fg, 1.0, 0.0, 1.0);
    }

    /// 同 [`draw_rich`](Self::draw_rich),但整段按 `scale` 缩放字形。
    ///
    /// 右侧状态栏有自己的一套字号(`status_bar.font_scale`),所以字体栅格化
    /// 拿到的位图是终端字号对应的,这里把位图按 `scale` 缩放着画。`scale`
    /// 等于 1.0 时直接走原来的 `draw_text`,像素级和以前完全一致。
    ///
    /// `gap` 是相邻片段之间的额外间距(物理像素),对应 `status_bar.item_spacing`。
    #[allow(clippy::too_many_arguments)]
    fn draw_rich_scaled(
        &mut self,
        frame: &mut Frame,
        spans: &[crate::status::Span],
        mut x: f64,
        max_x: f64,
        baseline: f64,
        default_fg: Rgb,
        scale: f64,
        gap: f64,
        space: f64,
    ) {
        let exact = (scale - 1.0).abs() < 0.01;
        // 浮点容差:宽度是一串 `cell_w × 倍数` 累加出来的,`x` 先加还是后加会
        // 差最后一位 —— 差 1e-13 就足以让**整段最后一个字**被判成"放不下"。
        // 以前状态栏里"晚上好"只显示成"晚上"就是这个原因。
        const WIDTH_EPS: f64 = 0.01;
        // 空格的推进量单独缩放(`status_bar.space_width`)。脚本里的"分隔带"
        // 就是一串空格(`'   ·   '`),缩放它们就能收紧切片之间的距离,
        // 而字形本身还是按原尺寸栅格化,不会被压扁。
        let space_w = space.clamp(0.0, 2.0);
        // 状态栏不是网格:按字体自然字距推进(wezterm 的 shaper 也是这样),
        // 用取整后的 `cell_w` 会让每个字符多占零点几像素,一行下来显得很松。
        let cell_w = self.advance_w * scale;
        let cell_h = self.cell_h * scale;
        let ascent = self.ascent * scale;
        for span in spans {
            // 先算出这一段画得下多少
            let mut used = 0.0f64;
            for ch in span.text.chars() {
                let w = if ch == ' ' {
                    cell_w * space_w
                } else {
                    crate::tabbar::char_width(ch) as f64 * cell_w
                };
                if x + used + w > max_x + WIDTH_EPS {
                    break;
                }
                used += w;
            }
            if used <= 0.0 {
                break;
            }
            if let Some(bg) = span.bg {
                frame.rect(x, baseline - ascent, used, cell_h, bg, 1.0);
            }
            let fg = span.fg.unwrap_or(default_fg);
            let style = StyleBits {
                bold: span.bold,
                italic: span.italic,
            };
            // 推进量从 0 开始累加(和上面算 `used` 完全同一个顺序、同一组数字),
            // 这样"算宽度"和"画"两边不会因为浮点加法次序不同而差出最后一位
            let mut off = 0.0f64;
            for ch in span.text.chars() {
                let cells = crate::tabbar::char_width(ch);
                let blank = ch == ' ';
                let w = if blank {
                    cell_w * space_w
                } else {
                    cells as f64 * cell_w
                };
                if off + w > used + WIDTH_EPS {
                    break;
                }
                let cx = x + off;
                // 空格没有墨迹:只推进,不进绘制队列(收窄后也不会画出字来)
                if !blank {
                    // 图标一律走 draw_status_glyph —— 它会把"画满 em 框"的
                    // 图标收到文字高度并对齐中线。`font_scale` 正好 1.0 时
                    // 也照做,否则改一下字号倍数图标就换了个大小,很怪。
                    if exact && glyph_role(ch) != GlyphRole::Icon {
                        let mut buf = [0u8; 4];
                        let s = ch.encode_utf8(&mut buf);
                        self.draw_text(frame, s, cx, baseline, fg, style, cells as u8);
                    } else {
                        self.draw_status_glyph(
                            frame, ch, cx, baseline, fg, style, w, cell_h, scale,
                        );
                    }
                }
                off += w;
                if span.underline {
                    let th = (cell_h * 0.06).round().max(1.0);
                    frame.rect(
                        x + off - w,
                        baseline + cell_h - ascent - th,
                        w,
                        th,
                        fg,
                        1.0,
                    );
                }
            }
            x += used + gap.max(0.0);
        }
    }

    /// 状态栏里画一个字符。字号由 `scale` 决定 —— **按这个尺寸重新栅格化**,
    /// 和 wezterm 画标签栏 / 状态栏的方式一致(它也是按 tab bar 的字号出位图)。
    ///
    /// `cell_w` / `cell_h` 是缩放后的格子大小,`baseline` 也是缩放后的基线;
    /// 位图本身已经是目标尺寸,所以这里只做"摆位",没有任何插值缩放。
    #[allow(clippy::too_many_arguments)]
    fn draw_status_glyph(
        &mut self,
        frame: &mut Frame,
        ch: char,
        ox: f64,
        baseline: f64,
        color: Rgb,
        style: StyleBits,
        cell_w: f64,
        cell_h: f64,
        scale: f64,
    ) {
        let Some(mut id) = self.glyph_scaled(ch, style, scale) else {
            return;
        };
        match glyph_role(ch) {
            GlyphRole::Icon => {
                // 图标:按"文字的高度"摆,而不是铺满整格。
                //
                // Nerd Font 里图标的墨迹占多大,完全看它是从哪套图形转来的:
                // 月亮(󰖚)只有半个字高,MDI 的电池(󰁹)却画满整个 em 框 ——
                // 直接塞进格子的话,电池会比旁边的 `97%` 高出一半、底边还坠到
                // 基线以下,怎么看怎么歪。所以拿数字 `0` 的墨迹当基准:
                // 比数字明显高的图标按比例重新栅格化小一档,再把**墨迹中线**
                // 对齐到数字中线,一排里所有图标就都坐在同一条线上。
                let (band_c, band_h) = self.text_band(style, scale);
                let big = {
                    let g = &self.glyphs[id as usize];
                    band_h > 1.0 && g.h as f64 > band_h * ICON_OVERSIZE
                };
                if big {
                    let h0 = self.glyphs[id as usize].h as f64;
                    let want = band_h * ICON_RATIO;
                    // 重新栅格化比缩位图清晰:字形的笔画宽度跟着字号走
                    if let Some(small) = self.glyph_scaled(ch, style, scale * want / h0) {
                        id = small;
                    }
                }
                let g = &self.glyphs[id as usize];
                let (gw, gh) = (g.w as f64, g.h as f64);
                let s = (cell_w / gw).min(cell_h / gh).min(1.0);
                let (dw, dh) = (gw * s, gh * s);
                let dx = ox + (cell_w - dw) / 2.0;
                // 墨迹中线对齐文字中线(band_c 是相对基线、向上为负的偏移)
                let dy = baseline + band_c - dh / 2.0;
                let bold = g.synthetic_bold;
                frame.glyph(
                    id,
                    (0.0, 0.0, gw, gh),
                    (dx, dy, dw, dh),
                    color,
                    style.italic,
                    false,
                );
                if bold {
                    // 合粗体:按字号等比偏移,不是固定 1px(大字号才看得出差别)
                    frame.glyph(
                        id,
                        (0.0, 0.0, gw, gh),
                        (dx + (0.5 * scale).max(1.0), dy, dw, dh),
                        color,
                        style.italic,
                        false,
                    );
                }
            }
            _ => {
                let g = &self.glyphs[id as usize];
                let (gw, gh) = (g.w as f64, g.h as f64);
                let bold = g.synthetic_bold;
                let dst = (ox + g.left as f64, baseline + g.top as f64, gw, gh);
                frame.glyph(id, (0.0, 0.0, gw, gh), dst, color, style.italic, false);
                if bold {
                    frame.glyph(
                        id,
                        (0.0, 0.0, gw, gh),
                        (dst.0 + (0.5 * scale).max(1.0), dst.1, dst.2, dst.3),
                        color,
                        style.italic,
                        false,
                    );
                }
            }
        }
    }

    /// 状态栏里"一行文字"的视觉带:墨迹中线(相对基线,向上为负)与字高。
    ///
    /// 用数字 `0` 量:它既没有升部也没有降部,墨迹高度就是视觉字高。拿它当
    /// 基准去摆图标,换字体、改字号都对得上,不用写死任何比例。
    fn text_band(&mut self, style: StyleBits, scale: f64) -> (f64, f64) {
        // 极端情况(字体没有数字):退回"上 2/3 格"的估算
        let fallback = (-self.cell_h * scale * 0.31, self.cell_h * scale * 0.62);
        let Some(id) = self.glyph_scaled('0', style, scale) else {
            return fallback;
        };
        let g = &self.glyphs[id as usize];
        if g.h == 0 {
            return fallback;
        }
        (g.top as f64 + g.h as f64 / 2.0, g.h as f64)
    }

    /// 标签栏里的窗口按钮。按钮集合、顺序、对齐由 `tabbar::layout` 决定,
    /// 这里只管画:默认方形图标(Windows),也可切成 Gnome 的圆形按钮底。
    /// 图标用矩形拼,不依赖字体里有没有 ▢ / ✕ 这类字符。
    fn draw_window_buttons(
        &mut self,
        frame: &mut Frame,
        draw: &TabBarDraw<'_>,
        bar_bg: Rgb,
        default_fg: Rgb,
    ) {
        use crate::tabbar::WindowButton;
        let bar_h = draw.layout.content.h;
        let s = crate::tabbar::window_icon_size(bar_h);
        let th = (s / 7.5).round().max(1.0);
        let fg = draw.button_color.unwrap_or(default_fg);
        // 配了自定义色就用它当强调色(关闭悬停底),否则沿用 Windows 红
        let accent = draw.button_color.unwrap_or(Rgb {
            r: 196,
            g: 43,
            b: 28,
        });

        for (kind, r) in &draw.layout.window_buttons {
            let hovered = draw.window_hovered == Some(*kind);
            if draw.button_gnome_style {
                // Adwaita:圆形底。悬停时圆更亮一点
                let d = (bar_h * 0.62).round().clamp(12.0, bar_h);
                let cx = r.x + r.w / 2.0;
                let cy = r.y + r.h / 2.0;
                let base = if hovered && *kind == WindowButton::Close {
                    accent
                } else {
                    Rgb {
                        r: 255,
                        g: 255,
                        b: 255,
                    }
                };
                let alpha = if hovered { 0.32 } else { 0.14 };
                frame.round_rect(cx - d / 2.0, cy - d / 2.0, d, d, d / 2.0, base, alpha);
            } else {
                // Windows:整块矩形高亮,关闭悬停是红底
                match (kind, hovered) {
                    (WindowButton::Close, true) => {
                        frame.rect(r.x, r.y, r.w, r.h, accent, 1.0);
                    }
                    (_, true) => {
                        frame.rect(
                            r.x,
                            r.y,
                            r.w,
                            r.h,
                            Rgb {
                                r: 255,
                                g: 255,
                                b: 255,
                            },
                            0.12,
                        );
                    }
                    _ => {
                        frame.rect(r.x, r.y, r.w, r.h, bar_bg, 1.0);
                    }
                }
            }
            let cx = r.x + r.w / 2.0;
            let cy = r.y + r.h / 2.0;
            let x0 = (cx - s / 2.0).round();
            let y0 = (cy - s / 2.0).round();

            match kind {
                // 最小化:一条横线
                WindowButton::Minimize => {
                    frame.rect(x0, (cy - th / 2.0).round(), s, th, fg, 1.0);
                }
                // 最大化:方框;已最大化时画"还原"的双框
                WindowButton::Maximize => {
                    if draw.maximized {
                        let off = (th * 2.0).max(2.0);
                        let inner = s - off;
                        // 后面那层的上边与右边
                        frame.rect(x0 + off, y0, inner, th, fg, 1.0);
                        frame.rect(x0 + s - th, y0, th, inner, fg, 1.0);
                        // 前面那层的完整方框
                        frame.rect(x0, y0 + off, inner, th, fg, 1.0);
                        frame.rect(x0, y0 + s - th, inner, th, fg, 1.0);
                        frame.rect(x0, y0 + off, th, inner, fg, 1.0);
                        frame.rect(x0 + inner - th, y0 + off, th, inner, fg, 1.0);
                    } else {
                        frame.rect(x0, y0, s, th, fg, 1.0);
                        frame.rect(x0, y0 + s - th, s, th, fg, 1.0);
                        frame.rect(x0, y0, th, s, fg, 1.0);
                        frame.rect(x0 + s - th, y0, th, s, fg, 1.0);
                    }
                }
                // 关闭:两条对角线,用重叠的小方块拼成
                WindowButton::Close => {
                    let span = s - th;
                    let steps = (span * 1.6).round().max(4.0) as i32;
                    for i in 0..=steps {
                        let t = i as f64 / steps as f64;
                        let px = x0 + t * span;
                        let py = y0 + t * span;
                        frame.rect(px, py, th, th, fg, 1.0);
                        frame.rect(px, y0 + span - t * span, th, th, fg, 1.0);
                    }
                }
            }
        }
    }

    /// 关闭确认的版式。逐行照抄 wezterm 的 `run_confirmation_impl`:
    ///
    /// ```text
    /// ● Really kill this window and all contained tabs and panes?
    ///
    ///  [Y]es         [N]o
    /// ```
    pub fn confirm_layout(&self, w: u32, h: u32, message: &str) -> ConfirmLayout {
        let cw = self.cell_w.max(1.0);
        let ch = self.cell_h.max(1.0);
        // 内容区上下界:标签栏在上面就压低上界,在下面就抬下界
        let top = self.content_top;
        let bottom = (h as f64 - self.content_bottom).max(top + ch);

        let cols = ((w as f64 / cw).floor() as usize).max(1);
        let rows = (((bottom - top) / ch).floor() as usize).max(1);

        // 正文占 80% 列宽、左边界在 10% 处 —— wezterm 的两个硬编码百分比
        let text_cols = (cols * 80 / 100).max(1);
        let x_col = cols * 10 / 100;

        let lines = wrap_cells(message, text_cols);
        let message_rows = lines.len();
        // "整块 = 正文 + 一个空行 + 按钮行",据此垂直居中
        let top_row = rows.saturating_sub(message_rows + 2) / 2;
        let button_row = top_row + message_rows + 1;

        // ` [Y]es ` 7 格,中间空 8 格,` [N]o ` 6 格
        const YES_W: usize = 7;
        const SPACER: usize = 8;
        const NO_W: usize = 6;

        ConfirmLayout {
            top,
            bottom,
            cell_w: cw,
            cell_h: ch,
            lines,
            x_col,
            top_row,
            button_row,
            choice_col: [x_col, x_col + YES_W + SPACER],
            choice_w: [YES_W, NO_W],
        }
    }

    /// 关闭确认:清屏 + 居中文本,和 wezterm 的 overlay 一致。
    ///
    /// 没有面板、没有边框、没有遮罩,也没有按钮色块 —— 模态感来自"屏幕被
    /// 清空"本身。选中的那项用反显(reverse video),也就是整段铺前景色、
    /// 文字用背景色,这是 wezterm 里 `AttributeChange::Reverse` 的效果。
    pub fn build_confirm_dialog(
        &mut self,
        frame: &mut Frame,
        w: u32,
        h: u32,
        draw: &DialogDraw<'_>,
    ) {
        // 最后一个叠加层:标签栏若没画,也要在它之后,免得被裁剪
        if frame.overlay_at.is_none() {
            frame.overlay_at = Some(frame.items.len());
        }
        let l = self.confirm_layout(w, h, draw.message);
        let fg = draw.palette.fg;

        // `Change::ClearScreen(Default)`:整片内容区刷成清屏色。
        // 用 clear 而不是 palette.bg —— 配了 window_background_color 时
        // 两者不同,清屏色才和终端平时看到的底色一致。
        frame.rect(
            0.0,
            l.top,
            w as f64,
            (l.bottom - l.top).max(0.0),
            frame.clear,
            1.0,
        );

        for (i, line) in l.lines.iter().enumerate() {
            let y = l.row_y(l.top_row + i);
            let mut x = l.col_x(l.x_col);
            // 提示语前面那一个红点:wezterm 用 emoji,我们手绘一个圆点。
            // 点占第一格,文字从第三格起 —— 中间那一格就是原来 emoji 后面
            // 那个空格的位置。
            if i == 0 {
                self.draw_alert_dot(frame, x, y);
                x += l.cell_w * 2.0;
            }
            self.draw_label(
                frame,
                line.as_str(),
                x,
                w as f64,
                y + self.ascent,
                fg,
                StyleBits::default(),
            );
        }

        // 按钮行:两段带前后空格的文本,悬停/命中那项反显
        let by = l.row_y(l.button_row);
        for i in 0..2 {
            let x = l.col_x(l.choice_col[i]);
            let bw = l.choice_w[i] as f64 * l.cell_w;
            let active = draw.active == Some(i);
            if active {
                frame.rect(x, by, bw, l.cell_h, fg, 1.0);
            }
            let color = if active { frame.clear } else { fg };
            self.draw_label(
                frame,
                draw.choices[i],
                x,
                x + bw,
                by + self.ascent,
                color,
                StyleBits::default(),
            );
        }
    }

    /// 确认框里提示语前面那个红点。
    ///
    /// wezterm 那句提示开头是个 emoji(🛑),靠字体回退画成彩色停止牌;我们这边
    /// 等宽字体拿不到轮廓,原先手绘的八角形放大后像块膏药。这里只画一个实心
    /// 圆:直径半格、颜色取纯红,垂直位置按文字的视觉中线(大写字母上半截)
    /// 对齐,而不是格子几何中心 —— 那样会显得坠在下面。
    fn draw_alert_dot(&mut self, frame: &mut Frame, x: f64, y: f64) {
        let d = (self.cell_h * 0.5).round().max(3.0);
        let cx = x + (self.cell_w * 0.5).round();
        let cy = y + self.ascent - self.cell_h * 0.27;
        frame.round_rect(
            cx - d / 2.0,
            cy - d / 2.0,
            d,
            d,
            d / 2.0,
            Rgb {
                r: 0xE5,
                g: 0x3B,
                b: 0x44,
            },
            1.0,
        );
    }

    /// 输入法组合串(拼音/假名还没上屏的那一段)。
    ///
    /// 画在光标格右下角:一层底色 + 描边 + 文字,正在转换的那一段加下划线。
    /// 不写进终端内容、也不发给 PTY —— 上屏时 `Ime::Commit` 才真正提交。
    pub fn build_ime_box(
        &mut self,
        frame: &mut Frame,
        x: f64,
        y: f64,
        text: &str,
        cursor: Option<(usize, usize)>,
        palette: &Palette,
    ) {
        if text.is_empty() {
            return;
        }
        if frame.overlay_at.is_none() {
            frame.overlay_at = Some(frame.items.len());
        }
        let pad = (self.cell_w * 0.25).round().max(2.0);
        let tw = self.text_width(text);
        let w = tw + pad * 2.0;
        let h = self.cell_h + pad * 2.0;
        // 贴着光标格往下放一格,避免盖住正在输入的位置;右边越界就往左顶
        let max_x = (frame.width as f64 - w - 2.0).max(0.0);
        let bx = x.min(max_x).max(0.0);
        let by = y + self.cell_h;
        frame.round_rect(bx - 1.0, by - 1.0, w + 2.0, h + 2.0, 5.0, palette.fg, 0.45);
        frame.round_rect(bx, by, w, h, 4.0, palette.bg, 0.97);
        self.draw_label(
            frame,
            text,
            bx + pad,
            bx + w,
            by + pad + self.ascent,
            palette.fg,
            StyleBits::default(),
        );
        // 下划线标出"正在转换"的那一段(没有范围就整串标)
        let (from, to) = match cursor {
            Some((f, l)) if l > f => (f, l),
            _ => (0, text.len()),
        };
        let mut px = bx + pad;
        let mut offset = 0usize;
        let mut start = None;
        for ch in text.chars() {
            let cw = crate::tabbar::char_width(ch) as f64 * self.cell_w;
            if offset == from {
                start = Some(px);
            }
            offset += ch.len_utf8();
            if offset == to {
                let sx = start.unwrap_or(px);
                frame.rect(
                    sx,
                    by + h - pad - 2.0,
                    px + cw - sx,
                    2.0,
                    palette.cursor_bg,
                    1.0,
                );
                start = None;
            }
            px += cw;
        }
        if let Some(sx) = start {
            frame.rect(sx, by + h - pad - 2.0, px - sx, 2.0, palette.cursor_bg, 1.0);
        }
    }

    /// 字符串按等宽格子量出来的宽度(宽字符算两格)
    pub fn text_width(&self, text: &str) -> f64 {
        text.chars()
            .map(|c| crate::tabbar::char_width(c) as f64 * self.cell_w)
            .sum()
    }

    /// 自绘面板(文件浏览器)用的一行文字:基线由调用方给,超出 `max_x` 的字符
    /// 直接不画。内部和标签栏标题走同一条 `draw_label` 路径,字形表、宽字符
    /// 推进、合成加粗都一致。
    #[allow(clippy::too_many_arguments)]
    pub fn draw_plain_text(
        &mut self,
        frame: &mut Frame,
        text: &str,
        x: f64,
        baseline: f64,
        max_x: f64,
        color: Rgb,
        bold: bool,
    ) {
        self.draw_label(
            frame,
            text,
            x,
            max_x,
            baseline,
            color,
            StyleBits {
                bold,
                italic: false,
            },
        );
    }

    /// 同 [`draw_plain_text`](Self::draw_plain_text),但整段按 `scale` 缩放字形。
    ///
    /// 搜索栏 / 命令面板的进出场动画要"框放大、字也跟着放大",光缩放背景矩形
    /// 会让字从边框里溢出来。这里复用状态栏那条 `draw_rich_scaled` 路径:字形按
    /// `scale` **重新栅格化**(不是把位图拉伸),`scale == 1.0` 时直接回落到
    /// 原来的 `draw_plain_text`,像素级和以前完全一致。
    ///
    /// 与网格文字不同,这里按字体自然字距(`advance_w`)推进,所以宽度要用
    /// [`text_width_at`](Self::text_width_at) 算,不要拿 `text_width` 乘。
    pub fn draw_scaled_text(
        &mut self,
        frame: &mut Frame,
        text: &str,
        x: f64,
        baseline: f64,
        max_x: f64,
        color: Rgb,
        bold: bool,
        scale: f64,
    ) {
        if (scale - 1.0).abs() < 0.01 {
            return self.draw_plain_text(frame, text, x, baseline, max_x, color, bold);
        }
        let span = crate::status::Span {
            text: text.to_string(),
            fg: Some(color),
            bg: None,
            bold,
            italic: false,
            underline: false,
        };
        self.draw_rich_scaled(
            frame,
            std::slice::from_ref(&span),
            x,
            max_x,
            baseline,
            color,
            scale,
            0.0,
            1.0,
        );
    }

    /// 一串文字在 `scale` 下的自然宽度(与 [`draw_scaled_text`](Self::draw_scaled_text)
    /// 的推进量同一套算法),用于给输入光标定位。
    pub fn text_width_at(&self, text: &str, scale: f64) -> f64 {
        let cw = self.advance_w * scale;
        text.chars()
            .map(|c| crate::tabbar::char_width(c) as f64 * cw)
            .sum()
    }

    /// 在任意水平位置画一串等宽字符(标签栏标题用)。
    /// 宽字符按两格推进,`max_x` 之外直接不画(等价于裁剪)。
    #[allow(clippy::too_many_arguments)]
    fn draw_label(
        &mut self,
        frame: &mut Frame,
        text: &str,
        mut x: f64,
        max_x: f64,
        baseline: f64,
        color: Rgb,
        style: StyleBits,
    ) {
        for ch in text.chars() {
            let cells = crate::tabbar::char_width(ch);
            let w = cells as f64 * self.cell_w;
            if x + w > max_x {
                break;
            }
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            self.draw_text(frame, s, x, baseline, color, style, cells as u8);
            x += w;
        }
    }

    /// CPU 后端:按显示列表把像素刷进软缓冲
    pub fn paint_cpu(&self, buf: &mut [u32], w: u32, h: u32, frame: &Frame) {
        let mut canvas = Canvas {
            buf,
            w,
            h,
            clip_top: frame.clip_top.floor() as i32,
            clip_bottom: (frame.clip_bottom.ceil() as i32).min(h as i32),
            clip_l: 0,
            clip_r: w as i32,
        };
        canvas.fill_all(frame.clear);

        // ---- 背景层 ----
        // 铺满整个窗口(包括标签栏那一条),所以临时把裁剪放开;画完还原,
        // 后续内容仍然按内容区裁剪。
        if !frame.bg.is_empty() {
            if std::env::var("VECTORTERM_DEBUG_BG").is_ok() {
                crate::wlog!(
                    "[dbg] paint_cpu bg={} canvas={}x{} clear=({},{},{}) clip={}..{}",
                    frame.bg.len(),
                    w,
                    h,
                    frame.clear.r,
                    frame.clear.g,
                    frame.clear.b,
                    canvas.clip_top,
                    canvas.clip_bottom
                );
            }
            let (saved_top, saved_bottom) = (canvas.clip_top, canvas.clip_bottom);
            canvas.clip_top = 0;
            canvas.clip_bottom = h as i32;
            let (fw, fh) = (w as f32, h as f32);
            for layer in &frame.bg {
                let img = &*layer.image;
                let (iw, ih) = (img.width as f32, img.height as f32);
                if layer.fill == ImageFill::Tile {
                    let origins = crate::background::tile_origins(iw, ih, fw, fh, 512);
                    if origins.is_empty() {
                        // 块数爆炸(图太小/窗口太大):退化成 cover 一张,别画几千次
                        let r = crate::background::dst_rect(ImageFill::Cover, iw, ih, fw, fh);
                        canvas.blit_image_with(r, img, layer.alpha);
                    } else {
                        for o in origins {
                            canvas.blit_image_with([o[0], o[1], iw, ih], img, layer.alpha);
                        }
                    }
                } else {
                    let r = crate::background::dst_rect(layer.fill, iw, ih, fw, fh);
                    canvas.blit_image_with(r, img, layer.alpha);
                }
            }
            canvas.clip_top = saved_top;
            canvas.clip_bottom = saved_bottom;
        }

        // overlay(标签栏)之前的项按内容区裁剪;之后的不裁,否则标签栏
        // 会被 clip_top 整条切掉。
        let overlay = frame
            .overlay_at
            .unwrap_or(frame.items.len())
            .min(frame.items.len());
        for (i, item) in frame.items.iter().enumerate() {
            // 横向裁剪从面板的第一条指令开始;标签栏在内容区之外,一并放开
            if let Some(c) = frame.clip_x {
                if i == c.from {
                    canvas.clip_l = (c.x0.floor() as i32).max(0);
                    canvas.clip_r = (c.x1.ceil() as i32).min(w as i32);
                }
            }
            if i == overlay {
                canvas.clip_top = 0;
                canvas.clip_bottom = h as i32;
                canvas.clip_l = 0;
                canvas.clip_r = w as i32;
            }
            match item {
                Item::Rect {
                    x,
                    y,
                    w: rw,
                    h: rh,
                    color,
                    alpha,
                } => canvas.fill_rect(*x as f64, *y as f64, *rw as f64, *rh as f64, *color, *alpha),
                Item::RoundRect {
                    x,
                    y,
                    w: rw,
                    h: rh,
                    radius,
                    color,
                    alpha,
                } => canvas.fill_round_rect(
                    *x as f64,
                    *y as f64,
                    *rw as f64,
                    *rh as f64,
                    *radius as f64,
                    *color,
                    *alpha,
                ),
                Item::Glyph {
                    id,
                    src,
                    dst,
                    color,
                    italic,
                    bold,
                } => {
                    if let Some(g) = self.glyphs.get(*id as usize) {
                        blit_glyph(
                            &mut canvas,
                            g,
                            (src[0] as f64, src[1] as f64, src[2] as f64, src[3] as f64),
                            (dst[0] as f64, dst[1] as f64, dst[2] as f64, dst[3] as f64),
                            *color,
                            *italic,
                            *bold,
                        );
                    }
                }
                Item::Trail {
                    corners,
                    color,
                    alpha,
                    mask,
                } => canvas.fill_quad(
                    corners,
                    *color,
                    *alpha,
                    // mask 的布局是 [x0, y0, x1, y1]
                    (mask[0], mask[1], mask[2], mask[3]),
                ),
                Item::Image { key, dst } => {
                    if let Some(img) = self.images.cached(key) {
                        canvas.blit_image(*dst, &img);
                    }
                }
            }
        }
    }

    /// 一帧到底:构建列表 + CPU 光栅化(测试便捷入口)
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn paint(
        &mut self,
        buf: &mut [u32],
        w: u32,
        h: u32,
        snap: &ScreenSnapshot,
        cursor: &CursorInfo,
        palette: &Palette,
        trail: Option<TrailDraw>,
        trail_color: Rgb,
        blink_on: bool,
    ) {
        let mut frame = Frame::default();
        self.build_frame(
            &mut frame,
            w,
            h,
            snap,
            cursor,
            palette,
            trail,
            trail_color,
            blink_on,
        );
        self.paint_cpu(buf, w, h, &frame);
    }

    /// 在 (ox, baseline) 处绘制一个单元格里的字形。
    ///
    /// 三类字形分开处理:
    /// * **Separator**(powerline 箭头):直接铺满整格,水平压住相邻格
    /// * **BoxGlyph**(制表符、方块元素):按 em 盒映射到格子再裁边,线宽不跑
    /// * **Icon**(Nerd Font 私有区图标):等比缩进格子并夹住垂直范围
    /// * **Text**(普通文字、CJK):保持自然大小,只在墨迹超格时缩小
    fn draw_text(
        &mut self,
        frame: &mut Frame,
        text: &str,
        ox: f64,
        baseline: f64,
        color: Rgb,
        style: StyleBits,
        span: u8,
    ) {
        let chars: Vec<char> = text.chars().collect();
        let cell_w = self.cell_w;
        let cell_h = self.cell_h;
        let cell_span = span.max(1) as f64 * cell_w;
        let cell_top = baseline - self.ascent;
        let mut pen = ox;
        for ch in chars {
            // 彩色 emoji:CBDT 位图字体,先于单色字形链处理。位图已经按当前
            // 字号缩放好,居中放进这个字符占的格子里(emoji 通常是双宽)。
            if let Some(e) = self.emoji_cell(ch) {
                frame.image(
                    e.key,
                    pen + (cell_span - e.w) / 2.0,
                    baseline - e.bearing_y,
                    e.w,
                    e.h,
                );
                pen += cell_span;
                continue;
            }
            // 实心块元素(▀▄█▌▐ 和象限块):直接按格子几何填矩形,不走字形。
            // 半块字符画要求上下两行、左右相邻格像素级咬合,字形路径做不到
            // (见 `block_rects` 的说明)。
            if let Some(rects) = block_rects(ch) {
                // 坐标先对到整像素(和单元格背景一样向下取整,滚动的半行
                // 偏移也是小数),再按格子比例取边界的整像素位置。
                // 关键是"两条边各自取整后相减",而不是"起点取整 + 宽度取整":
                // 这样 ▀ 的底边与下一行 ▄ 的顶边必然落在同一个像素上,
                // 行高是奇数还是偶数都不会漏一条缝或压掉一行。
                let left = pen.floor();
                let top = cell_top.floor();
                for &(x0, y0, x1, y1) in rects {
                    let px = left + (x0 * cell_span).round();
                    let py = top + (y0 * cell_h).round();
                    let pw = left + (x1 * cell_span).round() - px;
                    let ph = top + (y1 * cell_h).round() - py;
                    if pw > 0.0 && ph > 0.0 {
                        frame.rect(px, py, pw, ph, color, 1.0);
                    }
                }
                pen += cell_span;
                continue;
            }
            let Some(id) = self.glyph(ch, style) else {
                continue;
            };
            let g = &self.glyphs[id as usize];
            let gw = g.w as f64;
            let gh = g.h as f64;
            let bold = g.synthetic_bold;
            match glyph_role(ch) {
                GlyphRole::Separator => {
                    // Powerline 分隔符:本身就是"填满整格"的纯几何图形。
                    // 直接映射到格子矩形(允许非等比),这样箭头一定贴住
                    // 上下边界、水平方向压住相邻格,色块之间不会有缝。
                    frame.glyph(
                        id,
                        (0.0, 0.0, gw, gh),
                        (ox, cell_top, cell_span, cell_h),
                        color,
                        false,
                        false,
                    );
                    if bold {
                        frame.glyph(
                            id,
                            (0.0, 0.0, gw, gh),
                            (ox + 1.0, cell_top, cell_span, cell_h),
                            color,
                            false,
                            false,
                        );
                    }
                    pen += cell_span;
                }
                GlyphRole::BoxGlyph => {
                    // 制表符/方块元素:按 em 盒 → 单元格做等比映射再裁边。
                    // 不能直接铺满,否则 2px 的横线会被拉成 20px 高的色带。
                    let sy = cell_h / (g.ascent - g.descent).max(1.0) as f64;
                    let sx = cell_span / g.advance.max(1.0) as f64;
                    let dy0 = cell_top + (g.ascent + g.top as f32) as f64 * sy;
                    let dh0 = gh * sy;
                    let mut src = (0.0, 0.0, gw, gh);
                    let mut dst = (ox + g.left as f64 * sx, dy0, gw * sx, dh0);
                    if dst.1 < cell_top {
                        let cut = cell_top - dst.1;
                        src.1 += cut / sy;
                        src.3 -= cut / sy;
                        dst.1 = cell_top;
                        dst.3 -= cut;
                    }
                    let bottom = cell_top + cell_h;
                    if dst.1 + dst.3 > bottom {
                        let cut = dst.1 + dst.3 - bottom;
                        src.3 -= cut / sy;
                        dst.3 -= cut;
                    }
                    if src.3 > 0.5 && dst.3 > 0.5 {
                        frame.glyph(id, src, dst, color, false, false);
                        if bold {
                            frame.glyph(
                                id,
                                src,
                                (dst.0 + 1.0, dst.1, dst.2, dst.3),
                                color,
                                false,
                                false,
                            );
                        }
                    }
                    pen += cell_span;
                }
                GlyphRole::Icon => {
                    let s = (cell_span / gw).min(cell_h / gh).min(1.0);
                    let (dw, dh) = (gw * s, gh * s);
                    let dx = ox + (cell_span - dw) / 2.0;
                    let mut dy = baseline + g.top as f64 * s;
                    // 夹进格子:图标字体的升部/降部与主字体不同,不夹会串行
                    dy = dy.clamp(cell_top, (cell_top + cell_h - dh).max(cell_top));
                    let dst = (dx, dy, dw, dh);
                    frame.glyph(id, (0.0, 0.0, gw, gh), dst, color, style.italic, false);
                    if bold {
                        frame.glyph(
                            id,
                            (0.0, 0.0, gw, gh),
                            (dst.0 + 1.0, dst.1, dst.2, dst.3),
                            color,
                            style.italic,
                            false,
                        );
                    }
                    pen += cell_span;
                }
                GlyphRole::Text => {
                    // 保持自然大小,只在墨迹超格时缩小(wezterm 的"只缩不拉")
                    let max_w = (span.max(1) as f64 + 0.25) * cell_w;
                    let s = if gw > max_w { max_w / gw } else { 1.0 };
                    let (dw, dh) = (gw * s, gh * s);
                    let dst = (pen + g.left as f64 * s, baseline + g.top as f64, dw, dh);
                    frame.glyph(id, (0.0, 0.0, gw, gh), dst, color, style.italic, false);
                    if bold {
                        frame.glyph(
                            id,
                            (0.0, 0.0, gw, gh),
                            (dst.0 + 1.0, dst.1, dst.2, dst.3),
                            color,
                            style.italic,
                            false,
                        );
                    }
                    pen += g.advance as f64 * s;
                }
            }
        }
    }

    fn draw_static_cursor(
        &mut self,
        frame: &mut Frame,
        snap: &ScreenSnapshot,
        cursor: &CursorInfo,
        palette: &Palette,
    ) {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Kind {
            Block,
            Bar,
            Underline,
        }

        let Some(row) = snap.rows.get(cursor.row) else {
            return;
        };
        let Some(cell) = row.get(cursor.col) else {
            return;
        };
        let full = self.cell_rect(cursor.col, cursor.row);
        // 先取出来,后面画反色文字还要用 &mut self
        let st = self.cursor_style.clone();
        let phase = self.cursor_blink_phase.clamp(0.0, 1.0);

        // 形状:配置覆盖优先,否则跟终端 DECSCUSR
        let kind = match st.shape {
            CursorShapeOverride::Block => Kind::Block,
            CursorShapeOverride::Bar => Kind::Bar,
            CursorShapeOverride::Underline => Kind::Underline,
            CursorShapeOverride::Auto => match cursor.shape {
                CursorShape::BlinkingUnderline | CursorShape::SteadyUnderline => Kind::Underline,
                CursorShape::BlinkingBar | CursorShape::SteadyBar => Kind::Bar,
                _ => Kind::Block,
            },
        };
        let declared_blink = matches!(
            cursor.shape,
            CursorShape::BlinkingBlock
                | CursorShape::BlinkingUnderline
                | CursorShape::BlinkingBar
        );
        let blink_active = match st.blink {
            CursorBlinkMode::Never => false,
            CursorBlinkMode::Always => true,
            CursorBlinkMode::Auto => declared_blink,
        };
        let lit = phase < st.blink_duty;
        let mut alpha = st.alpha;
        let mut outline = false;
        if blink_active {
            match st.blink_style {
                // 灭相整块消失(传统终端)
                CursorBlinkStyle::Hidden => {
                    if !lit {
                        return;
                    }
                }
                // 灭相淡出到 8% 再淡回来;两端连续,不会跳变
                CursorBlinkStyle::Fade => {
                    if !lit {
                        let span = (1.0 - st.blink_duty).max(0.01);
                        let u = ((phase - st.blink_duty) / span).clamp(0.0, 1.0);
                        alpha *= 1.0 - 0.92 * (u * std::f32::consts::PI).sin();
                    }
                }
                // 灭相只留一圈描边
                CursorBlinkStyle::Outline => {
                    if !lit {
                        outline = true;
                    }
                }
            }
        }

        // 常驻动画:只改尺寸 / 透明度,不改光标所在格,尾迹和 IME 定位不受影响
        let mut scale = 1.0f32;
        if st.anim != CursorAnimation::None {
            let cycles = (self.cursor_anim_t * 1000.0 / (st.anim_ms as f64).max(1.0)) as f32;
            let wave = 0.5 - 0.5 * (cycles * std::f32::consts::TAU).cos();
            match st.anim {
                CursorAnimation::Pulse => scale = 1.0 + st.anim_amount * (2.0 * wave - 1.0),
                CursorAnimation::Breathe => alpha *= 1.0 - st.anim_amount * wave,
                CursorAnimation::None => {}
            }
        }
        if alpha <= 0.002 {
            return;
        }

        // width / height 是「相对默认形状」的倍数:1.0 就是旧版的整格方块、
        // 1/6 格宽的竖条、1/8 格高的下划线。thickness 可把后两者的粗细钉成像素。
        let (cw0, ch0) = (full.w as f32, full.h as f32);
        let (cw, ch) = match kind {
            Kind::Block => (cw0 * st.width, ch0 * st.height),
            Kind::Bar => {
                let t = if st.thickness > 0.0 {
                    st.thickness
                } else {
                    (cw0 / 6.0).ceil().max(2.0)
                };
                (t * st.width, ch0 * st.height)
            }
            Kind::Underline => {
                let t = if st.thickness > 0.0 {
                    st.thickness
                } else {
                    (ch0 / 8.0).ceil().max(2.0)
                };
                (cw0 * st.width, t * st.height)
            }
        };
        let cw = ((cw * scale).max(1.0)) as f64;
        let ch = ((ch * scale).max(1.0)) as f64;
        // 默认锚点:block/bar 贴左上,underline 贴底 —— 与旧版一致
        let ax = st.align_x.unwrap_or(0.0) as f64;
        let ay = st
            .align_y
            .unwrap_or(if kind == Kind::Underline { 1.0 } else { 0.0 }) as f64;
        let x = full.x + (full.w - cw) * ax;
        let y = full.y + (full.h - ch) * ay;

        let color = palette.cursor_bg;
        if outline {
            let t = 1.0f64.min((cw / 4.0).min(ch / 4.0)).max(1.0);
            frame.rect(x, y, cw, t, color, alpha);
            frame.rect(x, y + ch - t, cw, t, color, alpha);
            frame.rect(x, y + t, t, ch - 2.0 * t, color, alpha);
            frame.rect(x + cw - t, y + t, t, ch - 2.0 * t, color, alpha);
            return;
        }

        frame.rect(x, y, cw, ch, color, alpha);
        // 方块要把底下的字反色画回来;光标缩得太小 / 正在淡出时就不画了,
        // 免得字跑到光标外面。文字仍按原格子基线摆,不会因为缩放而抖动。
        if kind == Kind::Block
            && alpha > 0.75
            && st.width >= 0.4
            && !cell.text.is_empty()
            && !cell.invisible
        {
            self.draw_text(
                frame,
                &cell.text,
                full.x,
                full.y + self.ascent,
                palette.cursor_fg,
                StyleBits {
                    bold: cell.bold,
                    italic: cell.italic,
                },
                cell.span,
            );
        }
    }
}
fn rasterize_in(
    family: &FamilySlot,
    ch: char,
    style: StyleBits,
    factor: f32,
) -> Option<CachedGlyph> {
    let (slot, synthetic_bold) = family.pick(style);
    let font = &slot.font;
    let id = font.glyph_id(ch);
    if id.0 == 0 {
        return None;
    }
    let scale = slot.at(factor);
    let glyph = Glyph {
        id,
        scale,
        position: ab_glyph::point(0.0, 0.0),
    };
    let og = font.outline_glyph(glyph)?;
    let b = og.px_bounds();
    let w = (b.width().ceil() as u32).max(1);
    let h = (b.height().ceil() as u32).max(1);
    if !b.width().is_finite() || !b.height().is_finite() || b.width() <= 0.0 || b.height() <= 0.0 {
        return None;
    }
    let mut data = vec![0u8; w as usize * h as usize];
    og.draw(|x, y, v| {
        let idx = y as usize * w as usize + x as usize;
        if idx < data.len() {
            data[idx] = (v * 255.0).round() as u8;
        }
    });
    // 步进取字体自身的 advance(不受 px_bounds 左右留白影响)
    let sf = font.as_scaled(scale);
    let advance = sf.h_advance(id);
    Some(CachedGlyph {
        w,
        h,
        left: b.min.x as i32,
        top: b.min.y as i32,
        advance,
        ascent: sf.ascent(),
        descent: sf.descent(),
        synthetic_bold,
        data,
    })
}

/// 把缓存的字形位图按 alpha 混合到画布上。
///
/// `src` 是源位图内的矩形(x, y, w, h),`dst` 是画布上的目标矩形;两者尺寸
/// 不同时做**盒式重采样**(面积平均),这样缩小图标时不会像最近邻那样把细节
/// 压成色块。`synthetic_bold` 为真时再向右描一遍(没有 Bold face 的字体)。
fn blit_glyph(
    canvas: &mut Canvas,
    g: &CachedGlyph,
    src: (f64, f64, f64, f64),
    dst: (f64, f64, f64, f64),
    color: Rgb,
    italic: bool,
    synthetic_bold: bool,
) {
    let (sx0, sy0, sw, sh) = src;
    if sw <= 0.5 || sh <= 0.5 || dst.2 <= 0.0 || dst.3 <= 0.0 {
        return;
    }
    let x0 = dst.0.floor();
    let y0 = dst.1.floor();
    let dest_w = ((dst.0 + dst.2).ceil() - x0).max(1.0);
    let dest_h = ((dst.1 + dst.3).ceil() - y0).max(1.0);
    // 1:1(尺寸一致)时直接取源像素,保持锐利。
    // 注意只看尺寸:位置带小数(基线在格子里通常是 14.85 这种)也要走快路径,
    // 否则每个字形都会掉进逐像素除法的慢路径,整屏慢一倍以上。
    let identity = (sw - dst.2).abs() < 0.01 && (sh - dst.3).abs() < 0.01;
    let passes = if synthetic_bold { 2 } else { 1 };
    for py in 0..dest_h as i64 {
        // 没有专用斜体字形时用剪切变形合成(越靠上偏移越大,约 12°)
        let shear = if italic {
            ((dest_h - py as f64) * 0.22).round()
        } else {
            0.0
        };
        let dy = (y0 + py as f64) as i32;
        let (ry0, ry1) = if identity {
            let v = (sy0 + py as f64) as u32;
            (v, (v + 1).min(g.h))
        } else {
            row_span(sy0, sh, dest_h, py as f64, g.h)
        };
        if ry1 <= ry0 {
            continue;
        }
        for px in 0..dest_w as i64 {
            let a = if identity {
                let u = (sx0 + px as f64) as u32;
                if u >= g.w {
                    continue;
                }
                g.data[(ry0 * g.w + u) as usize]
            } else {
                let (rx0, rx1) = row_span(sx0, sw, dest_w, px as f64, g.w);
                if rx1 <= rx0 {
                    continue;
                }
                let mut sum = 0u32;
                for yy in ry0..ry1 {
                    let base = yy * g.w;
                    for xx in rx0..rx1 {
                        sum += g.data[(base + xx) as usize] as u32;
                    }
                }
                (sum / ((rx1 - rx0) * (ry1 - ry0)).max(1)) as u8
            };
            if a != 0 {
                for pass in 0..passes {
                    canvas.blend(
                        (x0 + px as f64 + shear + pass as f64) as i32,
                        dy,
                        color,
                        a as f32 / 255.0,
                    );
                }
            }
        }
    }
}

/// 目标像素 `i`(0-based)覆盖的源像素区间 `[start, end)`。
/// `off`/`len` 是源矩形在源位图里的起点与长度,`dest_n` 是目标像素数,
/// `limit` 是源位图该方向的像素数。
fn row_span(off: f64, len: f64, dest_n: f64, i: f64, limit: u32) -> (u32, u32) {
    let u0 = off + i * len / dest_n;
    let u1 = off + (i + 1.0) * len / dest_n;
    let s = (u0.floor().max(0.0) as u32).min(limit);
    let e = ((u1.ceil().max(u0.floor() + 1.0)) as u32).min(limit);
    (s, e.max(s))
}

/// 按 SGR 4:x 画不同形态的下划线(单/双/波浪/点/虚线)
fn push_underline(
    frame: &mut Frame,
    x: f64,
    y: f64,
    w: f64,
    style: Underline,
    color: Rgb,
    th: f64,
) {
    match style {
        Underline::None => {}
        Underline::Single => frame.rect(x, y, w, th, color, 1.0),
        Underline::Double => {
            frame.rect(x, y, w, th, color, 1.0);
            frame.rect(x, y + th + 1.0, w, th, color, 1.0);
        }
        Underline::Curly => {
            let mut i = 0.0;
            while i < w {
                let dy = (i * 0.55).sin() * 1.4;
                frame.rect(x + i, y + dy, 1.0, th, color, 1.0);
                i += 1.0;
            }
        }
        Underline::Dotted => {
            let mut i = 0.0;
            while i < w {
                frame.rect(x + i, y, 1.0, th, color, 1.0);
                i += 3.0;
            }
        }
        Underline::Dashed => {
            let mut i = 0.0;
            while i < w {
                frame.rect(x + i, y, 3.0, th, color, 1.0);
                i += 5.0;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 像素画布:softbuffer u32 = R<<16 | G<<8 | B
// ---------------------------------------------------------------------------

struct Canvas<'a> {
    buf: &'a mut [u32],
    w: u32,
    h: u32,
    /// 内容区上边界(像素):padding 区永远是背景色,亚行滚动时上移的内容
    /// 必须在 pad_y 处裁掉,不能渗进 padding
    clip_top: i32,
    /// 内容区下边界(像素):标签栏在下方时,图片这类越过网格的元素在这里收边
    clip_bottom: i32,
    /// 横向边界(像素):`Frame::clip_x` 生效时夹住左右,侧边栏滑入/滑出
    /// 时它的绘制矩形会越出自己的区域,靠这两条挡住。
    clip_l: i32,
    clip_r: i32,
}

impl Canvas<'_> {
    fn fill_all(&mut self, c: Rgb) {
        let p = pack(c);
        for x in self.buf.iter_mut() {
            *x = p;
        }
    }

    fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64, c: Rgb, alpha: f32) {
        let x0 = (x.floor() as i32).max(self.clip_l);
        let y0 = (y.floor() as i32).max(self.clip_top);
        let x1 = ((x + w).ceil() as i32).min(self.clip_r);
        let y1 = ((y + h).ceil() as i32).min(self.h as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        if alpha >= 1.0 {
            let p = pack(c);
            for yy in y0..y1 {
                let row = yy as usize * self.w as usize;
                for xx in x0..x1 {
                    self.buf[row + xx as usize] = p;
                }
            }
        } else {
            for yy in y0..y1 {
                for xx in x0..x1 {
                    self.blend(xx, yy, c, alpha);
                }
            }
        }
    }

    /// 圆角矩形:按像素中心到圆角矩形边界的距离算覆盖率,边缘 1px 抗锯齿。
    /// 标签栏只有几十像素高,逐像素算距离完全够用。
    fn fill_round_rect(&mut self, x: f64, y: f64, w: f64, h: f64, radius: f64, c: Rgb, alpha: f32) {
        let r = radius.min(w.min(h) * 0.5);
        if r < 0.5 {
            return self.fill_rect(x, y, w, h, c, alpha);
        }
        let x0 = (x.floor() as i32).max(self.clip_l);
        let y0 = (y.floor() as i32).max(self.clip_top);
        let x1 = ((x + w).ceil() as i32).min(self.clip_r);
        let y1 = ((y + h).ceil() as i32).min(self.h as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        // 四角圆心连线围成的内矩形:它内部覆盖率恒为 1,只有外面才需要开方
        let (ix0, iy0) = (x as f32 + r as f32, y as f32 + r as f32);
        let (ix1, iy1) = ((x + w) as f32 - r as f32, (y + h) as f32 - r as f32);
        let p = pack(c);
        for py in y0..y1 {
            let cy = py as f32 + 0.5;
            let row = py as usize * self.w as usize;
            for px in x0..x1 {
                let cx = px as f32 + 0.5;
                let dx = (ix0 - cx).max(cx - ix1).max(0.0);
                let dy = (iy0 - cy).max(cy - iy1).max(0.0);
                if dx <= 0.0 && dy <= 0.0 {
                    // 内部:直接写入(alpha 拉满时)或整体混合
                    if alpha >= 1.0 {
                        self.buf[row + px as usize] = p;
                    } else {
                        self.blend(px, py, c, alpha);
                    }
                    continue;
                }
                let d = (dx * dx + dy * dy).sqrt() - r as f32;
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(px, py, c, alpha * cov);
                }
            }
        }
    }

    /// 缩放贴图:目标像素中心反查源图坐标,双线性采样后按 alpha 混合。
    ///
    /// 只遍历目标矩形覆盖的像素(而不是源图),所以把 4K 图缩成 200px 宽
    /// 也只花 200×150 次采样。边缘用 clamp-to-edge,不会出现黑边。
    fn blit_image(&mut self, dst: [f32; 4], img: &crate::images::DecodedImage) {
        self.blit_image_with(dst, img, 1.0);
    }

    /// 同 `blit_image`,额外乘一个整体透明度(背景层用)。
    fn blit_image_with(&mut self, dst: [f32; 4], img: &crate::images::DecodedImage, alpha: f32) {
        let (dw, dh) = (dst[2], dst[3]);
        if dw <= 0.0 || dh <= 0.0 || img.width == 0 || img.height == 0 {
            return;
        }
        let x0 = (dst[0].floor() as i32).max(self.clip_l);
        let y0 = (dst[1].floor() as i32).max(self.clip_top);
        let x1 = ((dst[0] + dw).ceil() as i32).min(self.clip_r);
        let y1 = ((dst[1] + dh).ceil() as i32)
            .min(self.h as i32)
            .min(self.clip_bottom);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let iw = img.width as usize;
        let ih = img.height as usize;
        let px = img.pixels.as_slice();
        let step_x = img.width as f32 / dw;
        let step_y = img.height as f32 / dh;
        for py in y0..y1 {
            let sy = ((py as f32 + 0.5 - dst[1]) * step_y - 0.5).max(0.0);
            let iy = sy.floor();
            let fy = sy - iy;
            let r0 = (iy as usize).min(ih - 1) * iw * 4;
            let r1 = ((iy as usize) + 1).min(ih - 1) * iw * 4;
            for pxx in x0..x1 {
                let sx = ((pxx as f32 + 0.5 - dst[0]) * step_x - 0.5).max(0.0);
                let ix = sx.floor();
                let fx = sx - ix;
                let c0 = (ix as usize).min(iw - 1) * 4;
                let c1 = ((ix as usize) + 1).min(iw - 1) * 4;
                let (p00, p10) = (r0 + c0, r0 + c1);
                let (p01, p11) = (r1 + c0, r1 + c1);
                let mix = |i: usize| -> f32 {
                    let top = px[p00 + i] as f32 * (1.0 - fx) + px[p10 + i] as f32 * fx;
                    let bot = px[p01 + i] as f32 * (1.0 - fx) + px[p11 + i] as f32 * fx;
                    top * (1.0 - fy) + bot * fy
                };
                let a = mix(3);
                if a < 1.0 {
                    continue;
                }
                let c = Rgb {
                    r: mix(0).round() as u8,
                    g: mix(1).round() as u8,
                    b: mix(2).round() as u8,
                };
                self.blend(pxx, py, c, (a / 255.0).clamp(0.0, 1.0) * alpha);
            }
        }
    }

    fn blend(&mut self, x: i32, y: i32, c: Rgb, alpha: f32) {
        // 字形、半透明矩形、尾迹都汇到这一个入口,横向边界挡在这里最省事
        if alpha <= 0.0
            || x < self.clip_l
            || y < self.clip_top
            || x >= self.clip_r
            || y >= self.clip_bottom
        {
            return;
        }
        let idx = y as usize * self.w as usize + x as usize;
        let d = self.buf[idx];
        let dr = (d >> 16) & 0xff;
        let dg = (d >> 8) & 0xff;
        let db = d & 0xff;
        let mix = |s: u8, dch: u32| -> u32 {
            (s as f32 * alpha + dch as f32 * (1.0 - alpha)).round() as u32
        };
        self.buf[idx] = (mix(c.r, dr) << 16) | (mix(c.g, dg) << 8) | mix(c.b, db);
    }

    /// 填充任意四边形(kitty 尾迹):逐像素内外测试 + 常量透明度;
    /// `mask` 是"挖空"区域(当前光标矩形),其中的像素不绘制。
    fn fill_quad(
        &mut self,
        corners: &[Point2; 4],
        color: Rgb,
        alpha: f32,
        mask: (f32, f32, f32, f32),
    ) {
        if alpha <= 0.0 {
            return;
        }
        let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
        let (mut min_y, mut max_y) = (f32::MAX, f32::MIN);
        for p in corners {
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
            min_y = min_y.min(p.y);
            max_y = max_y.max(p.y);
        }
        let x0 = (min_x.floor() as i32).max(self.clip_l);
        let y0 = (min_y.floor() as i32).max(self.clip_top);
        let x1 = (max_x.ceil() as i32 + 1).min(self.clip_r);
        let y1 = (max_y.ceil() as i32 + 1).min(self.h as i32);
        for py in y0..y1 {
            for px in x0..x1 {
                let cx = px as f32 + 0.5;
                let cy = py as f32 + 0.5;
                if cx >= mask.0 && cx < mask.2 && cy >= mask.1 && cy < mask.3 {
                    continue;
                }
                if point_in_quad(cx, cy, corners) {
                    self.blend(px, py, color, alpha);
                }
            }
        }
    }
}

fn pack(c: Rgb) -> u32 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

/// 由"一个格子 + 它记录的纹理坐标"反推整张图的位置与尺寸。
///
/// 图片被切成每格一块,格子的 uv 跨度正好是 `cell_w / 图宽`;所以整图宽度
/// 是 `cell_w / du`,左上角则是"本格位置 - 左上角 uv × 整图尺寸"。这样即使
/// 图片上沿已经滚出视口,从任意一个可见格子出发都能算出正确的整图矩形。
///
/// `term_cell` 是终端假定的格子像素尺寸(见 `ScreenSnapshot::term_cell_px`):
/// wezterm 的 padding 用的是这个坐标系,渲染器按字体度量出来的格子未必相同,
/// 所以要先把 padding 按比例换成渲染坐标,否则"图比一格还小"时(比如 2x2 px
/// 的 Kitty 图)会被误判成"本格完全没有被图覆盖"而整张丢掉。
fn image_quad(
    cell_x: f64,
    cell_y: f64,
    cell_w: f64,
    cell_h: f64,
    im: &SnapImage,
    term_cell: (f64, f64),
) -> Option<[f32; 4]> {
    let (u0, v0) = im.tl;
    let (u1, v1) = im.br;
    let du = u1 - u0;
    let dv = v1 - v0;
    if du <= 1e-6 || dv <= 1e-6 {
        return None;
    }
    // 终端格子 -> 渲染格子 的比例;未知时按 1:1 处理
    let sx = if term_cell.0 > 0.1 {
        cell_w / term_cell.0
    } else {
        1.0
    };
    let sy = if term_cell.1 > 0.1 {
        cell_h / term_cell.1
    } else {
        1.0
    };
    // 只有整格部分被图覆盖:padding 之外的那部分不属于这张图。
    // 图片高度不是格高整数倍时,最后一行是残格,必须按"实际覆盖的像素"来算,
    // 否则会把残格当成整格,算出双倍尺寸并向上溢出。
    let pl = im.pad.0 as f64 * sx;
    let pt = im.pad.1 as f64 * sy;
    let covered_w = cell_w - pl - im.pad.2 as f64 * sx;
    let covered_h = cell_h - pt - im.pad.3 as f64 * sy;
    if covered_w <= 0.0 || covered_h <= 0.0 {
        return None;
    }
    let w = covered_w / du as f64;
    let h = covered_h / dv as f64;
    if !w.is_finite() || !h.is_finite() || w <= 0.5 || h <= 0.5 {
        return None;
    }
    let x = cell_x + pl - u0 as f64 * w;
    let y = cell_y + pt - v0 as f64 * h;
    Some([x as f32, y as f32, w as f32, h as f32])
}

fn point_in_tri(px: f32, py: f32, a: &Point2, b: &Point2, c: &Point2) -> bool {
    let d1 = (px - b.x) * (a.y - b.y) - (a.x - b.x) * (py - b.y);
    let d2 = (px - c.x) * (b.y - c.y) - (b.x - c.x) * (py - c.y);
    let d3 = (px - a.x) * (c.y - a.y) - (c.x - a.x) * (py - a.y);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

fn point_in_quad(px: f32, py: f32, q: &[Point2; 4]) -> bool {
    point_in_tri(px, py, &q[0], &q[1], &q[2]) || point_in_tri(px, py, &q[0], &q[2], &q[3])
}

// ---------------------------------------------------------------------------
// 单元测试:验证 SGR 属性解析与回看偏移对快照的影响
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AppConfig, CursorAnimation, CursorBlinkMode, CursorBlinkStyle, CursorShapeOverride,
    };
    use wezterm_term::TerminalSize;

    // --- 光标本体:形状 / 尺寸 / 闪烁 / 动画 ---------------------------
    /// 这一帧里所有「光标色」的实心矩形。终端内容为空,不会混进文字笔画。
    fn caret_rects(
        r: &mut Renderer,
        snap: &ScreenSnapshot,
        cur: &CursorInfo,
        pal: &Palette,
    ) -> Vec<(f32, f32, f32, f32, f32)> {
        let cols = snap.rows.first().map(|row| row.len()).unwrap_or(10);
        let w = (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32;
        let h = (snap.rows.len() as f64 * r.cell_h + 2.0 * r.pad_y) as u32;
        let mut frame = Frame::default();
        r.build_frame(&mut frame, w, h, snap, cur, pal, None, pal.cursor_bg, true);
        frame
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Rect {
                    x,
                    y,
                    w,
                    h,
                    color,
                    alpha,
                } if *color == pal.cursor_bg => Some((*x, *y, *w, *h, *alpha)),
                _ => None,
            })
            .collect()
    }

    fn caret_setup() -> (Renderer, ScreenSnapshot, CursorInfo, Palette) {
        let t = term_with("", 3, 10);
        let (snap, cur, pal) = snapshot(&t, 10, 0.0);
        let r = Renderer::new(&AppConfig::default(), 1.0).unwrap();
        (r, snap, cur, pal)
    }

    #[test]
    fn cursor_visual_defaults_draw_a_full_cell_block() {
        let (mut r, snap, cur, pal) = caret_setup();
        let cell = r.cell_rect(cur.col, cur.row);
        let rects = caret_rects(&mut r, &snap, &cur, &pal);
        assert_eq!(rects.len(), 1, "默认就是一个整格方块");
        let (x, y, w, h, a) = rects[0];
        assert!((x - cell.x as f32).abs() < 0.01);
        assert!((y - cell.y as f32).abs() < 0.01);
        assert!((w - cell.w as f32).abs() < 0.01);
        assert!((h - cell.h as f32).abs() < 0.01);
        assert_eq!(a, 1.0);
    }

    #[test]
    fn cursor_width_height_and_align_move_the_block() {
        let (mut r, snap, cur, pal) = caret_setup();
        let cell = r.cell_rect(cur.col, cur.row);
        r.cursor_style.width = 0.5;
        r.cursor_style.height = 0.4;
        r.cursor_style.align_x = Some(0.5);
        r.cursor_style.align_y = Some(1.0);
        let (x, y, w, h, _) = caret_rects(&mut r, &snap, &cur, &pal)[0];
        assert!((w - cell.w as f32 * 0.5).abs() < 0.01);
        assert!((h - cell.h as f32 * 0.4).abs() < 0.01);
        assert!((x - (cell.x + (cell.w - cell.w * 0.5) * 0.5) as f32).abs() < 0.01);
        assert!((y - (cell.y + cell.h - cell.h * 0.4) as f32).abs() < 0.01);
    }

    #[test]
    fn cursor_shape_override_and_thickness() {
        let (mut r, snap, cur, pal) = caret_setup();
        let cell = r.cell_rect(cur.col, cur.row);
        r.cursor_style.shape = CursorShapeOverride::Bar;
        r.cursor_style.thickness = 3.0;
        r.cursor_style.width = 2.0;
        let bar = caret_rects(&mut r, &snap, &cur, &pal);
        assert_eq!(bar.len(), 1);
        assert!((bar[0].2 - 6.0).abs() < 0.01, "3px × width 2 = 6px");
        assert!((bar[0].3 - cell.h as f32).abs() < 0.01);

        // underline 不写 align 时仍贴格子底部,和旧版一致
        r.cursor_style.shape = CursorShapeOverride::Underline;
        r.cursor_style.thickness = 4.0;
        r.cursor_style.width = 1.0;
        let line = caret_rects(&mut r, &snap, &cur, &pal);
        assert_eq!(line.len(), 1);
        assert!((line[0].3 - 4.0).abs() < 0.01);
        assert!((line[0].1 - (cell.y + cell.h - 4.0) as f32).abs() < 0.01);
    }

    #[test]
    fn cursor_blink_hidden_fade_outline() {
        let (mut r, snap, cur, pal) = caret_setup();
        r.cursor_style.blink = CursorBlinkMode::Always;
        r.cursor_blink_phase = 0.8; // duty = 0.5 → 灭相
        assert!(caret_rects(&mut r, &snap, &cur, &pal).is_empty(), "hidden 灭相不画");

        r.cursor_style.blink_style = CursorBlinkStyle::Fade;
        let faded = caret_rects(&mut r, &snap, &cur, &pal);
        assert_eq!(faded.len(), 1);
        assert!(faded[0].4 < 1.0 && faded[0].4 > 0.0, "fade 是半透明而不是消失");

        r.cursor_style.blink_style = CursorBlinkStyle::Outline;
        assert_eq!(caret_rects(&mut r, &snap, &cur, &pal).len(), 4, "描边四条边");

        r.cursor_style.blink_style = CursorBlinkStyle::Hidden;
        r.cursor_blink_phase = 0.1; // 亮相
        assert_eq!(caret_rects(&mut r, &snap, &cur, &pal).len(), 1);
    }

    #[test]
    fn cursor_blink_auto_follows_decscusr() {
        let (mut r, snap, cur, pal) = caret_setup();
        r.cursor_blink_phase = 0.8;
        let steady = CursorInfo {
            col: cur.col,
            row: cur.row,
            shape: CursorShape::SteadyBlock,
            visible: true,
        };
        let blinking = CursorInfo {
            shape: CursorShape::BlinkingBlock,
            ..steady
        };
        assert_eq!(caret_rects(&mut r, &snap, &steady, &pal).len(), 1);
        assert!(caret_rects(&mut r, &snap, &blinking, &pal).is_empty());
    }

    #[test]
    fn cursor_pulse_scales_and_breathe_dims() {
        let (mut r, snap, cur, pal) = caret_setup();
        let cell = r.cell_rect(cur.col, cur.row);
        r.cursor_style.anim = CursorAnimation::Pulse;
        r.cursor_style.anim_ms = 1200.0;
        r.cursor_style.anim_amount = 0.5;
        r.cursor_anim_t = 0.6; // 半个周期 → 放大到 1.5 倍
        let big = caret_rects(&mut r, &snap, &cur, &pal);
        assert!((big[0].2 - cell.w as f32 * 1.5).abs() < 0.01);
        assert_eq!(big[0].4, 1.0, "pulse 不改透明度");

        r.cursor_style.anim = CursorAnimation::Breathe;
        let dim = caret_rects(&mut r, &snap, &cur, &pal);
        assert!((dim[0].2 - cell.w as f32).abs() < 0.01, "breathe 不改尺寸");
        assert!((dim[0].4 - 0.5).abs() < 0.01, "breathe 半周期降到 50%");
    }

    /// 圆角矩形光栅化:角落是底色、中心是填充色、圆弧上有一圈过渡
    #[test]
    fn round_rect_rasterizes_corners() {
        let (w, h) = (20u32, 20u32);
        let clear = Rgb { r: 0, g: 0, b: 0 };
        let fill = Rgb { r: 200, g: 0, b: 0 };
        let mut buf = vec![0u32; (w * h) as usize];
        {
            let mut c = Canvas {
                buf: &mut buf,
                w,
                h,
                clip_top: 0,
                clip_bottom: h as i32,
                clip_l: 0,
                clip_r: w as i32,
            };
            c.fill_all(clear);
            c.fill_round_rect(0.0, 0.0, 20.0, 20.0, 6.0, fill, 1.0);
        }
        let at = |x: u32, y: u32| buf[(y * w + x) as usize];
        assert_eq!(at(0, 0), pack(clear), "圆角外侧不能被填上");
        assert_eq!(at(19, 19), pack(clear));
        assert_eq!(at(10, 10), pack(fill), "内部必须填满");
        assert_eq!(at(0, 10), pack(fill), "直边中段必须填满");
        // 45° 方向上正好压着圆弧的像素,覆盖率介于 0 和 1 之间
        let edge = at(1, 1);
        assert!(
            edge != pack(clear) && edge != pack(fill),
            "圆弧边缘应该抗锯齿过渡:{edge:#08x}"
        );
    }

    /// 尾迹四边形的挖空区约定:mask 是 **[x0, y0, x1, y1]**(两个后端一致)。
    /// 这里把它钉死 —— 曾经跨窗格尾迹照抄了 `corners` 的 x0/x1/y0/y1 顺序,
    /// 结果整块四边形被当成挖空区,一像素都没画出来,而且完全静默。
    #[test]
    fn trail_quad_paints_but_masks_the_cursor_cell() {
        let (w, h) = (60u32, 40u32);
        let clear = Rgb { r: 0, g: 0, b: 0 };
        let fill = Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
        let mut buf = vec![0u32; (w * h) as usize];
        {
            let mut c = Canvas {
                buf: &mut buf,
                w,
                h,
                clip_top: 0,
                clip_bottom: h as i32,
                clip_l: 0,
                clip_r: w as i32,
            };
            c.fill_all(clear);
            // 一条 40x10 的横带,从左往右收拢;光标格落在中间 (20,10)-(30,20)
            let corners = [
                Point2 { x: 40.0, y: 10.0 }, // 右上
                Point2 { x: 40.0, y: 20.0 }, // 右下
                Point2 { x: 10.0, y: 20.0 }, // 左下
                Point2 { x: 10.0, y: 10.0 }, // 左上
            ];
            // 注意顺序:x0, y0, x1, y1
            c.fill_quad(&corners, fill, 1.0, (20.0, 10.0, 30.0, 20.0));
        }
        let at = |x: u32, y: u32| buf[(y * w + x) as usize];
        assert_eq!(at(15, 15), pack(fill), "挖空区左侧必须被填上");
        assert_eq!(at(35, 15), pack(fill), "挖空区右侧必须被填上");
        assert_eq!(at(25, 15), pack(clear), "光标格应当被挖空");
        assert_eq!(at(25, 5), pack(clear), "四边形之外不能画");
        assert_eq!(at(5, 15), pack(clear), "四边形之外不能画");
    }

    /// 诊断:每个字符到底由哪个字体家族提供,墨迹/步进是多少。
    /// `cargo test glyph_sources -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn glyph_sources() {
        let mut cfg = AppConfig::default();
        cfg.fonts.size = 20.0;
        let mut r = Renderer::new(&cfg, 1.0).unwrap();
        r.ensure_icons();
        crate::wlog!(
            "cell={}x{} ascent={} em={} families={:?} icons={:?} mode={:?}",
            r.cell_w,
            r.cell_h,
            r.ascent,
            r.em_px,
            r.families
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            r.icon_families
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            r.icon_mode
        );
        let probe = [
            'M',
            '中',
            '\u{e0b0}',
            '\u{e0b2}',
            '\u{e0a0}',
            '\u{f015}',
            '\u{f121}',
            '\u{f00c}',
            '\u{f017}',
            '\u{e725}',
            '\u{e718}',
            '\u{f0001}',
        ];
        for ch in probe {
            let mut found = String::new();
            for fam in r.families.iter().chain(r.icon_families.iter()) {
                if let Some(g) = rasterize_in(fam, ch, StyleBits::default(), 1.0) {
                    found.push_str(&format!(
                        "\n    {:28} ink {}x{} left={} top={} adv={:.2}",
                        fam.name, g.w, g.h, g.left, g.top, g.advance
                    ));
                }
            }
            crate::wlog!("U+{:04X} {}{}", ch as u32, ch, found);
        }
    }

    fn term_with(input: &str, rows: usize, cols: usize) -> Terminal {
        let cfg = AppConfig::default();
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, &cfg, Box::new(std::io::sink()));
        t.advance_bytes(input.as_bytes());
        t
    }

    fn row_text(row: &[SnapCell]) -> String {
        row.iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn sgr_attributes_are_captured() {
        // 斜体 / 慢闪 / 上划线 / 花下划线 / 删除线
        let t = term_with(
            "\x1b[3mA\x1b[0m\x1b[5mB\x1b[0m\x1b[53mC\x1b[0m\x1b[4:3mD\x1b[0m\x1b[9mE",
            4,
            20,
        );
        let (snap, _, _) = snapshot(&t, 20, 0.0);
        let row = &snap.rows[0];
        assert!(row[0].italic, "SGR 3 应为斜体");
        assert!(row[1].blink, "SGR 5 应为闪烁");
        assert!(snap.has_blink, "有闪烁单元格时应置位 has_blink");
        assert!(row[2].overline, "SGR 53 应为上划线");
        assert_eq!(row[3].underline, Underline::Curly, "SGR 4:3 应为花下划线");
        assert!(row[4].strike, "SGR 9 应为删除线");
    }

    #[test]
    fn scroll_offset_moves_view_back() {
        let mut input = String::new();
        for i in 0..14 {
            input.push_str(&format!("L{i}\r\n"));
        }
        let t = term_with(&input, 5, 10);

        let (bottom, _, _) = snapshot(&t, 10, 0.0);
        let (back, _, _) = snapshot(&t, 10, 3.0);
        assert_ne!(
            row_text(&bottom.rows[0]),
            row_text(&back.rows[0]),
            "回看偏移应改变首行内容"
        );
        assert!(
            row_text(&back.rows[0]).starts_with("L"),
            "回看 3 行后首行应是更早的输入行,实际为 {:?}",
            row_text(&back.rows[0])
        );
    }

    /// 回归:powerline 箭头必须严格待在格子里(上下不串行),
    /// 且横向要几乎铺满 —— 否则 Oh My Posh 的色块之间会出现深色竖缝。
    #[test]
    fn powerline_glyph_stays_inside_cell() {
        let cfg = AppConfig::default();
        let t = term_with("\u{e0b0}\u{e0b2}", 4, 12);
        let (snap, cursor, palette) = snapshot(&t, 12, 0.0);
        let mut r = Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = (
            (12.0 * r.cell_w + 2.0 * r.pad_x) as u32,
            (4.0 * r.cell_h + 2.0 * r.pad_y) as u32,
        );
        let mut buf = vec![0u32; (w * h) as usize];
        let hidden = CursorInfo {
            col: cursor.col,
            row: cursor.row,
            shape: cursor.shape,
            visible: false,
        };
        r.paint(
            &mut buf,
            w,
            h,
            &snap,
            &hidden,
            &palette,
            None,
            palette.cursor_bg,
            true,
        );

        let bg = pack(palette.bg);
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (u32::MAX, 0u32, u32::MAX, 0u32);
        for y in 0..h {
            for x in 0..w {
                let p = buf[(y * w + x) as usize];
                if p != bg {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        let top = r.pad_y as u32;
        let bottom = (r.pad_y + r.cell_h) as u32;
        assert!(
            min_y >= top && max_y < bottom + 1,
            "箭头溢出了首行格子:y {min_y}..{max_y},格子 {top}..{bottom}"
        );
        // 两个箭头横向总共应铺满 2 格(允许各留 1px 抗锯齿)
        let filled = (max_x - min_x) as f64;
        assert!(
            filled >= 2.0 * r.cell_w - 3.0,
            "箭头没有填满横向格子:实际 {filled}px,期望 ≥ {}",
            2.0 * r.cell_w - 3.0
        );
    }

    /// 亚行滚动:整数部分取视图,小数部分交给渲染整体位移。
    /// 2.5 行必须等价于"3 行的视图 + 半行偏移",不能出现空白行或抖动的接缝。
    #[test]
    fn fractional_scroll_shifts_by_sub_line() {
        let mut input = String::new();
        for i in 0..14 {
            input.push_str(&format!("L{i}\r\n"));
        }
        let t = term_with(&input, 5, 10);

        let (exact, _, _) = snapshot(&t, 10, 3.0);
        let (half, _, _) = snapshot(&t, 10, 2.5);
        assert_eq!(exact.scroll_frac, 0.0, "整数位置不应有亚行偏移");
        assert!(
            (half.scroll_frac - 0.5).abs() < 1e-3,
            "2.5 行应取 3 行的视图上移 0.5 行,实际 {}",
            half.scroll_frac
        );
        assert_eq!(
            half.rows.len(),
            exact.rows.len() + 1,
            "亚行滚动要多取一行垫住底部"
        );
        for r in 0..exact.rows.len() {
            assert_eq!(
                row_text(&half.rows[r]),
                row_text(&exact.rows[r]),
                "第 {r} 行内容应与整数视图一致(只是整体位移)"
            );
        }
    }

    #[test]
    fn palette_256_and_truecolor() {
        // SGR 38;5;196 = 216 色立方里的纯红;38;2 / 48;2 = 真彩色直通
        let t = term_with(
            "\x1b[38;5;196mA\x1b[38;2;18;52;86mB\x1b[48;2;1;2;3mC",
            3,
            10,
        );
        let (snap, _, _) = snapshot(&t, 10, 0.0);
        let row = &snap.rows[0];
        assert_eq!(
            (row[0].fg.r, row[0].fg.g, row[0].fg.b),
            (255, 0, 0),
            "256 色 196 应为纯红"
        );
        assert_eq!(
            (row[1].fg.r, row[1].fg.g, row[1].fg.b),
            (18, 52, 86),
            "真彩色前景应原样保留"
        );
        assert_eq!(
            (row[2].bg.r, row[2].bg.g, row[2].bg.b),
            (1, 2, 3),
            "真彩色背景应原样保留"
        );
        assert_eq!(
            (row[2].fg.r, row[2].fg.g, row[2].fg.b),
            (18, 52, 86),
            "前景属性应跨单元格延续"
        );
    }

    #[test]
    fn indexed_palette_is_configurable() {
        let mut cfg = AppConfig::default();
        cfg.colors.ansi = vec!["#ff0000".into(), "#00ff00".into()];
        cfg.colors.indexed = vec![(196, "#123456".into())];
        let pal = crate::term::build_palette(&cfg);
        let ansi1 = pal.colors.0[0].to_srgb_u8();
        assert_eq!(
            (ansi1.0, ansi1.1, ansi1.2),
            (255, 0, 0),
            "colors.ansi[0] 应覆盖下标 0"
        );
        let ansi2 = pal.colors.0[1].to_srgb_u8();
        assert_eq!(
            (ansi2.0, ansi2.1, ansi2.2),
            (0, 255, 0),
            "colors.ansi[1] 应覆盖下标 1"
        );
        let c196 = pal.colors.0[196].to_srgb_u8();
        assert_eq!(
            (c196.0, c196.1, c196.2),
            (0x12, 0x34, 0x56),
            "indexed 应覆盖 256 色表"
        );
        // 未被覆盖的下标保持默认(216 色立方:196 = ff0000)
        let c46 = pal.colors.0[46].to_srgb_u8();
        assert_eq!(
            (c46.0, c46.1, c46.2),
            (0, 255, 0),
            "未被覆盖的下标应保留默认值"
        );
    }

    /// 汉字的 em 必须等于字号(1em 步进),字形保持自然大小。
    ///
    /// 这是 PxScale 语义的回归测试:ab_glyph 的 PxScale 是 asc-desc 高度,
    /// 微软雅黑 asc-desc = 1.32em,若直接传字号,汉字步进只有 0.76em(≈13.6px),
    /// 在两格(20px)里就会留出一道明显的缝。
    #[test]
    fn wide_glyph_keeps_em_size() {
        let cfg = AppConfig::default();
        let em = cfg.fonts.size as f64;
        let mut r = Renderer::new(&cfg, 1.0).expect("加载字体");
        let cell_w = r.cell_w;
        let g = r.glyph_info('中', StyleBits::default()).expect("中文字形");

        assert!(
            (g.advance as f64 - em).abs() < em * 0.1,
            "汉字步进应≈1em({em}px),实际 {:.2}px —— PxScale 未按 em 归一",
            g.advance
        );
        // wezterm 的策略:只有墨迹超过 span*cell_w(+0.25 格容差)才缩小
        let max_w = 2.25 * cell_w;
        assert!(
            g.w as f64 <= max_w,
            "汉字墨迹 {:.1}px 不应超过两格容差 {max_w:.1}px(会被迫缩小)",
            g.w
        );
        // 半角字形不加缩放,步进与格宽一致
        let m = r.glyph_info('M', StyleBits::default()).expect("M 字形");
        assert!(
            (m.advance as f64 - cell_w).abs() <= 1.0,
            "半角步进 {:.2} 应与格宽 {cell_w} 一致",
            m.advance
        );
    }

    /// Nerd Font 的图标位于 Unicode 私用区,主字体(Consolas)没有这些字形,
    /// 应通过自动发现的 Nerd Font 回退链拿到真实图标,而不是空白。
    #[test]
    fn nerd_font_icon_resolves_from_fallback() {
        let cfg = AppConfig::default();
        let mut r = Renderer::new(&cfg, 1.0).expect("加载字体");
        let cell_w = r.cell_w;
        // U+E0B0(powerline 右箭头)、U+F015(房子)
        for ch in ['\u{e0b0}', '\u{f015}'] {
            let Some(g) = r.glyph_info(ch, StyleBits::default()) else {
                crate::wlog!("跳过 {ch:?}:本机未安装 Nerd Font");
                return;
            };
            assert!(g.w > 1 && g.h > 1, "{ch:?} 应有真实图标位图");
            assert!(
                g.advance as f64 <= cell_w * 2.5,
                "图标步进 {:.1} 不应远超单元格({cell_w:.1})",
                g.advance
            );
        }
    }

    /// 仓库自带的 config.example.lua 必须能被真实加载(防止文档示例失效)
    #[test]
    fn example_config_parses() {
        let (cfg, err) = crate::config::load(Some(std::path::Path::new("config.example.lua")));
        assert!(err.is_none(), "示例配置解析失败:{err:?}");
        assert_eq!(cfg.fonts.family, "JetBrains Mono");
        assert_eq!(cfg.fonts.size, 18.0);
        assert_eq!(
            cfg.fonts.fallback,
            vec!["Nerd Font Symbols", "Microsoft YaHei"]
        );
        assert_eq!(cfg.fonts.nerd_font, crate::config::NerdFontMode::Auto);
    }

    /// Lua 里的字体表要能解析成 FontsConfig(含 Nerd Font 模式)
    #[test]
    fn lua_font_table_is_parsed() {
        let path = std::path::Path::new("target/test-font.lua");
        std::fs::write(
            path,
            r#"return {
  font = {
    family = "MesloLGL Nerd Font Mono",
    size = 13,
    line_height = 1.2,
    fallback = { "Microsoft YaHei" },
    nerd_font = true,
  },
}"#,
        )
        .unwrap();
        let (cfg, err) = crate::config::load(Some(path));
        assert!(err.is_none(), "配置解析失败:{err:?}");
        assert_eq!(cfg.fonts.family, "MesloLGL Nerd Font Mono");
        assert_eq!(cfg.fonts.size, 13.0);
        assert_eq!(cfg.fonts.line_height, Some(1.2));
        assert_eq!(cfg.fonts.fallback, vec!["Microsoft YaHei"]);
        assert_eq!(cfg.fonts.nerd_font, crate::config::NerdFontMode::Auto);
    }

    #[test]
    fn wide_char_takes_two_cells() {
        let t = term_with("中a", 3, 10);
        let (snap, _, _) = snapshot(&t, 10, 0.0);
        let row = &snap.rows[0];
        assert_eq!(row[0].text, "中");
        assert_eq!(row[0].span, 2, "全角字符应记两列");
        assert_eq!(row[1].text, "", "宽字符的续格应为空占位");
        assert_eq!(row[1].span, 0, "续格不画字形");
        assert_eq!(row[2].text, "a");
        assert_eq!(row[2].span, 1, "半角字符记一列");
    }

    // -----------------------------------------------------------------------
    // 图形协议:iTerm2(OSC 1337 File)/ Kitty(APC G)
    // -----------------------------------------------------------------------

    /// 内嵌字体的端到端回归:终端里写 "A😀中",应当画出彩色 emoji 像素。
    /// 这条同时覆盖 draw_text 的分派、图片缓存和 CPU 的 blit_image ——
    /// 只测 emoji.rs 的解码不够,位图很可能根本没进显示列表。
    #[test]
    fn builtin_emoji_is_drawn_in_color() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.fonts.family, "JetBrains Mono");
        let t = term_with("A😀中", 1, 8);
        let (snap, cursor, palette) = snapshot(&t, 8, 0.0);
        let mut r = Renderer::new(&cfg, 1.0).unwrap();
        let w = (2.0 * r.pad_x + 8.0 * r.cell_w) as u32;
        let h = (2.0 * r.pad_y + r.cell_h) as u32;
        let mut buf = vec![0u32; (w * h) as usize];
        r.paint(
            &mut buf, w, h, &snap, &cursor, &palette, None, palette.cursor_bg, true,
        );
        let colored = buf
            .iter()
            .filter(|&&p| {
                let (rr, gg, bb) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
                (rr as i32 - gg as i32).abs() > 40 || (gg as i32 - bb as i32).abs() > 40
            })
            .count();
        assert!(
            colored > 20,
            "彩色 emoji 应该产生成片的非灰像素,实际只有 {colored} 个"
        );
    }

    /// 2x2 PNG:左上红、右上绿、左下蓝、右下白
    fn png_2x2() -> Vec<u8> {
        let mut img = image::RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        img.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn frame_images(frame: &Frame) -> Vec<([u8; 32], [f32; 4])> {
        frame
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Image { key, dst } => Some((*key, *dst)),
                _ => None,
            })
            .collect()
    }

    /// 渲染一帧并返回 (帧, 画布宽, 画布高)
    fn render_frame(t: &Terminal, cfg: &AppConfig, rows: usize, cols: usize) -> (Frame, u32, u32) {
        let (snap, cursor, palette) = snapshot(t, cols, 0.0);
        let mut r = Renderer::new(cfg, 1.0).unwrap();
        let w = (2.0 * r.pad_x + cols as f64 * r.cell_w) as u32;
        let h = (2.0 * r.pad_y + rows as f64 * r.cell_h) as u32;
        let mut frame = Frame::default();
        r.build_frame(
            &mut frame,
            w,
            h,
            &snap,
            &cursor,
            &palette,
            None,
            palette.cursor_bg,
            true,
        );
        (frame, w, h)
    }

    #[test]
    fn iterm2_image_becomes_one_quad() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(png_2x2());
        // 40px 宽 = 4 格(测试环境每格 10px),20px 高 = 1 行
        let seq = format!(
            "\x1b]1337;File=inline=1;width=40px;height=20px;preserveAspectRatio=0:{b64}\x07"
        );
        let t = term_with(&seq, 6, 20);
        let (snap, _, _) = snapshot(&t, 20, 0.0);
        assert_eq!(snap.rows[0][0].images.len(), 1, "起点格应挂上图");
        assert_eq!(snap.rows[0][3].images.len(), 1, "图跨 4 格,第 4 格也有");
        assert!(snap.rows[0][4].images.is_empty(), "图右边应当是空白格");

        let cfg = AppConfig::default();
        let (frame, _, _) = render_frame(&t, &cfg, 6, 20);
        let imgs = frame_images(&frame);
        assert_eq!(imgs.len(), 1, "跨格的图只应发一个四边形,否则格线处会有接缝");
        let dst = imgs[0].1;
        assert!(
            (dst[2] - 4.0 * 10.0).abs() < 6.0,
            "宽度应约 4 格:{}",
            dst[2]
        );
    }

    #[test]
    fn kitty_raw_rgba_is_drawn_above_text() {
        use base64::Engine as _;
        let mut px = Vec::new();
        for c in [
            [255u8, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
        ] {
            px.extend_from_slice(&c);
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&px);
        // f=32 = 裸 RGBA,s/v = 尺寸,a=T = 传输并显示
        let seq = format!("\x1b_Gf=32,s=2,v=2,a=T,i=1;{b64}\x1b\\");
        let t = term_with(&seq, 6, 20);
        let (snap, _, _) = snapshot(&t, 20, 0.0);
        assert_eq!(snap.rows[0][0].images.len(), 1, "Kitty 图形应挂到光标格");

        let cfg = AppConfig::default();
        let (frame, _, _) = render_frame(&t, &cfg, 6, 20);
        let imgs = frame_images(&frame);
        assert_eq!(imgs.len(), 1, "Kitty 图形应产生一个四边形");
        // 2x2 px 的图比一格还小:padding 会把"本格被图覆盖的部分"压到很小,
        // 换算误差就会被放大成 0 或负值,这里守住"小但非零"。
        let dst = imgs[0].1;
        assert!(
            dst[2] > 0.5 && dst[2] <= 10.5,
            "2px 的图宽度应远小于一格:{dst:?}"
        );
        assert!(
            dst[3] > 0.5 && dst[3] <= 18.5,
            "2px 的图高度应远小于一行:{dst:?}"
        );
        // 左上角落在内容区原点(即画布内边距)
        assert!(dst[0] >= 0.0 && dst[1] >= 0.0, "图应贴在格子左上角:{dst:?}");
    }

    #[test]
    fn images_can_be_disabled() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(png_2x2());
        let seq = format!("\x1b]1337;File=inline=1;width=40px;height=20px:{b64}\x07");
        let t = term_with(&seq, 6, 20);

        let mut cfg = AppConfig::default();
        cfg.images.enabled = false;
        let (frame, _, _) = render_frame(&t, &cfg, 6, 20);
        assert!(
            frame_images(&frame).is_empty(),
            "images.enable = false 时不应绘制任何图"
        );
    }

    #[test]
    fn image_pixels_reach_the_callback() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(png_2x2());
        let seq = format!(
            "\x1b]1337;File=inline=1;width=40px;height=20px;preserveAspectRatio=0:{b64}\x07"
        );
        let t = term_with(&seq, 6, 20);
        let cfg = AppConfig::default();
        let (snap, cursor, palette) = snapshot(&t, 20, 0.0);
        let mut r = Renderer::new(&cfg, 1.0).unwrap();
        let w = (2.0 * r.pad_x + 20.0 * r.cell_w) as u32;
        let h = (2.0 * r.pad_y + 6.0 * r.cell_h) as u32;
        let mut buf = vec![0u32; (w * h) as usize];
        r.paint(
            &mut buf,
            w,
            h,
            &snap,
            &cursor,
            &palette,
            None,
            palette.cursor_bg,
            true,
        );

        // 左端应偏红、右端应偏绿(源图左上红 / 右上绿)
        let px = |x: f64, y: f64| buf[(y as u32 * w + x as u32) as usize];
        let left = px(r.pad_x + 2.0, r.pad_y + 4.0);
        let right = px(r.pad_x + 4.0 * r.cell_w - 3.0, r.pad_y + 4.0);
        let red = (left >> 16) & 0xff;
        let green = (right >> 8) & 0xff;
        assert!(red > 120, "图片左半边应接近红色,实际 #{left:06x}");
        assert!(green > 120, "图片右半边应接近绿色,实际 #{right:06x}");
    }
}

// ---------------------------------------------------------------------------
// 开发辅助:把一帧画面导出成 BMP,便于用眼睛核对字形/间距(不影响正常运行)
//   运行:cargo test dump_frame -- --ignored --nocapture
// ---------------------------------------------------------------------------

#[cfg(test)]
mod dump {
    use super::*;
    use crate::config::AppConfig;
    use wezterm_term::TerminalSize;

    fn write_bmp(path: &str, buf: &[u32], w: u32, h: u32) {
        let row_bytes = (w * 3 + 3) & !3;
        let data_size = row_bytes * h;
        let mut out = Vec::with_capacity((54 + data_size) as usize);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(54 + data_size).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(w as i32).to_le_bytes());
        out.extend_from_slice(&(h as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&data_size.to_le_bytes());
        out.extend_from_slice(&[0u8; 16]);
        for y in (0..h).rev() {
            let mut row = Vec::with_capacity(row_bytes as usize);
            for x in 0..w {
                let p = buf[(y * w + x) as usize];
                row.push((p & 0xff) as u8);
                row.push(((p >> 8) & 0xff) as u8);
                row.push(((p >> 16) & 0xff) as u8);
            }
            while row.len() < row_bytes as usize {
                row.push(0);
            }
            out.extend_from_slice(&row);
        }
        std::fs::write(path, out).unwrap();
    }

    fn render_to(path: &str, cfg: &AppConfig, rows: usize, cols: usize, scroll: f32) {
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, cfg, Box::new(std::io::sink()));
        t.advance_bytes(
            "中文渲染 spacing 测试｜中文中文中文\r\n\
             Nerd: \u{e0b0}\u{e0b2}\u{e0a0} \u{f015} \u{f17c} \u{e70f} \u{e718} \u{f0001} \u{f0016}\r\n\
             Dev: \u{e73b} \u{e781} \u{e7a2} \u{e6b8} \u{e232} \u{e26b} \u{f0e7}\r\n\
             中文English混排 mixed 中文 0O1lI|!\r\n\
             \x1b[1m粗体 Bold 中文\x1b[0m \x1b[3m斜体 Italic 汉字\x1b[0m\r\n\
             \x1b[38;5;196m256色\x1b[0m \x1b[38;2;120;200;255m真彩色\x1b[0m \u{2500}\u{2502}\u{250c}\u{2510}\r\n\
             中 文 中 文（手工空格对比）"
                .as_bytes(),
        );
        let (snap, cursor, palette) = snapshot(&t, cols, scroll);
        let mut r = Renderer::new(cfg, 1.0).unwrap();
        crate::wlog!(
            "  family={:?} cell={}x{} ascent={}",
            cfg.fonts.family,
            r.cell_w,
            r.cell_h,
            r.ascent
        );
        let (w, h) = (
            (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32,
            (rows as f64 * r.cell_h + 2.0 * r.pad_y) as u32,
        );
        let mut buf = vec![0u32; (w * h) as usize];
        r.paint(
            &mut buf,
            w,
            h,
            &snap,
            &cursor,
            &palette,
            None,
            palette.cursor_bg,
            true,
        );
        write_bmp(path, &buf, w, h);
        crate::wlog!("frame -> {path} ({w}x{h})");
    }

    #[test]
    #[ignore]
    fn dump_frame() {
        // 1) 默认配置:Consolas + 微软雅黑 + 自动发现的 Nerd Font 图标
        let cfg = AppConfig::default();
        render_to("target/vectorterm-frame.bmp", &cfg, 12, 62, 0.0);

        // 2) 主字体直接换成 Nerd Font Mono(Meslo,本机用户字体目录里有)
        let mut nerd = AppConfig::default();
        nerd.fonts.family = "MesloLGL Nerd Font Mono".to_string();
        nerd.fonts.fallback = vec!["Microsoft YaHei".to_string()];
        nerd.fonts.size = 16.0;
        nerd.fonts.line_height = Some(0.9);
        render_to("target/vectorterm-frame-nerd.bmp", &nerd, 12, 62, 0.0);
    }

    /// 亚行滚动的一帧:内容整体上移半行,第一行只露出下半截(应无接缝/空洞)
    #[test]
    #[ignore]
    fn dump_scroll_frame() {
        let mut cfg = AppConfig::default();
        cfg.fonts.size = 16.0;
        render_to("target/vectorterm-scroll.bmp", &cfg, 10, 52, 2.5);
    }

    fn omp_frame(path: &str, cfg: &AppConfig) {
        let (rows, cols) = (4usize, 64usize);
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, cfg, Box::new(std::io::sink()));
        // 仿 Oh My Posh 的 Catppuccin 风格提示符:
        // 每段 = 底色 + 图标 + 文本 + (下一段底色作前景的 powerline 箭头)
        let seg = |bg: (u8, u8, u8),
                   fg: (u8, u8, u8),
                   text: &str,
                   next: (u8, u8, u8),
                   arrow: &str| {
            format!(
                "\x1b[48;2;{};{};{}m\x1b[38;2;{};{};{}m {}\x1b[48;2;{};{};{}m\x1b[38;2;{};{};{}m{}\x1b[0m",
                bg.0, bg.1, bg.2, fg.0, fg.1, fg.2, text, next.0, next.1, next.2, bg.0, bg.1, bg.2, arrow
            )
        };
        let a = (137, 180, 250);
        let b = (245, 189, 230);
        let c = (166, 227, 161);
        let bg0 = (30, 30, 46);
        let arrow = "\u{e0b0}";
        let mut s = String::new();
        s.push_str(&seg(a, bg0, "\u{f015} ~/dev/vectorterm", b, arrow));
        s.push_str(&seg(b, bg0, "\u{e725} main", c, arrow));
        s.push_str(&seg(c, bg0, "\u{e718} v20.11", a, arrow));
        s.push_str("\x1b[48;2;30;30;46m\x1b[38;2;137;180;250m\u{e0b0}\x1b[0m\r\n");
        // 细箭头 \u{e0b1} + 常用 OMP 图标,并排看对齐与宽度
        s.push_str(&seg(
            a,
            bg0,
            "\u{f07b}\u{f1c0}\u{f121}\u{f0e7}",
            b,
            "\u{e0b1}",
        ));
        s.push_str(&seg(
            b,
            bg0,
            "\u{f00c}\u{f00d}\u{f017}\u{f073}",
            c,
            "\u{e0b1}",
        ));
        s.push_str(&seg(
            c,
            bg0,
            "\u{f0a0}\u{f015}\u{e70f}\u{f0e7}",
            a,
            "\u{e0b1}",
        ));
        s.push_str("\x1b[0m\r\n");
        s.push_str("\u{e0b0}\u{e0b2}\u{e0b3}\u{e0b4}\u{e0b5}\u{e0b6}\u{e0b7} 图标与文字基线 Icons vs text 中文\r\n");
        t.advance_bytes(s.as_bytes());

        let (snap, cursor, palette) = snapshot(&t, cols, 0.0);
        let mut r = Renderer::new(cfg, 1.0).unwrap();
        crate::wlog!(
            "  family={:?} cell={}x{} ascent={}",
            cfg.fonts.family,
            r.cell_w,
            r.cell_h,
            r.ascent
        );
        let (w, h) = (
            (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32,
            (rows as f64 * r.cell_h + 2.0 * r.pad_y) as u32,
        );
        let mut buf = vec![0u32; (w * h) as usize];
        r.paint(
            &mut buf,
            w,
            h,
            &snap,
            &cursor,
            &palette,
            None,
            palette.cursor_bg,
            true,
        );
        write_bmp(path, &buf, w, h);
        crate::wlog!("frame -> {path} ({w}x{h})");
    }

    /// 块元素必须按格子几何填在正确的位置上。
    ///
    /// 半块字符画(DSH、chafa、timg 那类)把一格当成上下两个"像素":
    /// `▄` 只点亮下半、`▀` 只点亮上半,`█` 整格点亮。所以这三者的边界
    /// 必须严格落在格子中线/上下沿 —— 只要 `▄` 的高度不是正好半格
    /// (以前按字体 em 盒映射就只有 0.37 格),整幅画的比例就全歪了,
    /// 行与行之间还会露出一条暗带。
    #[test]
    fn block_chars_tile_perfectly() {
        let mut cfg = AppConfig::default();
        cfg.fonts.size = 20.0;
        // 故意把行高压到字形自然高度以下 —— em 盒映射最容易在这一档露缝
        cfg.fonts.line_height = Some(0.886);
        let (rows, cols) = (6usize, 6usize);
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, &cfg, Box::new(std::io::sink()));
        t.advance_bytes(
            "\u{2584}\u{2584}\u{2584}\u{2584}\u{2584}\u{2584}\r\n\
             \u{2580}\u{2580}\u{2580}\u{2580}\u{2580}\u{2580}\r\n\
             \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\r\n\
             \u{2581}\u{2581}\u{2581}\u{2581}\u{2581}\u{2581}"
                .as_bytes(),
        );
        let (snap, cursor, palette) = snapshot(&t, cols, 0.0);
        let mut r = Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = (
            (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32,
            (rows as f64 * r.cell_h + 2.0 * r.pad_y) as u32,
        );
        let mut buf = vec![0u32; (w * h) as usize];
        r.paint(
            &mut buf,
            w,
            h,
            &snap,
            &cursor,
            &palette,
            None,
            palette.cursor_bg,
            true,
        );

        let bg = pack(palette.bg);
        let fg = pack(palette.fg);
        let at = |x: i64, y: i64| buf[y as usize * w as usize + x as usize];
        let o = r.cell_rect(0, 0);
        let (x0, y0) = (o.x.round() as i64, o.y.round() as i64);
        let (cw, ch) = (r.cell_w.round() as i64, r.cell_h.round() as i64);
        let half = (r.cell_h * 0.5).round() as i64;
        let eighth = (r.cell_h * 0.125).round() as i64;
        // 内缩 1px:只查块内部,不跟格子边界上的取整较劲。
        // 只查前 4 列:最后一列被光标块占着(写到行尾时光标停在那)。
        let (xl, xr) = (x0 + 1, x0 + 4 * cw - 1);

        // 逐行断言:该亮的亮、该灭的灭,边界必须正好落在格子分数位上
        let cases = [
            ("▄", 0usize, half, ch),        // 下半格
            ("▀", 1usize, 0, half),         // 上半格
            ("█", 2usize, 0, ch),           // 整格
            ("▁", 3usize, ch - eighth, ch), // 下 1/8
        ];
        for (name, row, lit_from, lit_to) in cases {
            let top = y0 + row as i64 * ch;
            for x in xl..xr {
                for y in 0..ch {
                    let want = if y >= lit_from && y < lit_to { fg } else { bg };
                    assert_eq!(
                        at(x, top + y),
                        want,
                        "{name} 第 {y} 行像素不对(格子高 {ch},中点 {half}): x={x} y={}",
                        top + y
                    );
                }
            }
        }
    }

    /// 制表符方框:线条必须首尾相接、粗细一致(方块元素同理)

    #[test]
    #[ignore]
    fn dump_box_frame() {
        let mut cfg = AppConfig::default();
        cfg.fonts.size = 20.0;
        let (rows, cols) = (6usize, 40usize);
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, &cfg, Box::new(std::io::sink()));
        t.advance_bytes(
            "\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}\r\n\
             \u{2502} 中文 box \u{2502}\r\n\
             \u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}\r\n\
             \u{2588}\u{2588}\u{2588}\u{2593}\u{2592}\u{2591}\u{2580}\u{2584}\u{258c}\u{2590}\u{2597}\r\n\
             \u{2501}\u{2501}\u{2501}\u{2501}\u{2550}\u{2550}\u{2550}\u{2550}\u{2551} 双线"
                .as_bytes(),
        );
        let (snap, cursor, palette) = snapshot(&t, cols, 0.0);
        let mut r = Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = (
            (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32,
            (rows as f64 * r.cell_h + 2.0 * r.pad_y) as u32,
        );
        let mut buf = vec![0u32; (w * h) as usize];
        r.paint(
            &mut buf,
            w,
            h,
            &snap,
            &cursor,
            &palette,
            None,
            palette.cursor_bg,
            true,
        );
        write_bmp("target/vectorterm-box.bmp", &buf, w, h);
        crate::wlog!("frame -> target/vectorterm-box.bmp ({w}x{h})");
    }

    /// Oh My Posh 提示符:powerline 分隔符与 Nerd Font 图标是否连贯、无挤压
    #[test]
    #[ignore]
    fn dump_omp_frame() {
        let mut cfg = AppConfig::default();
        cfg.fonts.size = 20.0;
        omp_frame("target/vectorterm-omp.bmp", &cfg);

        let mut nerd = AppConfig::default();
        nerd.fonts.family = "MesloLGL Nerd Font Mono".to_string();
        nerd.fonts.fallback = vec!["Microsoft YaHei".to_string()];
        nerd.fonts.size = 20.0;
        nerd.fonts.line_height = Some(1.0);
        omp_frame("target/vectorterm-omp-nerd.bmp", &nerd);
    }

    /// 性能基准:200x60 网格(约 2400x1260 像素)下各阶段耗时。
    ///
    /// `cargo test --release bench_paint -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_paint() {
        use std::time::Instant;

        let mut cfg = AppConfig::default();
        cfg.fonts.size = 16.0;
        let (rows, cols) = (60usize, 200usize);

        // 满屏彩色文本:每格都有字形、背景色每列都换,构造最坏情况
        let mut input = String::new();
        for r in 0..rows {
            for c in 0..cols {
                // 交替前景/背景,制造大量不同的 fill_rect
                if (r + c) % 2 == 0 {
                    input.push_str("\x1b[38;5;196mW\x1b[0m");
                } else {
                    input.push_str("\x1b[48;5;24;38;5;231mM\x1b[0m");
                }
            }
            input.push_str("\r\n");
        }

        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, &cfg, Box::new(std::io::sink()));
        t.advance_bytes(input.as_bytes());

        let mut r = Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = (
            (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32,
            (rows as f64 * r.cell_h + 2.0 * r.pad_y) as u32,
        );
        let mut buf = vec![0u32; (w * h) as usize];

        // 预热字形缓存(首次栅格化不算进稳定态)
        let (snap, cursor, palette) = snapshot(&t, cols, 0.0);
        r.paint(
            &mut buf,
            w,
            h,
            &snap,
            &cursor,
            &palette,
            None,
            palette.cursor_bg,
            true,
        );

        // 空白屏(只有背景):用来分离"背景填充"的固定成本
        let blank = crate::term::create_terminal(size, &cfg, Box::new(std::io::sink()));
        let (bsnap, bcursor, bpal) = snapshot(&blank, cols, 0.0);

        let iters = 60;
        let mut snapshot_us = 0.0f64;
        let mut layout_us = 0.0f64;
        let mut paint_us = 0.0f64;
        let mut blank_us = 0.0f64;
        let mut frame = Frame::default();

        for i in 0..iters {
            let t0 = Instant::now();
            let (s, c, p) = snapshot(&t, cols, (i % 3) as f32 * 0.5);
            snapshot_us += t0.elapsed().as_secs_f64();

            // 布局(CPU/GPU 共用):GPU 路径每帧的 CPU 成本就到这里为止
            let t1 = Instant::now();
            r.build_frame(&mut frame, w, h, &s, &c, &p, None, p.cursor_bg, true);
            layout_us += t1.elapsed().as_secs_f64();

            let t2 = Instant::now();
            r.paint_cpu(&mut buf, w, h, &frame);
            paint_us += t2.elapsed().as_secs_f64();

            let t3 = Instant::now();
            r.paint(
                &mut buf,
                w,
                h,
                &bsnap,
                &bcursor,
                &bpal,
                None,
                bpal.cursor_bg,
                true,
            );
            blank_us += t3.elapsed().as_secs_f64();
        }

        let ms = |v: f64| v * 1000.0 / iters as f64;
        crate::wlog!(
            "\n=== 软渲染基准 {cols}x{rows} 格,画布 {w}x{h} ({:.1} MPx) ===",
            (w * h) as f64 / 1e6
        );
        crate::wlog!("  快照(锁内) : {:.2} ms", ms(snapshot_us));
        crate::wlog!(
            "  布局(显示列表): {:.2} ms  ← GPU 路径的 CPU 成本到此为止",
            ms(layout_us)
        );
        crate::wlog!("  光栅化(全屏) : {:.2} ms  ← CPU 路径独有", ms(paint_us));
        crate::wlog!("  光栅化(空白) : {:.2} ms  ← 纯背景填充", ms(blank_us));
        crate::wlog!(
            "  CPU 单帧   : {:.2} ms  (60fps 预算 16.7ms,120fps 8.3ms)",
            ms(snapshot_us + layout_us + paint_us)
        );
        crate::wlog!(
            "  GPU 单帧   : {:.2} ms  (仅快照+布局,像素交给显卡)",
            ms(snapshot_us + layout_us)
        );
    }

    /// 图形协议:iTerm2 与 Kitty 各放一张图,导出首帧核对位置与缩放
    /// `cargo test dump_image_frame -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_image_frame() {
        use base64::Engine as _;
        let cfg = AppConfig::default();
        let (rows, cols) = (14usize, 44usize);
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, &cfg, Box::new(std::io::sink()));

        // 120x60 的彩条图:三段竖条
        let (iw, ih) = (120u32, 60u32);
        let mut img = image::RgbaImage::new(iw, ih);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            *p = if x < iw / 3 {
                image::Rgba([220, 60, 60, 255])
            } else if x < iw * 2 / 3 {
                image::Rgba([60, 200, 90, 255])
            } else {
                image::Rgba([70, 120, 230, 255])
            };
        }
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let mut s = String::from("iTerm2 (OSC 1337 File) — 120x60 PNG:\r\n");
        s.push_str(&format!(
            "\x1b]1337;File=inline=1;width=120px;height=60px;preserveAspectRatio=0:{b64}\x07"
        ));
        s.push_str("\r\nKitty (APC G, f=32 裸 RGBA) — 60x30:\r\n");

        let (kw, kh) = (60u32, 30u32);
        let mut raw = Vec::new();
        for y in 0..kh {
            for x in 0..kw {
                let on = ((x / 6) + (y / 6)) % 2 == 0;
                raw.extend_from_slice(if on {
                    &[250, 190, 60, 255]
                } else {
                    &[40, 40, 48, 255]
                });
            }
        }
        let kb64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        s.push_str(&format!("\x1b_Gf=32,s={kw},v={kh},a=T,i=7;{kb64}\x1b\\"));
        t.advance_bytes(s.as_bytes());

        let (snap, cursor, palette) = snapshot(&t, cols, 0.0);
        for (r, row) in snap.rows.iter().enumerate() {
            let marks: Vec<usize> = row
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.images.is_empty())
                .map(|(i, _)| i)
                .collect();
            if !marks.is_empty() {
                crate::wlog!("  row {r}: image cells {:?}", marks);
            }
        }
        let mut r = Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = (
            (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32,
            (rows as f64 * r.cell_h + 2.0 * r.pad_y) as u32,
        );
        let mut buf = vec![0u32; (w * h) as usize];
        r.paint(
            &mut buf,
            w,
            h,
            &snap,
            &cursor,
            &palette,
            None,
            palette.cursor_bg,
            true,
        );
        write_bmp("target/vectorterm-image.bmp", &buf, w, h);
        crate::wlog!("frame -> target/vectorterm-image.bmp ({w}x{h})");
    }

    /// 状态栏的 `font_scale` 是"把字形按比例缩放",所以同一段宽度里
    /// 能塞下的字数必须跟着变 —— 缩得越小塞得越多。截断逻辑要是写错,
    /// 这里就会立刻炸出来(小字号多画、大字号少画)。
    #[test]
    fn status_bar_scale_changes_how_much_fits() {
        let cfg = AppConfig::default();
        let mut r = Renderer::new(&cfg, 1.0).expect("加载字体");
        // 两段不同颜色:间距只加在片段之间,所以必须有两段才测得出来
        let spans = crate::status::parse(
            "\u{1b}[38;2;230;230;230m0123456789\u{1b}[38;2;120;170;255mabcdefghij\u{1b}[0m",
            &[Rgb {
                r: 200,
                g: 200,
                b: 200,
            }; 256],
        );
        let max_x = r.cell_w * 15.0;
        let gap3 = r.cell_w * 3.0;
        let mut drawn = |scale: f64, gap: f64| -> usize {
            let mut frame = Frame::default();
            r.draw_rich_scaled(
                &mut frame,
                &spans,
                0.0,
                max_x,
                20.0,
                Rgb {
                    r: 230,
                    g: 230,
                    b: 230,
                },
                scale,
                gap,
                1.0,
            );
            frame.items.len()
        };
        let small = drawn(0.5, 0.0);
        let normal = drawn(1.0, 0.0);
        let big = drawn(2.0, 0.0);
        assert!(small > normal, "0.5× 应塞下更多字:{small} vs {normal}");
        assert!(normal > big, "2.0× 应塞下更少字:{normal} vs {big}");
        assert!(big > 0, "放大后也要真的画出东西");

        // `status_bar.item_spacing`:片段之间留白要真的占位置(否则调了没反应)
        let gapped = drawn(1.0, gap3);
        assert!(
            gapped < normal,
            "片段间距应挤掉后面的字:{gapped} vs {normal}"
        );
        assert!(gapped > 0);
    }

    /// `status_bar.space_width`:收窄空格之后,同样一段宽度能多塞下字 ——
    /// 说明空隙真的小了,而且只影响空格,字形尺寸没变。
    #[test]
    fn status_space_width_squeezes_gaps() {
        let cfg = AppConfig::default();
        let mut r = Renderer::new(&cfg, 1.0).expect("加载字体");
        let spans = crate::status::parse(
            "ab   cd   ef",
            &[Rgb {
                r: 200,
                g: 200,
                b: 200,
            }; 256],
        );
        let max_x = r.cell_w * 8.0;
        let mut drawn = |space: f64| -> usize {
            let mut frame = Frame::default();
            r.draw_rich_scaled(
                &mut frame,
                &spans,
                0.0,
                max_x,
                20.0,
                Rgb {
                    r: 230,
                    g: 230,
                    b: 230,
                },
                1.0,
                0.0,
                space,
            );
            frame.items.len()
        };
        // 空格各占一格时,八格的宽度只放得下 "ab   cd";空格减半后能多出一个 e
        let wide = drawn(1.0);
        let tight = drawn(0.5);
        assert!(wide > 0);
        assert!(
            tight > wide,
            "空格收窄后应塞下更多字:{tight} vs {wide}"
        );
        // 空格本身不画(没有墨迹),所以多出来的只能是真字形
        let chars = spans.iter().map(|s| s.text.chars().count()).sum::<usize>();
        assert!(wide < chars);
    }

    /// 段尾那一格:宽度刚刚好时,最后一个字不能因为浮点误差被丢掉。
    ///
    /// 以前 `' 晚上好'` 只显示成 `' 晚上'` 就是这个原因 —— 上一步的宽度限制
    /// 按 `x + Σw` 算,而绘制循环拿 `(cx) + w`(cx 从 x 起累加)去比,两个
    /// 加法顺序不同,差 1e-13 就会把最后一个字判成"放不下"。
    #[test]
    fn last_glyph_of_a_span_survives_rounding() {
        let cfg = AppConfig::default();
        let mut r = Renderer::new(&cfg, 1.0).expect("加载字体");
        let scale = 0.9;
        let spans = crate::status::parse(
            " \u{f059a} 晚上好",
            &[Rgb {
                r: 200,
                g: 200,
                b: 200,
            }; 256],
        );
        let cw = r.advance_w * scale;
        // 空格按 0.2 格算(主题里的 `space_width`),其余按格数
        let mut width = 0.0;
        for ch in " \u{f059a} 晚上好".chars() {
            width += if ch == ' ' {
                cw * 0.2
            } else {
                crate::tabbar::char_width(ch) as f64 * cw
            };
        }
        // 起点扫一遍带小数的偏移:两边的加法和总会在某些偏移上分道扬镳
        for k in 0..400 {
            let x0 = 3.3 + k as f64 * 0.07;
            let mut frame = Frame::default();
            r.draw_rich_scaled(
                &mut frame,
                &spans,
                x0,
                x0 + width,
                20.0,
                Rgb {
                    r: 230,
                    g: 230,
                    b: 230,
                },
                scale,
                0.0,
                0.2,
            );
            assert_eq!(
                frame.items.len(),
                4,
                "x0={x0} 时正好放得下整段,图标 + 三个汉字都要画出来"
            );
        }
    }

    /// 状态栏图标要按"文字高度"摆:画满 em 框的图标(比如 MDI 的电池)收到
    /// 数字高一点,并且**墨迹中线对齐文字中线** —— 不然它会比旁边的字高出一截、
    /// 底边坠到基线下面,一排看过去就是歪的。
    #[test]
    fn oversized_status_icon_lines_up_with_text() {
        let cfg = AppConfig::default();
        let mut r = Renderer::new(&cfg, 1.0).expect("加载字体");
        let style = StyleBits::default();
        let scale = 0.9;
        let (band_c, band_h) = r.text_band(style, scale);
        assert!(band_h > 1.0, "数字的墨迹高度应该是个正数:{band_h}");

        // MDI 电池:这类图标把字形画满 em 框,正是"错位"的来源
        let battery = '\u{f0079}';
        let raw = r
            .glyph_size_at(battery, style, scale)
            .expect("应有电池字形")
            .1 as f64;
        assert!(
            raw > band_h * ICON_OVERSIZE,
            "这个图标本来就比数字高:{raw} vs {band_h}"
        );

        let baseline = 20.0;
        let mut frame = Frame::default();
        let spans = vec![crate::status::Span {
            text: format!(" {battery} 97%"),
            ..Default::default()
        }];
        r.draw_rich_scaled(
            &mut frame,
            &spans,
            0.0,
            500.0,
            baseline,
            Rgb {
                r: 230,
                g: 230,
                b: 230,
            },
            scale,
            0.0,
            0.2,
        );

        let icon = frame
            .items
            .iter()
            .find_map(|i| match i {
                crate::frame::Item::Glyph { dst, .. } => Some(*dst),
                _ => None,
            })
            .expect("第一个字形应是电池图标");
        let dh = icon[3] as f64;
        assert!(
            dh <= band_h * ICON_RATIO + 1.5,
            "图标不该还铺满整格:{dh} vs 数字 {band_h}"
        );
        let center = icon[1] as f64 + dh / 2.0;
        let want = baseline + band_c;
        assert!(
            (center - want).abs() <= 1.0,
            "图标中线要落在文字中线上:{center} vs {want}"
        );
    }

    /// 状态栏换字号时必须**重新栅格化**字形,而不是把正文字形拉伸。
    ///
    /// 拉伸过的位图尺寸不会变 —— 所以只要"字号变大 → 位图也变大"成立,就说明
    /// 走的是一条真正按目标尺寸出图的路径(字号变大仍糊的话,这条会挂)。
    #[test]
    fn status_scale_rerasterizes_glyphs() {
        let cfg = AppConfig::default();
        let mut r = Renderer::new(&cfg, 1.0).expect("加载字体");
        let style = StyleBits::default();
        let base = r.glyph_size_at('A', style, 1.0).expect("应有字形");
        let mid = r.glyph_size_at('A', style, 1.5).expect("应有字形");
        let big = r.glyph_size_at('A', style, 2.0).expect("应有字形");
        assert!(
            mid.1 > base.1 && big.1 > mid.1,
            "位图应随字号变大(说明是按目标尺寸重新栅格化):{base:?} {mid:?} {big:?}"
        );
        assert!(
            big.1 >= (base.1 as f64 * 1.7) as u32,
            "2 倍字号应接近 2 倍高度:{base:?} {big:?}"
        );
        // 正文字号仍走原缓存,像素级不变
        let plain = r.glyph_info('A', style).expect("应有字形");
        assert_eq!((plain.w, plain.h), base, "1.0 必须复用正文位图");
    }

    /// 目测标签栏:导出一张 CPU 渲染的 BMP
    /// `cargo test dump_tab_bar_frame -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_tab_bar_frame() {
        // 1.0 与放大后的状态栏各导一张:放大那张用来确认字形是**按目标尺寸
        // 重新栅格化**的,而不是把正文字形拉伸(拉伸会糊、颜色会发灰)。
        // 最后两张对比「空格宽度」:同一份脚本文字,1.0 是小点两侧各 3 个字宽,
        // 0.5 就是各 1.5 个字宽 —— 切片之间明显靠拢,字本身没变窄。
        dump_tab_bar_frame_at(1.0, 0.0, 1.0, "target/vectorterm-tabbar.bmp");
        dump_tab_bar_frame_at(1.3, 0.0, 1.0, "target/vectorterm-tabbar-130.bmp");
        dump_tab_bar_frame_at(1.0, 8.0, 1.0, "target/vectorterm-tabbar-gap8.bmp");
        dump_tab_bar_frame_at(2.0, 0.0, 1.0, "target/vectorterm-tabbar-200.bmp");
        dump_tab_bar_frame_at(1.0, 0.0, 1.0, "target/vectorterm-tabbar-space100.bmp");
        dump_tab_bar_frame_at(1.0, 0.0, 0.5, "target/vectorterm-tabbar-space50.bmp");
        dump_tab_bar_frame_at(1.0, 0.0, 0.35, "target/vectorterm-tabbar-space35.bmp");
    }

    fn dump_tab_bar_frame_at(
        status_scale: f64,
        status_gap: f64,
        status_space: f64,
        path: &str,
    ) {
        let cfg = AppConfig::default();
        let mut r = Renderer::new(&cfg, 1.0).unwrap();
        let m = crate::tabbar::Metrics::new(r.cell_w, r.cell_h);
        r.content_top = m.bar_h;
        crate::wlog!(
            "cell_w={} advance_w={} (取整让状态栏每个字多占 {:.2}px)",
            r.cell_w,
            r.advance_w,
            r.cell_w - r.advance_w
        );

        let (cols, rows) = (130usize, 10usize);
        let w = (cols as f64 * r.cell_w + 2.0 * r.pad_x).round() as u32;
        let h = (rows as f64 * r.cell_h + 2.0 * r.pad_y + m.bar_h).round() as u32;

        let size = wezterm_term::TerminalSize {
            rows,
            cols,
            pixel_width: w as usize,
            pixel_height: h as usize,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, &cfg, Box::new(std::io::sink()));
        t.advance_bytes("PowerShell 7\r\n$ cargo test\r\n标签栏 tab bar 测试\r\n".as_bytes());
        let (snap, cursor, palette) = snapshot(&t, cols, 0.0);

        let mut frame = Frame::default();
        r.build_frame(
            &mut frame,
            w,
            h,
            &snap,
            &cursor,
            &palette,
            None,
            palette.cursor_bg,
            true,
        );

        let layout = crate::tabbar::layout(
            w as f64,
            h as f64,
            4,
            &m,
            &crate::tabbar::Options {
                edge_inset: 10.0,
                top_inset: 10.0,
                at_bottom: false,
                max_width_cells: 16,
                show_new_tab: true,
                show_close: true,
                // 导出时带上窗口按钮,顺便回验 INTEGRATED_BUTTONS|RESIZE 的观感
                csd: true,
                window_buttons: vec![
                    crate::tabbar::WindowButton::Minimize,
                    crate::tabbar::WindowButton::Maximize,
                    crate::tabbar::WindowButton::Close,
                ],
                button_alignment: crate::tabbar::ButtonAlign::Right,
                // 右状态栏占一段,顺手把带颜色的状态栏也画进导出的帧里
                left_status_w: 0.0,
                right_status_w: m.cell_w * 70.0 * status_scale,
            },
        );
        let items = vec![
            TabBarItem {
                title: "PowerShell".into(),
                active: true,
                ..Default::default()
            },
            TabBarItem {
                title: "cmd".into(),
                hovered: true,
                close_hovered: true,
                ..Default::default()
            },
            TabBarItem {
                title: "Ubuntu".into(),
                ..Default::default()
            },
            TabBarItem {
                title: "一个很长很长很长的标签标题".into(),
                // `format-tab-title` 给的自定义配色
                bg: Some(Rgb {
                    r: 0x3a,
                    g: 0x2f,
                    b: 0x5c,
                }),
                fg: Some(Rgb {
                    r: 0xff,
                    g: 0xcc,
                    b: 0x66,
                }),
                ..Default::default()
            },
        ];
        // 用真实主题里那条状态栏(问候语 / 时钟 / 日期 / 电量 / 签名各一种颜色),
        // 方便和 wezterm 的截图逐段对比松紧。
        let right = crate::status::parse(
            concat!(
                "\u{1b}[38;2;77;107;254m \u{f0599} 晚上好",
                "\u{1b}[38;2;107;119;145m   ·   ",
                "\u{1b}[38;2;232;237;249m17:12",
                "\u{1b}[38;2;107;119;145m  11/09 周一  ",
                "\u{1b}[38;2;107;119;145m · ",
                "\u{1b}[38;2;15;157;88m \u{f0079} 100%",
                "\u{1b}[38;2;107;119;145m  ·  ",
                "\u{1b}[38;2;77;107;254mElaina-SakiOVO",
                "\u{1b}[0m  "
            ),
            &[Rgb { r: 128, g: 128, b: 128 }; 256],
        );
        let draw = TabBarDraw {
            items: &items,
            layout: &layout,
            colors: &cfg.tab_bar.colors,
            fancy: true,
            show_index: true,
            at_bottom: false,
            corner_radius: None,
            new_tab_hovered: false,
            window_hovered: None,
            button_color: None,
            button_gnome_style: false,
            maximized: false,
            left_status: &[],
            right_status: &right,
            status_scale,
            status_pad: 8.0,
            status_gap,
            status_space,
        };
        r.build_tab_bar(&mut frame, w, h, &draw);

        let mut buf = vec![0u32; (w * h) as usize];
        r.paint_cpu(&mut buf, w, h, &frame);

        let row_bytes = (w * 3 + 3) & !3;
        let mut out = Vec::new();
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(54 + row_bytes * h).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(w as i32).to_le_bytes());
        out.extend_from_slice(&(h as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(row_bytes * h).to_le_bytes());
        out.extend_from_slice(&[0u8; 16]);
        for y in (0..h).rev() {
            for x in 0..w {
                let v = buf[(y * w + x) as usize];
                out.push((v & 0xff) as u8);
                out.push(((v >> 8) & 0xff) as u8);
                out.push(((v >> 16) & 0xff) as u8);
            }
            for _ in w * 3..row_bytes {
                out.push(0);
            }
        }
        std::fs::write(path, &out).unwrap();
        crate::wlog!("tab bar frame -> {path} ({w}x{h})");
    }
}
