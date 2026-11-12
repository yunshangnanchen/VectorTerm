//! 彩色 emoji:CBDT/CBLC 位图字体 → RGBA 图片。
//!
//! 内嵌的 `NotoColorEmoji.ttf` 是 **CBDT** 构建:每个 emoji 是一张 PNG,
//! 存在 `CBDT` 表里,`CBLC` 负责把 glyph id 映射到那张 PNG 的偏移。
//! ab_glyph 只认 glyf 轮廓,拿这种字体当普通回退字体用会一无所获 ——
//! 所以彩色 emoji 走独立路径:取出 PNG → 解码 → 缩到当前字号 → 塞进
//! `Frame::image` 那套图片缓存(CPU/GPU 两端都已支持)。
//!
//! 为什么不是 wezterm 那份:wezterm 打包的是 Noto Color Emoji 的 **COLRv1**
//! 构建(渐变 + 变换 + 合成,十来种 Paint 结构),运行时要完整实现一套
//! 矢量彩色渲染器。这里改打包同一字体家族的官方 CBDT 构建,显示效果一致,
//! 解析只需一张 strike 表。字体来源与许可证见 `assets/fonts/README.md`。

use crate::images::DecodedImage;
use image::imageops::FilterType;
use std::sync::Arc;

fn u8at(b: &[u8], off: usize) -> Option<u8> {
    b.get(off).copied()
}

fn u16be(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(off)?, *b.get(off + 1)?]))
}

fn u32be(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(off)?,
        *b.get(off + 1)?,
        *b.get(off + 2)?,
        *b.get(off + 3)?,
    ]))
}

/// sfnt 表目录 → (表在文件里的绝对偏移, 长度)
fn table(font: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
    let n = u16be(font, 4)? as usize;
    for i in 0..n {
        let rec = 12 + i * 16;
        if font.get(rec..rec + 4)? == tag {
            let off = u32be(font, rec + 8)? as usize;
            let len = u32be(font, rec + 12)? as usize;
            if off.checked_add(len)? <= font.len() {
                return Some((off, len));
            }
        }
    }
    None
}

/// CBLC 里一个 indexSubTable 的索引信息
struct SubTable {
    first: u16,
    last: u16,
    image_format: u16,
    /// CBDT 内:该子表所有位图数据的起点
    image_offset: u32,
    /// sbitOffsets 数组在字体文件里的绝对偏移
    offsets_at: usize,
}

struct Strike {
    ppem: u8,
    cbdt: usize,
    subs: Vec<SubTable>,
}

/// 缩放到目标字号后的一张彩色字形
pub struct EmojiGlyph {
    pub image: DecodedImage,
    /// 位图顶边到基线的距离(已按目标字号缩放)
    pub bearing_y: f32,
    /// 自然步进(已按目标字号缩放)
    pub advance: f32,
}

/// 便宜的预筛:明显不是 emoji 的字符(ASCII / CJK / 拉丁)直接跳过,
/// 免得每个汉字都去解析一遍 cmap。
pub fn maybe_emoji(ch: char) -> bool {
    let c = ch as u32;
    matches!(c,
        0x1F000..=0x1FAFF          // 绝大多数 emoji(含旗帜、肤色相关码点)
        | 0x2600..=0x27BF          // 杂项符号 / 装饰符号
        | 0x2B00..=0x2BFF
        | 0x2194..=0x21AA
        | 0x231A..=0x23FA
        | 0x25AA..=0x25FE
        | 0x2934..=0x2935
        | 0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139
        | 0x3030 | 0x303D | 0x3297 | 0x3299
        | 0xFE0F | 0x200D          // 变体选择符 / ZWJ(占位,无位图)
    )
}

pub struct EmojiFont {
    bytes: &'static [u8],
    strike: Strike,
}

impl EmojiFont {
    pub fn load(bytes: &'static [u8]) -> Option<Self> {
        let (cblc, _) = table(bytes, b"CBLC")?;
        let (cbdt, _) = table(bytes, b"CBDT")?;
        let num_sizes = u32be(bytes, cblc + 4)? as usize;
        let mut best: Option<Strike> = None;
        for i in 0..num_sizes.min(16) {
            let b = cblc + 8 + i * 48;
            let array_off = u32be(bytes, b)? as usize;
            let num_subs = u32be(bytes, b + 8)? as usize;
            let ppem = u8at(bytes, b + 44)?;
            let arr = cblc.checked_add(array_off)?;
            let mut subs = Vec::new();
            for k in 0..num_subs.min(4096) {
                let e = arr + k * 8;
                let first = u16be(bytes, e)?;
                let last = u16be(bytes, e + 2)?;
                let add = u32be(bytes, e + 4)? as usize;
                let sub = arr.checked_add(add)?;
                // 只认 indexFormat 1(变长偏移数组),CBDT 版 Noto 用的就是它
                if u16be(bytes, sub)? != 1 {
                    continue;
                }
                subs.push(SubTable {
                    first,
                    last,
                    image_format: u16be(bytes, sub + 2)?,
                    image_offset: u32be(bytes, sub + 4)?,
                    offsets_at: sub + 8,
                });
            }
            if subs.is_empty() {
                continue;
            }
            if best.as_ref().map(|s| ppem > s.ppem).unwrap_or(true) {
                best = Some(Strike { ppem, cbdt, subs });
            }
        }
        Some(Self {
            bytes,
            strike: best?,
        })
    }

    pub fn has(&self, ch: char) -> bool {
        self.png(ch).is_some()
    }

    /// 字符 → 该 emoji 的 PNG 字节与位图度量
    fn png(&self, ch: char) -> Option<(&'static [u8], Metrics)> {
        // 变体选择符 / ZWJ 只是组合控制符,本身没有位图;别把占位方块画出来
        if !maybe_emoji(ch) || matches!(ch, '\u{FE0F}' | '\u{200D}') {
            return None;
        }
        let face = ttf_parser::Face::parse(self.bytes, 0).ok()?;
        let gid = face.glyph_index(ch)?.0;
        let sub = self
            .strike
            .subs
            .iter()
            .find(|s| s.first <= gid && gid <= s.last)?;
        let j = (gid - sub.first) as usize;
        let off = u32be(self.bytes, sub.offsets_at + j * 4)?;
        let off_next = u32be(self.bytes, sub.offsets_at + (j + 1) * 4)?;
        if off == off_next {
            return None; // 这个 glyph 没有位图
        }
        let base = self
            .strike
            .cbdt
            .checked_add(sub.image_offset as usize)?
            .checked_add(off as usize)?;
        // imageFormat 17/18 = small/big metrics + u32 长度 + PNG
        let metrics_len = match sub.image_format {
            17 => 5,
            18 => 8,
            _ => return None,
        };
        let m = Metrics::read(self.bytes, base)?;
        let len_off = base + metrics_len;
        let data_len = u32be(self.bytes, len_off)? as usize;
        let png_at = len_off + 4;
        let png = self.bytes.get(png_at..png_at.checked_add(data_len)?)?;
        if !png.starts_with(b"\x89PNG") {
            return None;
        }
        Some((png, m))
    }

    /// 按 `em_px` 解码并缩放。返回的字形可以直接交给 `Frame::image`
    /// 按原尺寸绘制(不需要后端再拉伸,位图质量由 Lanczos3 保证)。
    pub fn rasterize(&self, ch: char, em_px: f32) -> Option<EmojiGlyph> {
        let (png, m) = self.png(ch)?;
        let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
        let rgba = img.to_rgba8();
        if rgba.width() == 0 || rgba.height() == 0 {
            return None;
        }
        let scale = em_px / self.strike.ppem.max(1) as f32;
        let nw = (rgba.width() as f32 * scale).round().max(1.0) as u32;
        let nh = (rgba.height() as f32 * scale).round().max(1.0) as u32;
        let scaled = if nw == rgba.width() && nh == rgba.height() {
            rgba
        } else {
            image::imageops::resize(&rgba, nw, nh, FilterType::Lanczos3)
        };
        Some(EmojiGlyph {
            image: DecodedImage {
                width: nw,
                height: nh,
                pixels: Arc::new(scaled.into_raw()),
            },
            bearing_y: m.bearing_y as f32 * scale,
            advance: m.advance as f32 * scale,
        })
    }
}

struct Metrics {
    bearing_y: i8,
    advance: u8,
}

impl Metrics {
    fn read(b: &[u8], at: usize) -> Option<Self> {
        // smallMetrics: height, width, bearingX, bearingY, advance
        // bigMetrics  : height, width, horiBearingX/Y, advance, vertBearingX/Y, vertAdvance
        let bearing_y = *b.get(at + 3)? as i8;
        // horiAdvance 在 smallMetrics/bigMetrics 里都是第 5 个字节
        let advance = *b.get(at + 4)?;
        Some(Self {
            bearing_y,
            advance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> EmojiFont {
        EmojiFont::load(crate::builtin_fonts::EMOJI_BYTES)
            .expect("内嵌 Noto Color Emoji 应能解析 CBLC/CBDT")
    }

    #[test]
    fn finds_png_for_common_emoji() {
        let f = font();
        assert_eq!(f.strike.ppem, 109);
        for ch in ['🛑', '😀', '✅', '🎉', '🔥'] {
            assert!(f.has(ch), "{ch} 应该有位图");
        }
        assert!(!f.has('A'), "普通 ASCII 不该被当成彩色 emoji");
    }

    #[test]
    fn rasterizes_to_colored_pixels() {
        let f = font();
        let g = f.rasterize('😀', 15.0).expect("应能栅格化");
        assert!(g.image.width > 0 && g.image.height > 0);
        assert_eq!(
            g.image.pixels.len(),
            g.image.width as usize * g.image.height as usize * 4
        );
        let opaque = g.image.pixels.chunks_exact(4).filter(|c| c[3] > 200).count();
        let colored = g
            .image
            .pixels
            .chunks_exact(4)
            .filter(|c| c[3] > 200 && (c[0] != c[1] || c[1] != c[2]))
            .count();
        println!(
            "emoji 位图 {}x{}, 不透明 {opaque}px, 彩色 {colored}px, bearing_y={}, advance={}",
            g.image.width, g.image.height, g.bearing_y, g.advance
        );
        assert!(opaque > 20, "应该画出成片像素");
        assert!(colored > 20, "应该是彩色的");
    }
}
