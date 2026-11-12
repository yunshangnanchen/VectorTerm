//! 显示列表:布局只做一遍,CPU 光栅化与 GPU 后端消费同一份绘制指令。
//!
//! 这样两条渲染路径的字形定位、下划线、光标、尾迹几何完全一致,不会出现
//! "换后端就长得不一样"的问题;CPU 路径退化为一次重放,GPU 路径则把
//! 矩形/字形转成实例化四边形。

use crate::config::ImageFill;
use crate::cursor::Point2;
use crate::images::DecodedImage;
use crate::render::Rgb;
use std::sync::Arc;

/// 背景层:一张已预处理好的位图 + 填充方式 + 不透明度。
///
/// 渐变也会被烘焙成 1×256 / 256×1 的小图走同一条路径 —— 这样两条渲染
/// 后端只需要实现"画一张图",渐变、壁纸、纯色的视觉必然一致。
#[derive(Clone)]
pub struct BgLayer {
    pub image: Arc<DecodedImage>,
    pub fill: ImageFill,
    pub alpha: f32,
}

#[derive(Clone, Copy, Debug)]
pub enum Item {
    /// 实心矩形:单元格背景、上/下划线、删除线、光标块
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgb,
        alpha: f32,
    },
    /// 圆角矩形:标签块、关闭按钮底色。
    /// `radius` 单位像素,绘制时夹到不超过短边的一半;0 退化为普通矩形。
    RoundRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: Rgb,
        alpha: f32,
    },
    /// 字形:引用渲染器字形表里的条目
    Glyph {
        id: u32,
        /// 源位图内的矩形(x, y, w, h)
        src: [f32; 4],
        /// 画布上的目标矩形(x, y, w, h)
        dst: [f32; 4],
        color: Rgb,
        italic: bool,
        /// 没有 Bold face 时的合成加粗(再描一遍)
        bold: bool,
    },
    /// Kitty 风格尾迹四边形(顺时针四角)+ 挖空区域
    Trail {
        corners: [Point2; 4],
        color: Rgb,
        alpha: f32,
        mask: [f32; 4],
    },
    /// 图片(iTerm2 / Kitty / sixel):`key` 是解码缓存的内容哈希,
    /// 位图由渲染器按 key 取,这里只保留位置 —— 每张图每帧只发一个四边形。
    Image {
        key: [u8; 32],
        /// 整张图在画布上的矩形(x, y, w, h)
        dst: [f32; 4],
    },
}

/// 鼠标选区高亮。坐标是**当前视口**的 (col, row),由 main 把绝对物理行号换算好。
/// 在背景层之后、字形层之前绘制,文字不会被染色。
#[derive(Clone, Copy, Debug)]
pub struct Highlight {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub color: Rgb,
    pub alpha: f32,
}

impl Highlight {
    /// 返回 (起点, 终点),先按行再按列排序
    pub fn normalized(&self) -> ((usize, usize), (usize, usize)) {
        if (self.end.1, self.end.0) < (self.start.1, self.start.0) {
            (self.end, self.start)
        } else {
            (self.start, self.end)
        }
    }
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// 内容区上边界:亚行滚动时上移的内容不能渗进 padding
    pub clip_top: f32,
    /// 内容区下边界(标签栏在下方时提前结束),图片等会越过网格的元素靠它收边
    pub clip_bottom: f32,
    /// 整屏底色
    pub clear: Rgb,
    /// 背景层,按数组顺序自下而上叠放(渐变在下、图片在上)。空 = 只用 `clear` 纯色。
    pub bg: Vec<BgLayer>,
    pub items: Vec<Item>,
    /// 尾迹在 items 里的位置(GPU 需要在这里切一次绘制批次)
    pub trail_at: Option<usize>,
    /// 叠加层(标签栏)在 items 里的起始下标。从这里开始的东西
    /// 不受 `clip_top` 裁剪 —— 标签栏本来就画在内容区之外。
    pub overlay_at: Option<usize>,
    /// 横向裁剪:从 `from` 号指令起,只画在 `[x0, x1)` 内。
    /// 到 `overlay_at`(标签栏)自动放开。
    pub clip_x: Option<ClipX>,
}

/// 横向裁剪区间,见 [`Frame::clip_x`]。
///
/// 竖直方向一直有 `clip_top` / `clip_bottom`,横向却从来没需要过 —— 直到侧边栏
/// 要做"从左边滑出来"。滑动意味着绘制矩形会跑到自己区域外面,而右邻可能是
/// 另一个窗格的终端,不裁就会看到面板糊在隔壁的字符上。所以这里给两条后端
/// 各加一条横向边界(CPU 夹 x 区间、GPU 收 scissor 宽度)。
#[derive(Clone, Copy, Debug)]
pub struct ClipX {
    /// 从 items 的哪个下标开始生效(之前的指令已经画完,管不着)
    pub from: usize,
    pub x0: f32,
    pub x1: f32,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            clip_top: 0.0,
            clip_bottom: f32::MAX,
            clear: Rgb { r: 0, g: 0, b: 0 },
            bg: Vec::new(),
            items: Vec::new(),
            trail_at: None,
            overlay_at: None,
            clip_x: None,
        }
    }
}

impl Frame {
    /// 开始一帧:清空指令,记住尺寸与底色。容量会复用,不重新分配。
    pub fn begin(&mut self, width: u32, height: u32, clip_top: f32, clear: Rgb) {
        self.width = width;
        self.height = height;
        self.clip_top = clip_top;
        self.clip_bottom = height as f32;
        self.clear = clear;
        self.bg.clear();
        self.items.clear();
        self.trail_at = None;
        self.overlay_at = None;
        self.clip_x = None;
    }

    /// 从**这一条指令起**开一条横向裁剪带:`[x0, x1)` 之外不画。
    ///
    /// 只影响之后压入的指令,已经画好的(比如窗格里的终端)不受影响;再往后
    /// 到 `overlay_at`(标签栏)自动放开,所以调用方只管在这里开,不用收。
    pub fn clip_from_here(&mut self, x0: f64, x1: f64) {
        self.clip_x = Some(ClipX {
            from: self.items.len(),
            x0: x0 as f32,
            x1: x1 as f32,
        });
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, color: Rgb, alpha: f32) {
        self.items.push(Item::Rect {
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
            color,
            alpha,
        });
    }

    /// 圆角矩形。半径 0 等价于 `rect`,所以调用方不必分支。
    pub fn round_rect(
        &mut self,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        radius: f64,
        color: Rgb,
        alpha: f32,
    ) {
        if radius < 0.5 {
            return self.rect(x, y, w, h, color, alpha);
        }
        self.items.push(Item::RoundRect {
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
            radius: radius as f32,
            color,
            alpha,
        });
    }

    pub fn glyph(
        &mut self,
        id: u32,
        src: (f64, f64, f64, f64),
        dst: (f64, f64, f64, f64),
        color: Rgb,
        italic: bool,
        bold: bool,
    ) {
        self.items.push(Item::Glyph {
            id,
            src: [src.0 as f32, src.1 as f32, src.2 as f32, src.3 as f32],
            dst: [dst.0 as f32, dst.1 as f32, dst.2 as f32, dst.3 as f32],
            color,
            italic,
            bold,
        });
    }

    /// 半透明四边形(kitty 光标尾迹)。
    ///
    /// * `corners` —— 四个角,**顺序固定为** 右上、右下、左下、左上
    ///   (见 [`crate::cursor::corners_of`]);
    /// * `mask` —— 要挖空的矩形(当前光标格),布局是 **[x0, y0, x1, y1]**,
    ///   两个后端都按 `mask.x..mask.z` 取 x、`mask.y..mask.w` 取 y。
    ///   注意它和 `corners` 的 x0/x1/y0/y1 不是同一个顺序 —— 弄反的后果是整个
    ///   四边形被当成挖空区,一像素都画不出来(GPU 分支也一样)。
    pub fn trail(
        &mut self,
        corners: [Point2; 4],
        color: Rgb,
        alpha: f32,
        mask: [f32; 4],
    ) {
        self.trail_at = Some(self.items.len());
        self.items.push(Item::Trail {
            corners,
            color,
            alpha,
            mask,
        });
    }

    /// 一整张图(不是一格一块)。两端渲染器都从 `key` 对应的缓存里取位图,
    /// 按 `dst` 缩放绘制,所以跨格的图不会出现接缝。
    pub fn image(&mut self, key: [u8; 32], x: f64, y: f64, w: f64, h: f64) {
        self.items.push(Item::Image {
            key,
            dst: [x as f32, y as f32, w as f32, h as f32],
        });
    }
}
