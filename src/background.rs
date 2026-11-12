//! 自定义背景:图片解码 → 降采样 → HSB 调整 → 模糊。
//!
//! 预处理只在**加载时做一次**,产出的 [`DecodedImage`] 同时交给 CPU 与 GPU
//! 两条渲染路径使用 —— 这样"切后端背景就变了"这种分叉从源头就不存在。
//!
//! 模糊用的是可分离 box blur(横一趟、纵一趟,重复两遍近似高斯):复杂度
//! O(w·h),与半径无关。4K 壁纸 + 半径 24 大约 20ms,只在上传时付一次。

use crate::config::{BackgroundConfig, ImageFill};
use crate::images::DecodedImage;
use std::path::Path;
use std::sync::Arc;

/// 预处理后允许的最大边长。
///
/// 背景最终是被缩放到窗口大小的,留到 2560 已经够 1440p 全屏;再大只是
/// 多占内存和模糊时间。超限时先缩再模糊,半径按比例折算,视觉半径不变。
const MAX_EDGE: u32 = 2560;

/// 加载并预处理背景图。失败返回可读原因(调用方打印后继续用纯色)。
pub fn load(path: &Path, cfg: &BackgroundConfig) -> Result<Arc<DecodedImage>, String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut rgba = img.to_rgba8();
    let (src_w, src_h) = rgba.dimensions();

    // 先降采样:模糊和后续采样都在这个尺寸上做
    let scale = if src_w.max(src_h) > MAX_EDGE {
        MAX_EDGE as f32 / src_w.max(src_h) as f32
    } else {
        1.0
    };
    if scale < 1.0 {
        let w = ((src_w as f32 * scale) as u32).max(1);
        let h = ((src_h as f32 * scale) as u32).max(1);
        rgba = image::imageops::resize(&rgba, w, h, image::imageops::FilterType::Triangle);
    }
    let (w, h) = rgba.dimensions();
    let mut pixels = rgba.into_raw();

    // HSB:亮度/饱和度/色相。半径也按同一次缩放折算,保证视觉大小一致
    apply_hsb(&mut pixels, cfg.brightness, cfg.saturation, cfg.hue);

    let radius = (cfg.blur * scale).round() as usize;
    if radius > 0 {
        blur(&mut pixels, w as usize, h as usize, radius);
    }

    Ok(Arc::new(DecodedImage {
        width: w,
        height: h,
        pixels: Arc::new(pixels),
    }))
}

/// 亮度乘数 → 饱和度 → 色相旋转。
///
/// 顺序有讲究:先压亮度再调饱和度,暗背景不会因为饱和度放大而发脏;
/// 色相放最后,避免亮部被色相旋转拉出偏色。
fn apply_hsb(pixels: &mut [u8], brightness: f32, saturation: f32, hue: f32) {
    let (b, s, hue) = (brightness, saturation, hue);
    let plain = (b - 1.0).abs() < 1e-4 && (s - 1.0).abs() < 1e-4 && (hue - 1.0).abs() < 1e-4;
    if plain {
        return;
    }
    for px in pixels.chunks_exact_mut(4) {
        let mut r = px[0] as f32 / 255.0;
        let mut g = px[1] as f32 / 255.0;
        let mut bl = px[2] as f32 / 255.0;

        if (b - 1.0).abs() > 1e-4 {
            r *= b;
            g *= b;
            bl *= b;
        }
        if (s - 1.0).abs() > 1e-4 {
            // 按 Rec.709 亮度拉到灰,再按饱和度插值回去
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * bl;
            r = luma + (r - luma) * s;
            g = luma + (g - luma) * s;
            bl = luma + (bl - luma) * s;
        }
        if (hue - 1.0).abs() > 1e-4 {
            let (hh, ss, vv) = rgb_to_hsv(r, g, bl);
            let (r2, g2, b2) = hsv_to_rgb((hh * hue).rem_euclid(1.0), ss, vv);
            r = r2;
            g = g2;
            bl = b2;
        }
        px[0] = (r.clamp(0.0, 1.0) * 255.0).round() as u8;
        px[1] = (g.clamp(0.0, 1.0) * 255.0).round() as u8;
        px[2] = (bl.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d.abs() < 1e-6 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max.abs() < 1e-6 { 0.0 } else { d / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match (i as i32).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

/// 可分离 box blur 跑两遍(≈ 高斯)。半径 0 直接返回。
pub fn blur(pixels: &mut [u8], w: usize, h: usize, radius: usize) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let r = radius.min(w.min(h).saturating_sub(1));
    if r == 0 {
        return;
    }
    let mut tmp = vec![0u8; pixels.len()];
    for _ in 0..2 {
        blur_h(pixels, &mut tmp, w, h, r);
        blur_v(&tmp, pixels, w, h, r);
    }
}

/// 横向 box blur。用前缀和算窗口均值,边界处窗口自然缩短(不复制边缘像素),
/// 复杂度 O(w·h·4),与半径无关。
fn blur_h(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let mut pre = vec![[0u32; 4]; w + 1];
    for y in 0..h {
        let base = y * w * 4;
        pre[0] = [0; 4];
        for x in 0..w {
            for c in 0..4 {
                pre[x + 1][c] = pre[x][c] + src[base + x * 4 + c] as u32;
            }
        }
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r + 1).min(w);
            let n = (hi - lo) as u32;
            for c in 0..4 {
                dst[base + x * 4 + c] = ((pre[hi][c] - pre[lo][c]) / n) as u8;
            }
        }
    }
}

/// 纵向 box blur,同样走前缀和。
fn blur_v(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let stride = w * 4;
    let mut pre = vec![[0u32; 4]; h + 1];
    for x in 0..w {
        let col = x * 4;
        pre[0] = [0; 4];
        for y in 0..h {
            for c in 0..4 {
                pre[y + 1][c] = pre[y][c] + src[col + y * stride + c] as u32;
            }
        }
        for y in 0..h {
            let lo = y.saturating_sub(r);
            let hi = (y + r + 1).min(h);
            let n = (hi - lo) as u32;
            for c in 0..4 {
                dst[col + y * stride + c] = ((pre[hi][c] - pre[lo][c]) / n) as u8;
            }
        }
    }
}

/// 背景图在画布上的**完整**目标矩形。
///
/// 故意允许超出画布:CPU 端把它交给 `blit_image`,内部会按画布裁掉越界
/// 部分,而缩放比例仍按完整矩形算 —— 这正是 cover 要的效果。
pub fn dst_rect(fill: ImageFill, iw: f32, ih: f32, cw: f32, ch: f32) -> [f32; 4] {
    if iw <= 0.0 || ih <= 0.0 || cw <= 0.0 || ch <= 0.0 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    match fill {
        ImageFill::Stretch => [0.0, 0.0, cw, ch],
        ImageFill::Cover => {
            let s = (cw / iw).max(ch / ih);
            let (w, h) = (iw * s, ih * s);
            [(cw - w) * 0.5, (ch - h) * 0.5, w, h]
        }
        ImageFill::Contain => {
            let s = (cw / iw).min(ch / ih);
            let (w, h) = (iw * s, ih * s);
            [(cw - w) * 0.5, (ch - h) * 0.5, w, h]
        }
        ImageFill::Center | ImageFill::Tile => [
            ((cw - iw) * 0.5).round(),
            ((ch - ih) * 0.5).round(),
            iw,
            ih,
        ],
    }
}

/// 平铺时每块的左上角。非 tile 返回空(调用方用 [`dst_rect`] 画一张)。
///
/// `max` 是上限:窗口特别大而图特别小时,平铺块数会爆炸,超过就退化成
/// cover —— 背景图铺得再密也不值得为它画几千个四边形。
pub fn tile_origins(iw: f32, ih: f32, cw: f32, ch: f32, max: usize) -> Vec<[f32; 2]> {
    if iw < 1.0 || ih < 1.0 {
        return Vec::new();
    }
    let cols = (cw / iw).ceil().max(1.0) as usize;
    let rows = (ch / ih).ceil().max(1.0) as usize;
    if cols.saturating_mul(rows) > max {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        for c in 0..cols {
            out.push([c as f32 * iw, r as f32 * ih]);
        }
    }
    out
}

/// 把可能超出画布的矩形收敛到画布内,并反算出对应的 uv 区间。
///
/// GPU 端只能画可见的那一块(否则 uv 0..1 会把整张图压进可见区域,
/// cover 就被拉伸了);CPU 端不需要这个,直接把完整矩形交给 `blit_image`。
pub fn clip_uv(rect: [f32; 4], cw: f32, ch: f32) -> Option<([f32; 4], [f32; 4])> {
    let (x, y, w, h) = (rect[0], rect[1], rect[2], rect[3]);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let x0 = x.max(0.0);
    let y0 = y.max(0.0);
    let x1 = (x + w).min(cw);
    let y1 = (y + h).min(ch);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let uv = [
        (x0 - x) / w,
        (y0 - y) / h,
        (x1 - x) / w,
        (y1 - y) / h,
    ];
    Some(([x0, y0, x1 - x0, y1 - y0], uv))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_fills_and_crops() {
        // 100×100 的图铺进 200×100 的画布:放大 2 倍,纵向不变,横向居中
        let r = dst_rect(ImageFill::Cover, 100.0, 100.0, 200.0, 100.0);
        assert_eq!(r, [0.0, -50.0, 200.0, 200.0]);
        // 纵向超出 100,居中后上边 -50
        let r = dst_rect(ImageFill::Cover, 200.0, 100.0, 100.0, 100.0);
        assert_eq!(r, [-50.0, 0.0, 200.0, 100.0]);
    }

    #[test]
    fn contain_leaves_margins() {
        let r = dst_rect(ImageFill::Contain, 100.0, 100.0, 200.0, 100.0);
        assert_eq!(r, [50.0, 0.0, 100.0, 100.0]);
    }

    #[test]
    fn clip_uv_maps_visible_part() {
        // 完整矩形 -50..150 落在 0..100 的画布里:可见部分对应 uv 0.25..0.75
        let (rect, uv) = clip_uv([-50.0, 0.0, 200.0, 100.0], 100.0, 100.0).unwrap();
        assert_eq!(rect, [0.0, 0.0, 100.0, 100.0]);
        assert!((uv[0] - 0.25).abs() < 1e-6);
        assert!((uv[2] - 0.75).abs() < 1e-6);
        assert!((uv[1] - 0.0).abs() < 1e-6);
        assert!((uv[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fully_outside_is_dropped() {
        assert!(clip_uv([200.0, 0.0, 100.0, 100.0], 100.0, 100.0).is_none());
    }

    #[test]
    fn tile_origins_covers_canvas() {
        let o = tile_origins(32.0, 32.0, 100.0, 70.0, 1000);
        assert_eq!(o.len(), 4 * 3); // ceil(100/32)=4 列,ceil(70/32)=3 行
        assert_eq!(o[0], [0.0, 0.0]);
        assert_eq!(o[1], [32.0, 0.0]);
    }

    #[test]
    fn tile_gives_up_when_too_many() {
        // 1px 的图铺 100×100 要 10000 块,超过上限就返回空(退化成 cover)
        assert!(tile_origins(1.0, 1.0, 100.0, 100.0, 500).is_empty());
    }

    #[test]
    fn blur_keeps_size_and_softens_edge() {
        let (w, h) = (8usize, 8usize);
        let mut px = vec![0u8; w * h * 4];
        // 左半边白、右半边黑
        for y in 0..h {
            for x in 0..w {
                let v = if x < w / 2 { 255 } else { 0 };
                let i = (y * w + x) * 4;
                px[i] = v;
                px[i + 1] = v;
                px[i + 2] = v;
                px[i + 3] = 255;
            }
        }
        blur(&mut px, w, h, 2);
        // 分界处不再是硬边,但远离分界的区域要保住原亮度
        let at = |x: usize, y: usize| px[(y * w + x) * 4] as i32;
        assert!(at(3, 4) > 0 && at(3, 4) < 255, "边界应被抹开:{}", at(3, 4));
        assert!(at(0, 4) > 200, "白区边缘不该被抹成灰:{}", at(0, 4));
        assert!(at(7, 4) < 40, "黑区边缘应接近黑:{}", at(7, 4));
        // 纵向:同一列逐行一致(横向的渐变不应被纵向模糊改变)
        assert_eq!(at(0, 4), at(0, 0));
        assert_eq!(at(7, 4), at(7, 7));
        // 单调递减
        for x in 1..w {
            assert!(at(x, 4) <= at(x - 1, 4), "x={x} 处不是单调递减");
        }
    }

    #[test]
    fn hsb_brightness_and_grayscale() {
        let mut px = vec![200u8, 100, 50, 255];
        apply_hsb(&mut px, 0.5, 1.0, 1.0);
        assert_eq!(&px[..3], &[100, 50, 25]);

        let mut px = vec![200u8, 100, 50, 255];
        apply_hsb(&mut px, 1.0, 0.0, 1.0);
        // 饱和度 0 = 灰度,三通道相等
        assert_eq!(px[0], px[1]);
        assert_eq!(px[1], px[2]);
    }
}
