//! 图片解码缓存。
//!
//! wezterm-term 已经把三种图形协议解析成同一套东西:iTerm2 的
//! `OSC 1337 ; File=...`、Kitty 的 `APC G ...`(含分片传输)以及 sixel,
//! 最终都会在单元格属性上挂一个 `ImageCell` 附件,数据是 `Arc<ImageData>`
//! ——可能是编码后的 PNG/GIF/WebP 字节,也可能是裸 RGBA。
//!
//! 渲染侧只认 RGBA8,所以这里负责解码,并按**内容哈希**缓存:同一张图
//! (哪怕在多个标签、多行里各放一次)只解码一次,滚动时不会每帧重解。
//! 哈希是 `ImageData` 自带预算好的,取它不需要重新扫一遍数据。

use std::collections::HashMap;
use std::sync::Arc;
use termwiz::image::{ImageData, ImageDataType};

/// 单张图允许的最大像素数(64M 像素 ≈ 256MB RGBA)。
///
/// 这不是性能参数,是**安全边界**:一张几十 KB 的 PNG 可以声明
/// 50000x50000,解码时按 width*height*4 分配,内存直接吃光 —— Rust 里
/// 分配失败是 abort,不是可捕获的异常,窗口会当场消失。所以解码前先读
/// 头部尺寸,超限的图直接不显示(和 wezterm 对超大图的态度一致)。
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// 解码后的 RGBA8 位图(未预乘 alpha)。
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// 长度恒为 width * height * 4,行优先,左上角起
    pub pixels: Arc<Vec<u8>>,
}

impl DecodedImage {
    pub fn byte_len(&self) -> usize {
        self.pixels.len()
    }

    /// 等比缩到不超过 `max` 像素边长。
    ///
    /// GPU 纹理有尺寸上限(常见 8192),而终端里偶尔真会出现更大的图
    /// (长截图、PDF 转出来的图)。超限时在 CPU 侧缩一次,总比整张不画好。
    pub fn fit_within(&self, max: u32) -> Arc<DecodedImage> {
        let max = max.max(1);
        if self.width <= max && self.height <= max {
            return Arc::new(Self {
                width: self.width,
                height: self.height,
                pixels: self.pixels.clone(),
            });
        }
        let scale = max as f32 / self.width.max(self.height) as f32;
        let w = ((self.width as f32 * scale).round() as u32).max(1);
        let h = ((self.height as f32 * scale).round() as u32).max(1);
        let Some(buf) = image::RgbaImage::from_raw(self.width, self.height, self.pixels.as_ref().clone())
        else {
            return Arc::new(Self {
                width: self.width,
                height: self.height,
                pixels: self.pixels.clone(),
            });
        };
        let small = image::imageops::resize(&buf, w, h, image::imageops::FilterType::Triangle);
        Arc::new(Self {
            width: w,
            height: h,
            pixels: Arc::new(small.into_raw()),
        })
    }
}

/// 按内容哈希缓存的解码结果。
///
/// 失败的解码也会被记住(存 `None`),否则一张坏图会在每一帧重试解码。
pub struct ImageCache {
    map: HashMap<[u8; 32], Option<Arc<DecodedImage>>>,
    /// 使用顺序:队首最久没被碰到,超预算时从队首淘汰
    order: Vec<[u8; 32]>,
    bytes: usize,
    limit: usize,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new(256)
    }
}

impl ImageCache {
    /// `limit_mb` 是解码后位图的总预算(不是压缩后的大小)。
    pub fn new(limit_mb: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: Vec::new(),
            bytes: 0,
            limit: limit_mb.max(1) * 1024 * 1024,
        }
    }

    /// 命中时把 key 挪到队尾(最近使用)。已经缓存过"解码失败"也返回 None。
    pub fn get(&mut self, key: &[u8; 32]) -> Option<Arc<DecodedImage>> {
        let hit = self.map.get(key)?;
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            let k = self.order.remove(pos);
            self.order.push(k);
        }
        hit.clone()
    }

    /// 只读查询,不更新 LRU。光栅化在 `&self` 上进行,而可见图每帧都会被
    /// `build_frame` 触碰一次,所以不会被淘汰,这里不必改顺序。
    pub fn cached(&self, key: &[u8; 32]) -> Option<Arc<DecodedImage>> {
        self.map.get(key)?.clone()
    }

    /// 拿到某张图的 RGBA;第一次见时解码并入缓存。
    pub fn decode(&mut self, data: &ImageData) -> Option<Arc<DecodedImage>> {
        let key = data.hash();
        if let Some(hit) = self.get(&key) {
            return Some(hit);
        }
        if self.map.contains_key(&key) {
            // 之前解过但失败了
            return None;
        }

        let decoded = decode_image(data);
        if let Some(img) = &decoded {
            self.bytes += img.byte_len();
        }
        self.map.insert(key, decoded.clone());
        self.order.push(key);
        self.evict();
        decoded
    }

    /// 放入一张已经解码好的位图(彩色 emoji 用:源是字体里的 PNG)。
    /// 与 `decode` 走同一套 LRU/预算,CPU 和 GPU 后端都能按 key 取到。
    pub fn insert(&mut self, key: [u8; 32], img: Arc<DecodedImage>) {
        if let Some(old) = self.map.insert(key, Some(img.clone())) {
            if let Some(old) = old {
                self.bytes = self.bytes.saturating_sub(old.byte_len());
            }
        } else {
            self.order.push(key);
        }
        self.bytes += img.byte_len();
        self.evict();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.map.values().filter(|v| v.is_some()).count()
    }

    fn evict(&mut self) {
        while self.bytes > self.limit && self.order.len() > 1 {
            let victim = self.order.remove(0);
            if let Some(Some(img)) = self.map.remove(&victim) {
                self.bytes = self.bytes.saturating_sub(img.byte_len());
                // 最后一个 Arc 也在这里释放,位图内存随之归还
            }
        }
    }
}

/// `ImageData` → RGBA8。动画(GIF/APNG)只取第一帧:
/// 终端里逐帧播放会把重绘节流打乱,收益远小于代价。
fn decode_image(data: &ImageData) -> Option<Arc<DecodedImage>> {
    let guard = data.data();
    match &*guard {
        ImageDataType::Rgba8 {
            data,
            width,
            height,
            ..
        } => {
            let want = (*width as usize).checked_mul(*height as usize)?.checked_mul(4)?;
            if data.len() != want {
                return None;
            }
            // 裸 RGBA 已经在内存里了,但这里还要 clone 一份给渲染侧,
            // 超大图同样会把内存顶爆 —— 一并挡掉。
            if *width as u64 * *height as u64 > MAX_PIXELS {
                crate::wlog!("[VectorTerm] 图片过大,已跳过:{width}x{height}");
                return None;
            }
            Some(Arc::new(DecodedImage {
                width: *width,
                height: *height,
                pixels: Arc::new(data.clone()),
            }))
        }
        ImageDataType::AnimRgba8 {
            width,
            height,
            frames,
            ..
        } => {
            let first = frames.first()?;
            let want = (*width as usize).checked_mul(*height as usize)?.checked_mul(4)?;
            if first.len() != want {
                return None;
            }
            Some(Arc::new(DecodedImage {
                width: *width,
                height: *height,
                pixels: Arc::new(first.clone()),
            }))
        }
        ImageDataType::EncodedFile(bytes) => decode_bytes(bytes),
        ImageDataType::EncodedLease(lease) => decode_bytes(&lease.get_data().ok()?),
    }
}

fn decode_bytes(bytes: &[u8]) -> Option<Arc<DecodedImage>> {
    // 先只读头部拿尺寸,再决定要不要真的解码。`load_from_memory` 会按
    // width*height*4 一次分配完,没有上限的话一个"解压炸弹"就能把进程
    // 干掉(分配失败在 Rust 里是 abort,拦不住)。
    let probe = image::io::Reader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let (pw, ph) = probe.into_dimensions().ok()?;
    if pw == 0 || ph == 0 || pw as u64 * ph as u64 > MAX_PIXELS {
        crate::wlog!("[VectorTerm] 图片过大,已跳过:{pw}x{ph}");
        return None;
    }
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    Some(Arc::new(DecodedImage {
        width,
        height,
        pixels: Arc::new(img.into_raw()),
    }))
}

// ---------------------------------------------------------------------------
// 测试:编码图解码、裸 RGBA 直通、失败缓存、容量淘汰
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// 生成一张 2x2 PNG:左上红、右上绿、左下蓝、右下白
    fn sample_png() -> Vec<u8> {
        let mut img = image::RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        img.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn decodes_png_to_rgba() {
        let png = sample_png();
        let data = ImageData::with_data(ImageDataType::EncodedFile(png.clone()));
        let mut cache = ImageCache::default();
        let img = cache.decode(&data).expect("应该能解码");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(&img.pixels[0..4], &[255, 0, 0, 255], "左上角应是红色");
        assert_eq!(&img.pixels[12..16], &[255, 255, 255, 255], "右下角应是白色");
        assert_eq!(cache.len(), 1);
        assert!(cache.bytes() >= 16);
    }

    #[test]
    fn second_decode_hits_cache() {
        let data = ImageData::with_data(ImageDataType::EncodedFile(sample_png()));
        let mut cache = ImageCache::default();
        let a = cache.decode(&data).unwrap();
        let b = cache.decode(&data).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "同一张图不应重复解码");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn raw_rgba_straight_through() {
        let data = ImageData::with_data(ImageDataType::new_single_frame(
            1,
            1,
            vec![10, 20, 30, 40],
        ));
        let mut cache = ImageCache::default();
        let img = cache.decode(&data).unwrap();
        assert_eq!(&*img.pixels, &[10, 20, 30, 40]);
    }

    #[test]
    fn bad_data_is_remembered_as_failure() {
        let data = ImageData::with_data(ImageDataType::EncodedFile(b"not an image".to_vec()));
        let mut cache = ImageCache::default();
        assert!(cache.decode(&data).is_none());
        assert!(cache.decode(&data).is_none(), "失败也要缓存,不能每帧重试");
        assert_eq!(cache.len(), 0, "失败项不计入有效条目");
    }

    #[test]
    fn oversized_images_are_downscaled_to_fit() {
        let data = ImageData::with_data(ImageDataType::new_single_frame(
            64,
            32,
            vec![255u8; 64 * 32 * 4],
        ));
        let mut cache = ImageCache::default();
        let img = cache.decode(&data).unwrap();
        let small = img.fit_within(16);
        assert_eq!((small.width, small.height), (16, 8));
        assert_eq!(small.pixels.len(), 16 * 8 * 4);
        // 没超限时应当直接复用同一份像素
        let same = img.fit_within(64);
        assert!(Arc::ptr_eq(&same.pixels, &img.pixels));
    }

    #[test]
    fn cache_evicts_to_stay_under_budget() {
        // 每张 100x100 RGBA = 40KB,预算 1MB 时最多 26 张
        let mut cache = ImageCache::new(1);
        for i in 0..40u32 {
            let mut px = vec![0u8; 100 * 100 * 4];
            px[0] = i as u8;
            px[1] = (i >> 8) as u8;
            let data = ImageData::with_data(ImageDataType::new_single_frame(100, 100, px));
            cache.decode(&data);
        }
        assert!(cache.bytes() <= 1024 * 1024, "缓存总量应受预算约束");
        assert!(cache.len() < 40, "超出预算的旧图应被淘汰");
    }
}
