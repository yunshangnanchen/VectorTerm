//! GPU 渲染后端(wgpu / DX12)
//!
//! 思路和现代 GPU 终端一致:字形仍然在 CPU 上栅格化(ab_glyph),但位图放进
//! 一张**图集纹理**,屏幕上所有内容都用实例化四边形一次性画完 ——
//! 每格背景、每个字形、下划线都是同一个渲染管线里的一个实例,只有尾迹
//! 四边形因为四个角可以自由变形而单独走一条管线。
//!
//! 这样 4K 分辨率下每帧的 CPU 工作量只剩"拼顶点",不再有百万级像素的
//! 逐像素混合;而布局(字形位置/缩放)与 CPU 后端共用同一份显示列表,
//! 两条路径长得完全一样。

use crate::config::ImageFill;
use crate::frame::{Frame, Item};
use crate::render::Renderer;
use anyhow::{anyhow, Context};
use std::collections::HashMap;
use std::sync::Arc;
use winit::raw_window_handle::HasWindowHandle;
use winit::window::Window;

/// 图集尺寸(单通道,2048² 足够放几千个字形)
const ATLAS_W: u32 = 2048;
const ATLAS_H: u32 = 2048;
/// 字形之间留 1px 空隙,避免线性采样时采到邻居
const PAD: u32 = 1;

const QUAD_ATTRS: &[wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x2, 4 => Float32x2
];

/// 背景层的纹理身份:同一个 `Arc<Vec<u8>>` 指针就是同一张图。
/// 背景图在 `Renderer` 里长期存活,地址稳定,可以安全地当 key 用。
fn bg_key(img: &crate::images::DecodedImage) -> [u8; 32] {
    let mut k = [0u8; 32];
    let ptr = Arc::as_ptr(&img.pixels) as usize as u64;
    k[..8].copy_from_slice(&ptr.to_le_bytes());
    k[8..12].copy_from_slice(&img.width.to_le_bytes());
    k[12..16].copy_from_slice(&img.height.to_le_bytes());
    k
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    /// 目标矩形(x, y, w, h),像素
    rect: [f32; 4],
    /// 图集 uv(左上、右下)
    uv: [f32; 4],
    color: [f32; 4],
    /// x = 斜体剪切(顶部相对底部的像素偏移),y 保留
    params: [f32; 2],
    /// x = 圆角半径(像素,0 = 直角,**-1 = 图片实例**),y 保留
    extra: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    size: [f32; 2],
    /// 整窗不透明度:只给背景类填充的片元用(`fs_main_bg`)
    bg_alpha: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TrailGlobals {
    size: [f32; 2],
    _pad: [f32; 2],
    corners: [[f32; 4]; 4],
    mask: [f32; 4],
    color: [f32; 4],
}

#[derive(Clone, Copy)]
struct Slot {
    x: u32,
    y: u32,
}

/// 一张图片的 GPU 资源
struct ImageTex {
    bind: wgpu::BindGroup,
    /// 上传的字节数,用于总量控制(与 CPU 侧解码缓存各管一半预算)
    bytes: usize,
    last_used: u64,
}

/// 一批实例用的 bind group:字形/方块用图集,图片用自己的纹理
#[derive(Clone, Copy, PartialEq, Eq)]
enum BindRef {
    Atlas,
    Image([u8; 32]),
}

struct Seg {
    start: u32,
    end: u32,
    bind: BindRef,
    /// 这一段是不是"背景类"填充(见 `Frame::bg_items`),决定用哪条管线
    bg: bool,
}

/// 实例收集器。相邻且 bind 相同的实例合并成一段,切 bind / 切图层时断开,
/// 绘制阶段就能一段一个 `draw`,不用为每张图单开渲染通道。
struct Batches {
    insts: Vec<Instance>,
    segs: Vec<Seg>,
    force: bool,
}

impl Batches {
    fn new() -> Self {
        Self {
            insts: Vec::new(),
            segs: Vec::new(),
            force: true,
        }
    }

    /// 强制下一批实例开新段(尾迹、标签栏叠加层这类边界)
    fn cut(&mut self) {
        self.force = true;
    }

    fn push(&mut self, bind: BindRef, bg: bool, inst: Instance) {
        let start = self.insts.len() as u32;
        if !self.force {
            if let Some(last) = self.segs.last_mut() {
                if last.bind == bind && last.bg == bg {
                    last.end = start + 1;
                    self.insts.push(inst);
                    return;
                }
            }
        }
        self.force = false;
        self.segs.push(Seg {
            start,
            end: start + 1,
            bind,
            bg,
        });
        self.insts.push(inst);
    }
}

/// CPU 侧的图集:维护像素 + 货架式分配器
struct Atlas {
    data: Vec<u8>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    dirty: Option<(u32, u32, u32, u32)>,
}

impl Atlas {
    fn new() -> Self {
        Self {
            data: vec![0u8; (ATLAS_W * ATLAS_H) as usize],
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
            dirty: None,
        }
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let w = w + PAD * 2;
        let h = h + PAD * 2;
        if w > ATLAS_W {
            return None;
        }
        if self.shelf_x + w > ATLAS_W {
            // 换一行
            self.shelf_y += self.shelf_h;
            self.shelf_x = 0;
            self.shelf_h = 0;
        }
        if self.shelf_y + h > ATLAS_H {
            return None;
        }
        let (x, y) = (self.shelf_x, self.shelf_y);
        self.shelf_x += w;
        self.shelf_h = self.shelf_h.max(h);
        Some((x + PAD, y + PAD))
    }

    /// 写一个字形位图(单通道 alpha)
    fn put(&mut self, x: u32, y: u32, w: u32, h: u32, src: &[u8]) {
        for row in 0..h {
            let dst = ((y + row) * ATLAS_W + x) as usize;
            let s = (row * w) as usize;
            let n = w.min(src.len().saturating_sub(s) as u32) as usize;
            if n > 0 {
                self.data[dst..dst + n].copy_from_slice(&src[s..s + n]);
            }
        }
        let (x0, y0, x1, y1) = (x, y, x + w, y + h);
        self.dirty = Some(match self.dirty {
            None => (x0, y0, x1, y1),
            Some((ax, ay, bx, by)) => (ax.min(x0), ay.min(y0), bx.max(x1), by.max(y1)),
        });
    }
}

pub struct GpuRenderer {
    /// 有窗口时走交换链;测试里用离屏纹理
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    /// 背景类填充用的变体:不写目标 alpha(见 `fs_main_bg`)
    pipeline_bg: wgpu::RenderPipeline,
    trail_pipeline: wgpu::RenderPipeline,
    quad_bind: wgpu::BindGroup,
    trail_bind: wgpu::BindGroup,
    globals: wgpu::Buffer,
    trail_globals: wgpu::Buffer,
    instances: wgpu::Buffer,
    instance_cap: usize,
    atlas: Atlas,
    atlas_tex: wgpu::Texture,
    /// 白色像素的 uv,实心矩形用它采样
    white_uv: [f32; 2],
    slots: HashMap<u32, Option<Slot>>,
    /// 图片纹理:同一张图(按内容哈希)只上传一次。
    /// 每条目自带 bind group —— 布局与字形批次相同,只是把图集换成整张图。
    image_tex: HashMap<[u8; 32], ImageTex>,
    image_bind_bytes: usize,
    frame_seq: u64,
    quad_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    adapter_name: String,
    /// 离屏模式下的最后一帧(测试读回用)
    #[cfg_attr(not(test), allow(dead_code))]
    last_target: Option<wgpu::Texture>,
    /// 走 DirectComposition 时持有它(逐像素 alpha 的毛玻璃路线)。
    /// 不透明窗口 / 离屏渲染是 `None`。
    #[cfg(windows)]
    dcomp: Option<DcompTarget>,
    /// 清屏用的 alpha(背景层不透明度)。1.0 = 不透明。
    bg_alpha: f32,
}

/// 交换链的 alpha 模式。
///
/// `CreateSwapChainForHwnd` 不接受 `PREMULTIPLIED`:DXGI 直接返回
/// `0x887A0001`(和 `Scaling`/`SwapEffect` 的组合无关,四种都试过)。
/// 想要逐像素 alpha 只能换成 `CreateSwapChainForComposition`,把内容挂在
/// DirectComposition 的 visual 上 —— `dx12/adapter.rs` 那条路才会在
/// capabilities 里报 `PreMultiplied`。所以这里只看 caps 报了什么:
/// 报了预乘就用(说明走的是合成交换链),否则退回不透明。
fn pick_alpha_mode(
    caps: Option<&wgpu::SurfaceCapabilities>,
    want_transparent: bool,
) -> wgpu::CompositeAlphaMode {
    let Some(c) = caps else {
        return wgpu::CompositeAlphaMode::Auto;
    };
    if want_transparent && c.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
        return wgpu::CompositeAlphaMode::PreMultiplied;
    }
    if c.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
        return wgpu::CompositeAlphaMode::Opaque;
    }
    c.alpha_modes
        .first()
        .copied()
        .unwrap_or(wgpu::CompositeAlphaMode::Auto)
}

/// DirectComposition 的三件套:`CreateSwapChainForComposition` 出来的画面
/// 必须挂在 `IDCompositionVisual` 上才会被 DWM 合成。
///
/// 字段全是 `_` 前缀 —— 它们的存在本身就是"别提前 drop":visual 上挂着
/// 交换链的内容,target 上挂着 visual,device 上挂着 target。
#[cfg(windows)]
struct DcompTarget {
    _device: windows::Win32::Graphics::DirectComposition::IDCompositionDevice,
    _target: windows::Win32::Graphics::DirectComposition::IDCompositionTarget,
    visual: windows::Win32::Graphics::DirectComposition::IDCompositionVisual,
}

#[cfg(windows)]
impl DcompTarget {
    /// 给窗口挂一套 DComp:device → target(绑 hwnd) → visual → SetRoot → Commit。
    ///
    /// 同时把窗口标成 `WS_EX_NOREDIRECTIONBITMAP`。不加这个的话 DWM 会照常给
    /// 窗口分一块空白重定向位图,盖在合成内容上面 —— 表现就是"窗口在、内容是灰的"。
    fn attach(window: &Window) -> anyhow::Result<Self> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::DirectComposition::{
            DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
        };

        let hwnd = match window.window_handle()?.as_raw() {
            winit::raw_window_handle::RawWindowHandle::Win32(h) => h.hwnd.get(),
            _ => return Err(anyhow!("不是 Win32 窗口,无法挂 DirectComposition")),
        };
        unsafe {
            let hwnd = HWND(hwnd as _);
            let device: IDCompositionDevice = DCompositionCreateDevice(None)
                .context("DCompositionCreateDevice 失败(需要 Win8+)")?;
            let target: IDCompositionTarget = device
                .CreateTargetForHwnd(hwnd, true)
                .context("IDCompositionDevice::CreateTargetForHwnd 失败")?;
            let visual: IDCompositionVisual = device.CreateVisual().context("CreateVisual 失败")?;
            target.SetRoot(&visual).context("IDCompositionTarget::SetRoot 失败")?;

            // 注意:WS_EX_NOREDIRECTIONBITMAP 必须由创建窗口的那一方在
            // CreateWindowEx 时给(winit 的 `with_no_redirection_bitmap`),
            // 这里再改 ex-style 是无效的 —— DWM 早就决定好有没有重定向位图了。
            device.Commit().context("IDCompositionDevice::Commit 失败")?;

            Ok(Self {
                _device: device,
                _target: target,
                visual,
            })
        }
    }

    fn visual_ptr(&self) -> *mut std::ffi::c_void {
        use windows::core::Interface;
        self.visual.as_raw()
    }

    fn commit(&self) {
        let _ = unsafe { self._device.Commit() };
    }
}


const SHADER: &str = r#"
struct Globals {
    size: vec2<f32>,
    bg_alpha: f32,
    _pad: f32,
};
@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct Inst {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) params: vec2<f32>,
    @location(4) extra: vec2<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    /// 像素坐标:本顶点相对矩形左上角的偏移
    @location(2) local: vec2<f32>,
    /// 矩形宽、高、圆角半径(像素);半径取负表示这批实例采样的是整张图
    @location(3) box: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[vi];
    var p = inst.rect.xy + c * inst.rect.zw;
    // 斜体剪切:顶上偏得多,底下几乎不偏
    p.x = p.x + inst.params.x * (1.0 - c.y);
    var o: VsOut;
    o.pos = vec4<f32>(
        p.x / globals.size.x * 2.0 - 1.0,
        1.0 - p.y / globals.size.y * 2.0,
        0.0, 1.0,
    );
    o.uv = vec2<f32>(mix(inst.uv.x, inst.uv.z, c.x), mix(inst.uv.y, inst.uv.w, c.y));
    o.color = inst.color;
    o.local = vec2<f32>(c.x * inst.rect.z, c.y * inst.rect.w);
    o.box = vec3<f32>(inst.rect.z, inst.rect.w, inst.extra.x);
    return o;
}

/// 两条片元入口共用的着色:算出直通颜色的覆盖率结果。
/// 注意这里**不能**加 `@fragment` —— WGSL 不允许从别的函数调用入口函数。
fn fs_shade(in: VsOut) -> vec4<f32> {
    // 图片与字形共用一条管线,只是绑定的纹理不同:
    // 图片那批实例的 bind group 把 atlas 换成了整张图,uv 直接是 0..1。
    let s = textureSample(atlas, samp, in.uv);
    let a = s.r;
    var cov = 1.0;
    // 圆角矩形:用圆角盒的有符号距离算覆盖率,和 CPU 路径同一套公式
    if (in.box.z > 0.0) {
        let half = in.box.xy * 0.5;
        let r = min(in.box.z, min(half.x, half.y));
        let q = abs(in.local - half) - (half - vec2<f32>(r, r));
        let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
        cov = clamp(0.5 - d, 0.0, 1.0);
    }
    // 上一版这里写成"图片实例提前 return s",Intel 的 DXIL 编译器会把
    // 输出寄存器的 r 分量留着不覆盖,红通道恒为 1.0(整屏偏红)。
    // 改成单出口 + select,两条路径都显式写满四个分量。
    let tinted = vec4<f32>(in.color.r, in.color.g, in.color.b, in.color.a * a * cov);
    // 图片分支也乘 color.a:背景层靠它在不动纹理的前提下做半透明
    let img = vec4<f32>(s.rgb, s.a * in.color.a);
    return select(tinted, img, in.box.z < -0.5);
}

/// 内容:字形、图片、光标、选区、分隔线。alpha 原样累积,预乘输出。
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = fs_shade(in);
    // 预乘输出:混合状态是 One / OneMinusSrcAlpha
    return vec4<f32>(c.rgb * c.a, c.a);
}

/// 背景类填充:清屏色、背景层(渐变/壁纸)、单元格底色、标签栏底色。
///
/// 颜色整体乘一次 `bg_alpha`,但**不写 alpha** —— 这条管线的 alpha 混合是
/// `Zero / One`,目标 alpha 只有清屏写过的那一次。于是不管背景叠了几层
/// (壁纸 + 标签栏 = 两层),窗口的不透明度都只等于 `window_background_opacity`,
/// 不会变成 1-(1-a)^n 那种逐层累加。
@fragment
fn fs_main_bg(in: VsOut) -> @location(0) vec4<f32> {
    let c = fs_shade(in);
    return vec4<f32>(c.rgb * c.a * globals.bg_alpha, c.a);
}
"#;

const TRAIL_SHADER: &str = r#"
struct TrailGlobals {
    size: vec2<f32>,
    _pad: vec2<f32>,
    corners: array<vec4<f32>, 4>,
    mask: vec4<f32>,
    color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> tg: TrailGlobals;

@vertex
fn vs_trail(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var idx = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let p = tg.corners[idx[vi]].xy;
    return vec4<f32>(
        p.x / tg.size.x * 2.0 - 1.0,
        1.0 - p.y / tg.size.y * 2.0,
        0.0, 1.0,
    );
}

@fragment
fn fs_trail(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // 当前光标矩形挖空,由光标本体保持清晰
    if (pos.x >= tg.mask.x && pos.x < tg.mask.z && pos.y >= tg.mask.y && pos.y < tg.mask.w) {
        discard;
    }
    return tg.color;
}
"#;

// ---------------------------------------------------------------------------
// 适配器选择
//
// ghostty 的思路是"每个平台绑死一个 GPU API"(macOS Metal、Linux OpenGL),
// 初始化失败就明确报错;终端里出现"黑屏但能输入"是最糟的体验,所以它在
// 创建上下文时会依次试几档配置,并主动识别 llvmpipe/swrast 这类软件渲染,
// 而不是默默把帧率交给 CPU。
//
// Windows 上 wgpu 一次能摸到 DX12 / Vulkan / GL 三套后端,可以做得更宽容:
// 把适配器全枚举出来 → 按"后端可信度 + 设备类型"排序 → 逐个尝试创建设备,
// 第一个成功的就用;全都不行才退回 CPU 软光栅化(比 WARP 快,而且已经过
// 像素级对齐测试)。这样 AMD/NVIDIA/Intel 的核显独显、远程桌面、虚拟机
// 都能落在最合适的那一档上。
// ---------------------------------------------------------------------------

/// DX12 是 Windows 主力;Vulkan 兜住部分老驱动/异常驱动;GL 是最后一道
/// (远程桌面、精简虚拟机里常见 ANGLE/EGL)。
const BACKENDS: wgpu::Backends =
    wgpu::Backends::DX12.union(wgpu::Backends::VULKAN).union(wgpu::Backends::GL);

/// 适配器选择策略,全部可由环境变量覆盖(方便在别的机器上排查)。
struct AdapterPolicy {
    /// 允许 WARP / lavapipe 这类软件适配器(默认关:本项目的 CPU 光栅化更快)
    allow_software: bool,
    /// 优先独显(默认关:终端是常驻窗口,核显更省电,也是 wgpu 的 LowPower 语义)
    prefer_discrete: bool,
    /// 只保留名字里含该子串的适配器,或 `"0"`/`"1"` 这类序号
    only: Option<String>,
    /// 打印候选列表
    list: bool,
}

impl AdapterPolicy {
    /// `prefer_discrete` 来自配置(gpu_power),环境变量优先,便于临时排查。
    fn from_env(prefer_discrete: bool) -> Self {
        let flag = |k: &str| {
            std::env::var(k)
                .map(|v| !v.is_empty() && v != "0")
                .unwrap_or(false)
        };
        Self {
            allow_software: flag("VECTORTERM_GPU_ALLOW_SOFTWARE"),
            prefer_discrete: std::env::var("VECTORTERM_GPU_POWER")
                .map(|v| v.eq_ignore_ascii_case("high"))
                .unwrap_or(prefer_discrete),
            only: std::env::var("VECTORTERM_GPU_ADAPTER").ok().filter(|s| !s.is_empty()),
            list: flag("VECTORTERM_GPU_LIST"),
        }
    }
}

fn backend_rank(b: wgpu::Backend) -> u8 {
    match b {
        wgpu::Backend::Dx12 => 0,
        wgpu::Backend::Vulkan => 1,
        wgpu::Backend::Gl => 2,
        wgpu::Backend::Metal => 3,
        _ => 4,
    }
}

/// 同一后端内按设备类型排序。Cpu 排在最后(默认还会被直接过滤掉)。
fn type_rank(t: wgpu::DeviceType, prefer_discrete: bool) -> u8 {
    match t {
        wgpu::DeviceType::DiscreteGpu if prefer_discrete => 0,
        wgpu::DeviceType::IntegratedGpu if prefer_discrete => 1,
        wgpu::DeviceType::DiscreteGpu => 1,
        wgpu::DeviceType::IntegratedGpu => 0,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    }
}

/// 排序键:先比后端可信度,再比设备类型。同分时保留 wgpu 的枚举顺序。
fn rank_key(
    backend: wgpu::Backend,
    device_type: wgpu::DeviceType,
    prefer_discrete: bool,
) -> (u8, u8) {
    (backend_rank(backend), type_rank(device_type, prefer_discrete))
}

fn type_label(t: wgpu::DeviceType) -> &'static str {
    match t {
        wgpu::DeviceType::DiscreteGpu => "独显",
        wgpu::DeviceType::IntegratedGpu => "核显",
        wgpu::DeviceType::VirtualGpu => "虚拟",
        wgpu::DeviceType::Cpu => "软件",
        wgpu::DeviceType::Other => "其他",
    }
}

fn describe(adapter: &wgpu::Adapter) -> String {
    let i = adapter.get_info();
    format!("{} / {} / {:?}", i.name, type_label(i.device_type), i.backend)
}

fn describe_full(adapter: &wgpu::Adapter) -> String {
    let i = adapter.get_info();
    let mut s = describe(adapter);
    if !i.driver.is_empty() || !i.driver_info.is_empty() {
        s.push_str(&format!(" [{} {}]", i.driver, i.driver_info));
    }
    s
}

/// 枚举 + 过滤 + 排序。返回的每一项都能拿去 `request_device`。
/// 这里刻意不碰窗口(不传 surface):交换链在设备创建成功之后才建。
fn rank_adapters(
    instance: &wgpu::Instance,
    policy: &AdapterPolicy,
) -> Vec<(wgpu::Adapter, String)> {
    let all = instance.enumerate_adapters(BACKENDS);
    if policy.list {
        crate::wlog!("[VectorTerm] 候选图形适配器({} 个):", all.len());
        for (i, a) in all.iter().enumerate() {
            crate::wlog!("  [{i}] {}", describe_full(a));
        }
    }

    let mut out: Vec<(u8, u8, usize, wgpu::Adapter, String)> = Vec::new();
    for (i, adapter) in all.into_iter().enumerate() {
        let info = adapter.get_info();
        let label = describe(&adapter);

        // 软件适配器(WARP / lavapipe):默认排除,交给 CPU 光栅化
        if info.device_type == wgpu::DeviceType::Cpu && !policy.allow_software {
            continue;
        }
        // 用户点名:名字子串或枚举序号
        if let Some(want) = &policy.only {
            let hit = match want.parse::<usize>() {
                Ok(n) => n == i,
                Err(_) => info
                    .name
                    .to_ascii_lowercase()
                    .contains(&want.to_ascii_lowercase()),
            };
            if !hit {
                continue;
            }
        }

        let key = rank_key(info.backend, info.device_type, policy.prefer_discrete);
        out.push((key.0, key.1, i, adapter, label));
    }
    // stable sort:同分时保留 wgpu 的原始顺序
    out.sort_by_key(|(b, t, i, ..)| (*b, *t, *i));
    out.into_iter()
        .map(|(_, _, _, a, label)| (a, label))
        .collect()
}

impl GpuRenderer {
    /// `prefer_discrete`:双显卡机器上是否优先独显(配置里的 `gpu_power = "high"`)
    pub fn new(window: Arc<Window>, prefer_discrete: bool) -> anyhow::Result<Self> {
        Self::build(Some(window), 0, 0, prefer_discrete, false)
    }

    /// 逐像素透明版本:交换链挂到 DirectComposition 上,清屏 alpha 由
    /// [`GpuRenderer::set_background_alpha`] 控制。
    ///
    /// 失败时**不要**退回这条路的半成品:调用方应该拿错误去走 CPU 后端
    /// (那条路还能用层叠窗口的统一 alpha)。因为这里可能已经把窗口标成
    /// `WS_EX_NOREDIRECTIONBITMAP`,失败了要把窗口还原 —— 见 `Drop`。
    pub fn new_transparent(
        window: Arc<Window>,
        prefer_discrete: bool,
    ) -> anyhow::Result<Self> {
        Self::build(Some(window), 0, 0, prefer_discrete, true)
    }

    /// 不碰窗口地探一下"有没有可用的硬件适配器"。
    ///
    /// 窗口创建之前要用它决定 `WS_EX_NOREDIRECTIONBITMAP` —— 那个位一旦给了,
    /// DWM 就不再给窗口分重定向表面,**GDI(CPU 后端)的 BitBlt 会画不上去**,
    /// 表现是窗口里空空如也,只剩背后的模糊材质。所以只有确定要走 GPU 时才给。
    pub fn hardware_adapter_available(prefer_discrete: bool) -> bool {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: BACKENDS,
            ..Default::default()
        });
        let mut policy = AdapterPolicy::from_env(prefer_discrete);
        policy.list = false;
        !rank_adapters(&instance, &policy).is_empty()
    }

    /// 无窗口构造:测试用,渲染到离屏纹理再读回
    #[cfg(test)]
    pub fn new_offscreen(width: u32, height: u32) -> anyhow::Result<Self> {
        Self::build(None, width, height, false, false)
    }

    fn build(
        window: Option<Arc<Window>>,
        off_w: u32,
        off_h: u32,
        prefer_discrete: bool,
        transparent: bool,
    ) -> anyhow::Result<Self> {
        // 透明路线先把窗口改成"无重定向位图",交换链才有地方展示。
        // 只在真的要逐像素 alpha 时才做 —— 普通窗口加了它会让 softbuffer
        // 之类的 GDI 路径画不出东西。
        #[cfg(windows)]
        let dcomp = match (&window, transparent) {
            (Some(w), true) => Some(DcompTarget::attach(w)?),
            _ => None,
        };

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: BACKENDS,
            ..Default::default()
        });

        // 逐个尝试排好序的适配器:某个驱动拒绝创建设备时还有下一个,
        // 而不是直接掉到 CPU 软渲染。
        //
        // 注意顺序:先 request_device、成功之后才给窗口建交换链。
        // 反过来做的话,GPU 用不了的机器上 HWND 会先被 DXGI 挂上交换链,
        // 再交给 softbuffer 时行为不可控 —— 宁可不碰窗口。
        let policy = AdapterPolicy::from_env(prefer_discrete);
        let candidates = rank_adapters(&instance, &policy);
        if candidates.is_empty() {
            return Err(anyhow!(
                "枚举不到可用的图形适配器(后端={:?});设 VECTORTERM_GPU_LIST=1 可打印完整列表",
                BACKENDS
            ));
        }

        let mut errors: Vec<String> = Vec::new();
        let mut picked: Option<(
            wgpu::Adapter,
            wgpu::Device,
            wgpu::Queue,
            Option<wgpu::Surface<'static>>,
        )> = None;
        for (adapter, label) in candidates {
            let device = pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("VectorTerm"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            ));
            let (device, queue) = match device {
                Ok(v) => v,
                Err(e) => {
                    errors.push(format!("{label}: {e}"));
                    continue;
                }
            };
            // 设备拿到了,这时候才碰窗口
            let surface = match &window {
                Some(w) => {
                    // 有 DComp 就把内容挂到 visual 上(合成交换链,能给逐像素 alpha);
                    // 没有就走原来的 hwnd 交换链。
                    let created = {
                        #[cfg(windows)]
                        {
                            match &dcomp {
                                Some(d) => unsafe {
                                    instance.create_surface_unsafe(
                                        wgpu::SurfaceTargetUnsafe::CompositionVisual(d.visual_ptr()),
                                    )
                                },
                                None => instance.create_surface(w.clone()),
                            }
                        }
                        #[cfg(not(windows))]
                        {
                            instance.create_surface(w.clone())
                        }
                    };
                    match created {
                        Ok(s) => Some(s),
                        Err(e) => {
                            errors.push(format!("{label}: 创建交换链失败 {e}"));
                            continue;
                        }
                    }
                }
                None => None,
            };
            if let Some(s) = &surface {
                if s.get_capabilities(&adapter).formats.is_empty() {
                    errors.push(format!("{label}: 不支持该窗口的表面格式"));
                    continue;
                }
            }
            picked = Some((adapter, device, queue, surface));
            break;
        }
        let Some((adapter, device, queue, surface)) = picked else {
            return Err(anyhow!(
                "所有适配器都没能创建出设备:{}",
                errors.join("; ")
            ));
        };
        let adapter_name = describe(&adapter);

        let (w, h) = match &window {
            Some(win) => {
                let size = win.inner_size();
                (size.width.max(1), size.height.max(1))
            }
            None => (off_w.max(1), off_h.max(1)),
        };
        let caps = surface.as_ref().map(|s| s.get_capabilities(&adapter));
        // 用非 sRGB 格式:我们的颜色本来就是 sRGB 分量,走线性只会整体发灰
        let format = match &caps {
            Some(c) => c
                .formats
                .iter()
                .find(|f| !f.is_srgb())
                .copied()
                .unwrap_or(c.formats[0]),
            None => wgpu::TextureFormat::Rgba8Unorm,
        };
        let present_mode = match std::env::var("VECTORTERM_PRESENT_MODE").as_deref() {
            Ok("immediate") => wgpu::PresentMode::Immediate,
            Ok("mailbox") => wgpu::PresentMode::Mailbox,
            Ok("novsync") => wgpu::PresentMode::AutoNoVsync,
            _ => wgpu::PresentMode::AutoVsync,
        };
        // 后端不支持就给回 Fifo:否则 configure 会 panic,把窗口直接打死。
        // AutoVsync / AutoNoVsync 是 wgpu 的「自动挑一个」,本身不在可用列表里。
        let present_mode = match &caps {
            Some(c) if !matches!(present_mode, wgpu::PresentMode::AutoVsync | wgpu::PresentMode::AutoNoVsync)
                && !c.present_modes.contains(&present_mode) =>
            {
                crate::wlog!("[VectorTerm] 不支持 {present_mode:?},退回 AutoVsync");
                wgpu::PresentMode::AutoVsync
            }
            _ => present_mode,
        };
        let frame_latency = std::env::var("VECTORTERM_FRAME_LATENCY")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(1);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w,
            height: h,
            present_mode,
            alpha_mode: pick_alpha_mode(caps.as_ref(), transparent),
            view_formats: vec![],
            // 拖动缩放时每跨一格都要重建交换链。ResizeBuffers 会等上一帧
            // 真正显示出去,队列越深等得越久。
            desired_maximum_frame_latency: frame_latency,
        };
        if let Some(s) = &surface {
            s.configure(&device, &config);
            if let Some(c) = &caps {
                crate::wlog!(
                    "[VectorTerm] 呈现模式 {:?} 帧延迟 {} 可用模式 {:?}",
                    config.present_mode, config.desired_maximum_frame_latency, c.present_modes
                );
            }
        }

        // ---- 图集纹理 ----
        let atlas_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_W,
                height: ATLAS_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas_tex.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let trail_globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trail globals"),
            size: std::mem::size_of::<TrailGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let quad_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // 片元也要读:背景类填充的 `fs_main_bg` 从 globals 取 bg_alpha
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let quad_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad"),
            layout: &quad_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let trail_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trail"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let trail_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trail"),
            layout: &trail_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: trail_globals.as_entire_binding(),
            }],
        });

        // ---- 管线 ----
        let quad_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quads"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let trail_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("trail"),
            source: wgpu::ShaderSource::Wgsl(TRAIL_SHADER.into()),
        });
        let quad_pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quads"),
            bind_group_layouts: &[&quad_layout],
            push_constant_ranges: &[],
        });
        let trail_pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trail"),
            bind_group_layouts: &[&trail_layout],
            push_constant_ranges: &[],
        });
        // 全链走预乘 alpha:不透明窗口下合成结果与直通混合逐像素等价,
        // 透明窗口下才能得到正确的边缘。片元输出的也是预乘颜色。
        let blend = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        });
        // 背景类填充的 alpha 混合:只让颜色落到目标上,alpha 保持清屏给的值。
        // 否则每叠一层背景(壁纸、标签栏底色)窗口就少透一点,几层之后
        // `window_background_opacity` 已经不再是"整窗不透明度"了。
        let blend_bg = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quads"),
            layout: Some(&quad_pipe_layout),
            vertex: wgpu::VertexState {
                module: &quad_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: QUAD_ATTRS,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &quad_module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        // 与上一份唯一不同的是片元入口和 alpha 混合,见 `fs_main_bg`
        let pipeline_bg = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quads-bg"),
            layout: Some(&quad_pipe_layout),
            vertex: wgpu::VertexState {
                module: &quad_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: QUAD_ATTRS,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &quad_module,
                entry_point: Some("fs_main_bg"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: blend_bg,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let trail_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trail"),
            layout: Some(&trail_pipe_layout),
            vertex: wgpu::VertexState {
                module: &trail_module,
                entry_point: Some("vs_trail"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &trail_module,
                entry_point: Some("fs_trail"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let instance_cap = 8192;
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (instance_cap * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut atlas = Atlas::new();
        // 左上角留一个不透明的白点给实心矩形用
        let (wx, wy) = atlas.alloc(2, 2).context("图集:预留白点")?;
        atlas.put(wx, wy, 2, 2, &[255u8; 4]);
        let white_uv = [(wx as f32 + 0.5) / ATLAS_W as f32, (wy as f32 + 0.5) / ATLAS_H as f32];

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &atlas_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_W),
                rows_per_image: Some(ATLAS_H),
            },
            wgpu::Extent3d {
                width: ATLAS_W,
                height: ATLAS_H,
                depth_or_array_layers: 1,
            },
        );
        atlas.dirty = None;

        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            pipeline_bg,
            trail_pipeline,
            quad_bind,
            trail_bind,
            globals,
            trail_globals,
            instances,
            instance_cap,
            atlas,
            atlas_tex,
            white_uv,
            slots: HashMap::new(),
            image_tex: HashMap::new(),
            image_bind_bytes: 0,
            frame_seq: 0,
            quad_layout,
            sampler,
            adapter_name,
            last_target: None,
            #[cfg(windows)]
            dcomp,
            bg_alpha: 1.0,
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// 背景不透明度(逐像素 alpha 的清屏值)。1.0 = 不透明。
    ///
    /// 只作用在"背景类"填充上(清屏色、壁纸/渐变、单元格底色、标签栏底色),
    /// 而且**整层合成完只乘一次** —— 叠几层背景都恰好是这个值,不是
    /// 1-(1-a)^n。字形、光标、图片这些前景实例不参与,文字永远实心 ——
    /// 这正是层叠窗口统一 alpha 做不到的。
    pub fn set_background_alpha(&mut self, alpha: f32) {
        self.bg_alpha = alpha.clamp(0.0, 1.0);
    }

    /// 当前是否真的走合成交换链(逐像素 alpha 可用)。
    pub fn is_transparent_surface(&self) -> bool {
        #[cfg(windows)]
        {
            self.dcomp.is_some()
                && self.config.alpha_mode == wgpu::CompositeAlphaMode::PreMultiplied
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        if (w, h) == (self.config.width, self.config.height) {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        if let Some(s) = &self.surface {
            s.configure(&self.device, &self.config);
        }
    }

    /// 等待已提交的 GPU 工作全部完成。仅供基准测试:把异步的 submit 变成
    /// 可测量的端到端帧时间(离屏模式没有 present / vsync)。
    #[cfg(test)]
    pub fn sync(&self) {
        let _ = self.device.poll(wgpu::Maintain::Wait);
    }

    /// 读回离屏帧(RGBA8)。仅供测试:验证 GPU 与 CPU 两条路径画得一样。
    #[cfg(test)]
    pub fn read_pixels(&self) -> Vec<u8> {
        let tex = self.last_target.as_ref().expect("离屏帧未生成");
        let (w, h) = (self.config.width, self.config.height);
        let unpadded = w * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded.div_ceil(align) * align;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded as u64) * (h as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::Maintain::Wait);
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((unpadded * h) as usize);
        for y in 0..h {
            let s = (y * padded) as usize;
            out.extend_from_slice(&data[s..s + unpadded as usize]);
        }
        drop(data);
        buffer.unmap();
        out
    }

    /// 把用到的字形塞进图集,并上传脏区域
    fn sync_atlas(&mut self, renderer: &Renderer, frame: &Frame) {
        let mut added = false;
        for item in &frame.items {
            if let Item::Glyph { id, .. } = item {
                if self.slots.contains_key(id) {
                    continue;
                }
                let Some((data, w, h)) = renderer.glyph_bitmap(*id) else {
                    self.slots.insert(*id, None);
                    continue;
                };
                let slot = self.atlas.alloc(w, h).map(|(x, y)| {
                    self.atlas.put(x, y, w, h, data);
                    Slot { x, y }
                });
                self.slots.insert(*id, slot);
                added = true;
            }
        }
        if !added {
            return;
        }
        if let Some((x0, y0, x1, y1)) = self.atlas.dirty.take() {
            let (w, h) = (x1 - x0, y1 - y0);
            let offset = (y0 * ATLAS_W + x0) as usize;
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: x0, y: y0, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &self.atlas.data[offset..],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ATLAS_W),
                    rows_per_image: Some(ATLAS_H),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    fn quad(
        &mut self,
        b: &mut Batches,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        shear: f32,
        radius: f32,
        bg: bool,
    ) {
        b.push(
            BindRef::Atlas,
            bg,
            Instance {
                rect: [x, y, w, h],
                uv: [
                    self.white_uv[0],
                    self.white_uv[1],
                    self.white_uv[0],
                    self.white_uv[1],
                ],
                color,
                params: [shear, 0.0],
                extra: [radius, 0.0],
            },
        );
    }

    /// 确保某张图有 GPU 纹理(按内容哈希缓存),失败返回 false。
    /// 把背景层翻译成实例。填充几何与 CPU 端共用 `background` 模块,
    /// 但 GPU 只能画"可见的那一块",所以还要反算 uv ——
    /// 否则 cover 的超出部分会被压进可见区,变成拉伸。
    fn push_background(&mut self, b: &mut Batches, frame: &Frame) {
        if frame.bg.is_empty() || frame.width == 0 || frame.height == 0 {
            return;
        }
        let (fw, fh) = (frame.width as f32, frame.height as f32);
        for layer in &frame.bg {
            let img = &*layer.image;
            let (iw, ih) = (img.width as f32, img.height as f32);
            let key = bg_key(img);
            if !self.ensure_bg_texture(key, img) {
                continue;
            }
            let mut quads: Vec<([f32; 4], [f32; 4])> = Vec::new();
            if layer.fill == ImageFill::Tile {
                let origins = crate::background::tile_origins(iw, ih, fw, fh, 512);
                if origins.is_empty() {
                    // 块数爆炸时退化成 cover,和 CPU 端一致
                    let r = crate::background::dst_rect(ImageFill::Cover, iw, ih, fw, fh);
                    quads.extend(crate::background::clip_uv(r, fw, fh));
                } else {
                    for o in origins {
                        if let Some(q) =
                            crate::background::clip_uv([o[0], o[1], iw, ih], fw, fh)
                        {
                            quads.push(q);
                        }
                    }
                }
            } else {
                let r = crate::background::dst_rect(layer.fill, iw, ih, fw, fh);
                quads.extend(crate::background::clip_uv(r, fw, fh));
            }
            for (rect, uv) in quads {
                b.push(
                    BindRef::Image(key),
                    // 背景层天然属于"背景类":与清屏色一起合成后整体透出去
                    true,
                    Instance {
                        rect,
                        uv,
                        color: [1.0, 1.0, 1.0, layer.alpha],
                        params: [0.0, 0.0],
                        // 负半径 = 这批实例绑的是整张图,片元走图片分支
                        extra: [-1.0, 0.0],
                    },
                );
            }
        }
    }

    /// 背景纹理:key 命中就复用,否则上传一张。
    fn ensure_bg_texture(&mut self, key: [u8; 32], img: &crate::images::DecodedImage) -> bool {
        if let Some(e) = self.image_tex.get_mut(&key) {
            e.last_used = self.frame_seq;
            return true;
        }
        let max = self.device.limits().max_texture_dimension_2d.max(1);
        let img = img.fit_within(max);
        if img.width == 0 || img.height == 0 {
            return false;
        }
        self.upload_texture(key, &img);
        true
    }

    fn ensure_image(&mut self, renderer: &Renderer, key: &[u8; 32]) -> bool {
        let seq = self.frame_seq;
        if let Some(e) = self.image_tex.get_mut(key) {
            e.last_used = seq;
            return true;
        }
        let Some(img) = renderer.images.cached(key) else {
            return false;
        };
        // 超过纹理尺寸上限时在 CPU 侧缩一次,总比整张画不出来好
        let max = self.device.limits().max_texture_dimension_2d.max(1);
        let img = img.fit_within(max);
        if img.width == 0 || img.height == 0 {
            return false;
        }
        self.upload_texture(*key, &img);
        true
    }

    /// 上传一张位图并建好 bind group,按 `key` 记住。
    /// 终端图片与背景层共用这条路径,着色器里都走"整张图"分支。
    fn upload_texture(&mut self, key: [u8; 32], img: &crate::images::DecodedImage) {
        let size = wgpu::Extent3d {
            width: img.width,
            height: img.height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // 非 sRGB:着色器写出的值原样落到目标上,和矩形/字形同一条约定。
            // 用 Rgba8UnormSrgb 会被硬件转成线性、出现整体偏暗。
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(img.width * 4),
                rows_per_image: Some(img.height),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // 与字形批次同一个布局,只是把图集换成整张图 —— 片元着色器据此走图片分支
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image"),
            layout: &self.quad_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.image_bind_bytes += img.byte_len();
        let last_used = self.frame_seq;
        self.image_tex.insert(
            key,
            ImageTex {
                bind,
                bytes: img.byte_len(),
                last_used,
            },
        );
    }

    /// 回收长时间没再出现的图片纹理(滚出回看区后就不必占显存了)
    fn gc_images(&mut self) {
        let seq = self.frame_seq;
        self.image_tex
            .retain(|_, e| seq.saturating_sub(e.last_used) < 120);
        self.image_bind_bytes = self.image_tex.values().map(|e| e.bytes).sum();
        const BUDGET: usize = 256 * 1024 * 1024;
        if self.image_bind_bytes <= BUDGET {
            return;
        }
        let mut keys: Vec<([u8; 32], u64)> =
            self.image_tex.iter().map(|(k, e)| (*k, e.last_used)).collect();
        keys.sort_by_key(|(_, t)| *t);
        for (k, _) in keys {
            if self.image_bind_bytes <= BUDGET / 2 {
                break;
            }
            if let Some(e) = self.image_tex.remove(&k) {
                self.image_bind_bytes = self.image_bind_bytes.saturating_sub(e.bytes);
            }
        }
    }

    pub fn draw(&mut self, frame: &Frame, renderer: &Renderer) -> anyhow::Result<()> {
        self.sync_atlas(renderer, frame);
        self.frame_seq += 1;

        let mut b = Batches::new();
        // 背景层最先入队:它铺满整窗,后面所有内容都叠在它上面。
        // 放在这里(而不是 items 里)是为了让它不受内容区 scissor 裁剪。
        self.push_background(&mut b, frame);
        // 背景层实例到此为止。绘制阶段要用它把"背景层"和"内容"分开:
        // 背景层必须铺满整窗(和 CPU 端一样暂时放开 clip),否则标签栏下那条
        // 内边距只剩下清屏色,会变成一条比内容更透的横带。
        let bg_end = b.insts.len() as u32;
        let mut trail_item: Option<&Item> = None;
        // 尾迹在实例流里的位置:GPU 要在这里切一次管线
        let mut trail_at: u32 = 0;
        // 标签栏叠加层的起始实例:它不受内容区 scissor 裁剪
        let mut overlay_at: u32 = 0;
        let mut has_overlay = false;
        // 横向裁剪带的起点(侧边栏滑动用),同样要在实例流里断一刀
        let mut clip_at: u32 = 0;
        let mut clip_range: Option<(f32, f32)> = None;
        for (i, item) in frame.items.iter().enumerate() {
            if Some(i) == frame.overlay_at {
                overlay_at = b.insts.len() as u32;
                has_overlay = true;
                b.cut();
            }
            if let Some(c) = frame.clip_x {
                if i == c.from {
                    clip_at = b.insts.len() as u32;
                    clip_range = Some((c.x0, c.x1));
                    b.cut();
                }
            }
            match item {
                Item::Rect {
                    x,
                    y,
                    w,
                    h,
                    color,
                    alpha,
                } => {
                    let c = [
                        color.r as f32 / 255.0,
                        color.g as f32 / 255.0,
                        color.b as f32 / 255.0,
                        *alpha,
                    ];
                    // 这一段是背景类还是内容类,由渲染层在 `Frame::bg_items` 里标好
                    let bg = frame.bg_items.binary_search(&i).is_ok();
                    self.quad(&mut b, *x, *y, *w, *h, c, 0.0, 0.0, bg);
                }
                Item::RoundRect {
                    x,
                    y,
                    w,
                    h,
                    radius,
                    color,
                    alpha,
                } => {
                    let c = [
                        color.r as f32 / 255.0,
                        color.g as f32 / 255.0,
                        color.b as f32 / 255.0,
                        *alpha,
                    ];
                    let bg = frame.bg_items.binary_search(&i).is_ok();
                    self.quad(&mut b, *x, *y, *w, *h, c, 0.0, *radius, bg);
                }
                Item::Glyph {
                    id,
                    src,
                    dst,
                    color,
                    italic,
                    ..
                } => {
                    let Some(Some(slot)) = self.slots.get(id) else {
                        continue;
                    };
                    let (sw, sh, sx, sy) = (src[2], src[3], src[0], src[1]);
                    // 半像素内缩:1:1 时正好落在源像素中心,不会糊
                    let u0 = (slot.x as f32 + sx + 0.5) / ATLAS_W as f32;
                    let v0 = (slot.y as f32 + sy + 0.5) / ATLAS_H as f32;
                    let u1 = (slot.x as f32 + sx + sw - 0.5) / ATLAS_W as f32;
                    let v1 = (slot.y as f32 + sy + sh - 0.5) / ATLAS_H as f32;
                    let shear = if *italic { dst[3] * 0.22 } else { 0.0 };
                    b.push(
                        BindRef::Atlas,
                        false,
                        Instance {
                            rect: [dst[0], dst[1], dst[2], dst[3]],
                            uv: [u0, v0, u1, v1],
                            color: [
                                color.r as f32 / 255.0,
                                color.g as f32 / 255.0,
                                color.b as f32 / 255.0,
                                1.0,
                            ],
                            params: [shear, 0.0],
                            extra: [0.0, 0.0],
                        },
                    );
                }
                Item::Image { key, dst } => {
                    if !self.ensure_image(renderer, key) {
                        continue;
                    }
                    // 整张图一个四边形:uv 直接铺满 0..1,与 CPU 的双线性缩放一致
                    b.push(
                        BindRef::Image(*key),
                        false,
                        Instance {
                            rect: [dst[0], dst[1], dst[2], dst[3]],
                            uv: [0.0, 0.0, 1.0, 1.0],
                            color: [1.0, 1.0, 1.0, 1.0],
                            params: [0.0, 0.0],
                            // 负半径 = 这批实例绑的是整张图,片元着色器据此返回 RGBA
                            extra: [-1.0, 0.0],
                        },
                    );
                }
                Item::Trail { .. } => {
                    trail_at = b.insts.len() as u32;
                    trail_item = Some(item);
                    b.cut();
                }
            }
        }
        let total = b.insts.len() as u32;
        if frame.overlay_at == Some(frame.items.len()) {
            overlay_at = total;
            has_overlay = true;
        }

        // 扩实例缓冲
        if total as usize > self.instance_cap {
            self.instance_cap = (total as usize).next_power_of_two();
            self.instances = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instances"),
                size: (self.instance_cap * std::mem::size_of::<Instance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if total > 0 {
            self.queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(&b.insts));
        }
        self.queue.write_buffer(
            &self.globals,
            0,
            bytemuck::bytes_of(&Globals {
                size: [
                    self.config.width as f32,
                    self.config.height as f32,
                ],
                bg_alpha: self.bg_alpha,
                _pad: 0.0,
            }),
        );
        if let Some(Item::Trail {
            corners,
            color,
            alpha,
            mask,
        }) = trail_item
        {
            self.queue.write_buffer(
                &self.trail_globals,
                0,
                bytemuck::bytes_of(&TrailGlobals {
                    size: [
                        self.config.width as f32,
                        self.config.height as f32,
                    ],
                    _pad: [0.0, 0.0],
                    corners: [
                        [corners[0].x, corners[0].y, 0.0, 0.0],
                        [corners[1].x, corners[1].y, 0.0, 0.0],
                        [corners[2].x, corners[2].y, 0.0, 0.0],
                        [corners[3].x, corners[3].y, 0.0, 0.0],
                    ],
                    mask: *mask,
                    color: [
                        color.r as f32 / 255.0,
                        color.g as f32 / 255.0,
                        color.b as f32 / 255.0,
                        *alpha,
                    ],
                }),
            );
        }

        // 目标纹理:窗口走交换链,测试走离屏
        let (target_tex, frame_tex) = match &self.surface {
            Some(s) => match s.get_current_texture() {
                Ok(t) => {
                    let view_holder = t.texture.clone();
                    (view_holder, Some(t))
                }
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    s.configure(&self.device, &self.config);
                    return Ok(());
                }
                Err(e) => return Err(anyhow!("获取交换链纹理失败:{e}")),
            },
            None => {
                // 离屏模式复用上一帧的纹理:否则每帧 create_texture 的成本会
                // 混进基准,而真实窗口路径用的是交换链纹理(不会重建)。
                let reuse = self.last_target.as_ref().filter(|t| {
                    t.size().width == self.config.width && t.size().height == self.config.height
                });
                let offscreen = match reuse {
                    Some(t) => t.clone(),
                    None => self.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("offscreen"),
                        size: wgpu::Extent3d {
                            width: self.config.width,
                            height: self.config.height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: self.config.format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    }),
                };
                (offscreen, None)
            }
        };
        if frame_tex.is_none() {
            self.last_target = Some(target_tex.clone());
        }
        let view = target_tex.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let c = frame.clear;
            // 逐像素透明时清屏要写预乘色:颜色乘 alpha,alpha 单独给。
            // 不透明窗口下 bg_alpha 恒为 1,乘了也不变。
            let a = self.bg_alpha.clamp(0.0, 1.0) as f64;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("VectorTerm"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: c.r as f64 / 255.0 * a,
                            g: c.g as f64 / 255.0 * a,
                            b: c.b as f64 / 255.0 * a,
                            a,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // 亚行滚动时上移的内容不能渗进上边距;标签栏在下方时,内容到它上沿为止
            let top = frame.clip_top.max(0.0).round() as u32;
            let top = top.min(self.config.height.saturating_sub(1));
            let bottom = frame
                .clip_bottom
                .max(0.0)
                .round()
                .min(self.config.height as f32) as u32;
            let content_h = bottom.saturating_sub(top).max(1);
            let content_h = content_h.min(self.config.height - top);
            pass.set_scissor_rect(0, top, self.config.width, content_h);

            // 分段绘制:每段一个 bind group(图集或某张图),尾迹与标签栏在段边界上
            let mut trail_drawn = false;
            let mut full_scissor = false;
            let mut h_clipped = false;
            for seg in &b.segs {
                if !trail_drawn && trail_item.is_some() && seg.start >= trail_at {
                    pass.set_pipeline(&self.trail_pipeline);
                    pass.set_bind_group(0, &self.trail_bind, &[]);
                    pass.draw(0..6, 0..1);
                    trail_drawn = true;
                }
                // 背景层与标签栏叠加层铺满整窗,不受内容区裁剪:
                // 背景层被裁掉的话,标签栏下面那条内边距就没有壁纸/渐变,
                // 只剩一层清屏色,看起来是"半截毛玻璃"(比内容更透的一条横带)。
                let want_full = seg.start < bg_end || (has_overlay && seg.start >= overlay_at);
                if want_full && !full_scissor {
                    pass.set_scissor_rect(0, 0, self.config.width, self.config.height);
                    full_scissor = true;
                    h_clipped = false;
                } else if !want_full && full_scissor {
                    // 背景层画完,回到内容区裁剪
                    pass.set_scissor_rect(0, top, self.config.width, content_h);
                    full_scissor = false;
                }
                // 横向裁剪带(侧边栏滑入/滑出):把 scissor 收窄到它自己的区域。
                // 不管的话滑动中的面板会糊到右邻窗格的终端上。
                if let Some((cx0, cx1)) = clip_range {
                    if !h_clipped && !full_scissor && seg.start >= clip_at {
                        let x0 = cx0.max(0.0).min(self.config.width as f32) as u32;
                        let x1 = cx1.max(0.0).min(self.config.width as f32) as u32;
                        pass.set_scissor_rect(x0, top, x1.saturating_sub(x0).max(1), content_h);
                        h_clipped = true;
                    }
                }
                let bind = match seg.bind {
                    BindRef::Atlas => &self.quad_bind,
                    BindRef::Image(k) => match self.image_tex.get(&k) {
                        Some(t) => &t.bind,
                        None => continue,
                    },
                };
                // 背景类填充走"不写 alpha"的那条管线:整窗透明度只由清屏决定
                let pipe = if seg.bg {
                    &self.pipeline_bg
                } else {
                    &self.pipeline
                };
                pass.set_pipeline(pipe);
                pass.set_bind_group(0, bind, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                pass.draw(0..6, seg.start..seg.end);
            }
            // 尾迹在最后一个实例之后(没有后续段时)
            if !trail_drawn && trail_item.is_some() {
                pass.set_pipeline(&self.trail_pipeline);
                pass.set_bind_group(0, &self.trail_bind, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        if let Some(t) = frame_tex {
            t.present();
        }
        // 合成交换链:Present 只更新内容,DComp 收到 Commit 才会把它交给 DWM
        #[cfg(windows)]
        if let Some(d) = &self.dcomp {
            d.commit();
        }
        self.gc_images();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::render;
    use wezterm_term::TerminalSize;

    /// 把 GPU 离屏帧导出为 BMP,供目测核对
    /// `cargo test dump_gpu_frame -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_gpu_frame() {
        let cfg = AppConfig::default();
        let (cols, rows) = (64usize, 6usize);
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, &cfg, Box::new(std::io::sink()));
        let seg = |bg: (u8, u8, u8), fg: (u8, u8, u8), text: &str, next: (u8, u8, u8)| {
            format!(
                "\x1b[48;2;{};{};{}m\x1b[38;2;{};{};{}m {}\x1b[48;2;{};{};{}m\x1b[38;2;{};{};{}m{}\x1b[0m",
                bg.0, bg.1, bg.2, fg.0, fg.1, fg.2, text,
                next.0, next.1, next.2, bg.0, bg.1, bg.2, '\u{e0b0}'
            )
        };
        let a = (137, 180, 250);
        let b = (245, 189, 230);
        let c = (166, 227, 161);
        let bg0 = (30, 30, 46);
        let mut s = String::new();
        s.push_str(&seg(a, bg0, " \u{f015} ~/dev/vectorterm", b));
        s.push_str(&seg(b, bg0, " \u{e725} main", c));
        s.push_str(&seg(c, bg0, " \u{e718} v20.11", a));
        s.push_str("\x1b[48;2;30;30;46m\x1b[38;2;137;180;250m\u{e0b0}\x1b[0m\r\n");
        s.push_str("GPU 渲染:中文 English \u{2500}\u{2502}\u{250c}\u{2510} \u{f00c}\u{f017}\r\n");
        t.advance_bytes(s.as_bytes());

        let (snap, cursor, palette) = render::snapshot(&t, cols, 0.0);
        let mut r = render::Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = (
            (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32,
            (rows as f64 * r.cell_h + 2.0 * r.pad_y) as u32,
        );
        let mut frame = Frame::default();
        r.build_frame(&mut frame, w, h, &snap, &cursor, &palette, None, palette.cursor_bg, true);

        let mut gpu = GpuRenderer::new_offscreen(w, h).expect("离屏 GPU");
        gpu.draw(&frame, &r).expect("GPU 绘制");
        let px = gpu.read_pixels();
        // 写 24 位 BMP
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
                let i = ((y * w + x) * 4) as usize;
                out.push(px[i + 2]);
                out.push(px[i + 1]);
                out.push(px[i]);
            }
            for _ in w * 3..row_bytes {
                out.push(0);
            }
        }
        std::fs::write("target/vectorterm-gpu.bmp", &out).unwrap();
        crate::wlog!("frame -> target/vectorterm-gpu.bmp ({w}x{h})  适配器:{}", gpu.adapter_name());
    }

    /// 图片:GPU 走"换 bind group 的实例段",CPU 走双线性缩放,两者观感要一致
    #[test]
    fn gpu_draws_images_like_cpu() {
        use base64::Engine as _;
        let cfg = AppConfig::default();
        let (cols, rows) = (28usize, 8usize);
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, &cfg, Box::new(std::io::sink()));

        // 纯色图:双线性/线性过滤的取样差异最小,便于逐像素比较
        let (iw, ih) = (60u32, 40u32);
        let mut raw = Vec::with_capacity((iw * ih * 4) as usize);
        for _ in 0..iw * ih {
            raw.extend_from_slice(&[80, 200, 160, 255]);
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        let seq = format!("\x1b_Gf=32,s={iw},v={ih},a=T,i=3;{b64}\x1b\\");
        t.advance_bytes(seq.as_bytes());
        t.advance_bytes(b"\r\ntext below image");

        let (snap, cursor, palette) = render::snapshot(&t, cols, 0.0);
        let mut r = render::Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = (
            (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32,
            (rows as f64 * r.cell_h + 2.0 * r.pad_y) as u32,
        );
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
        assert!(
            frame
                .items
                .iter()
                .any(|i| matches!(i, Item::Image { .. })),
            "帧里应该有图片四边形"
        );

        let mut cpu = vec![0u32; (w * h) as usize];
        r.paint_cpu(&mut cpu, w, h, &frame);

        let mut gpu = match GpuRenderer::new_offscreen(w, h) {
            Ok(g) => g,
            Err(e) => {
                crate::wlog!("跳过 GPU 图片对比:没有可用适配器({e:#})");
                return;
            }
        };
        gpu.draw(&frame, &r).expect("GPU 绘制");
        let px = gpu.read_pixels();

        let mut sum = 0f64;
        let mut bad = 0usize;
        for i in 0..(w * h) as usize {
            let c = cpu[i];
            let (r0, g0, b0) = ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
            let (r1, g1, b1) = (px[i * 4] as u32, px[i * 4 + 1] as u32, px[i * 4 + 2] as u32);
            let d = (r0 as i32 - r1 as i32).abs()
                + (g0 as i32 - g1 as i32).abs()
                + (b0 as i32 - b1 as i32).abs();
            sum += d as f64;
            if d > 96 {
                bad += 1;
            }
        }
        let total = (w * h) as f64;
        let mean = sum / total / 3.0;
        let bad_ratio = bad as f64 / total;
        crate::wlog!(
            "图片 GPU vs CPU:{}x{} 平均通道误差 {:.2},差异像素 {:.2}%",
            w,
            h,
            mean,
            bad_ratio * 100.0
        );
        assert!(mean < 3.0, "图片渲染差异过大(平均 {mean:.2}/255)");
        assert!(bad_ratio < 0.05, "差异像素占比 {:.1}% 过高", bad_ratio * 100.0);
    }

    /// 圆角矩形两条路径必须一致:GPU 在片元着色器里算圆角盒 SDF,
    /// CPU 在 `Canvas::fill_round_rect` 里算同一套公式,差值应只剩采样取整
    #[test]
    fn gpu_round_rect_matches_cpu() {
        let cfg = AppConfig::default();
        let r = render::Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = (80u32, 48u32);

        let mut frame = Frame::default();
        let bg = render::Rgb { r: 12, g: 12, b: 12 };
        frame.begin(w, h, 0.0, bg);
        // 实心圆角、半透明圆角、半径大于短边一半(应自动夹到半高)各一个
        frame.round_rect(4.0, 4.0, 30.0, 20.0, 6.0, render::Rgb { r: 240, g: 90, b: 60 }, 1.0);
        frame.round_rect(40.0, 6.0, 24.0, 16.0, 5.0, render::Rgb { r: 60, g: 160, b: 240 }, 0.5);
        frame.round_rect(8.0, 28.0, 20.0, 12.0, 40.0, render::Rgb { r: 200, g: 200, b: 60 }, 1.0);

        let mut cpu = vec![0u32; (w * h) as usize];
        r.paint_cpu(&mut cpu, w, h, &frame);

        let mut gpu = match GpuRenderer::new_offscreen(w, h) {
            Ok(g) => g,
            Err(e) => {
                crate::wlog!("跳过 GPU 圆角对比:没有可用适配器({e:#})");
                return;
            }
        };
        gpu.draw(&frame, &r).expect("GPU 绘制");
        let px = gpu.read_pixels();

        let mut bad = 0usize;
        let mut sum = 0f64;
        for i in 0..(w * h) as usize {
            let c = cpu[i];
            let (r0, g0, b0) = ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
            let (r1, g1, b1) = (px[i * 4] as u32, px[i * 4 + 1] as u32, px[i * 4 + 2] as u32);
            let d = (r0 as i32 - r1 as i32).abs()
                + (g0 as i32 - g1 as i32).abs()
                + (b0 as i32 - b1 as i32).abs();
            sum += d as f64;
            if d > 96 {
                bad += 1;
            }
        }
        let total = (w * h) as f64;
        let mean = sum / total / 3.0;
        let bad_ratio = bad as f64 / total;
        crate::wlog!(
            "圆角 GPU vs CPU:{w}x{h} 平均通道误差 {mean:.2},差异像素占比 {:.2}%",
            bad_ratio * 100.0
        );
        assert!(mean < 3.0, "圆角两条路径差异过大(平均 {mean:.2}/255)");
        assert!(bad_ratio < 0.03, "差异像素占比 {:.1}% 过高", bad_ratio * 100.0);
    }

    /// 背景类填充只让清屏写一次 alpha:`window_background_opacity = 0.5` 时,
    /// 不管背景叠了几层,透出去的都是 50%,而不是 1-(1-a)^n。
    ///
    /// 这正是"标签栏比内容区更实"那条横带的成因:清屏、壁纸、标签栏底色
    /// 各自乘过一次 opacity,三层下来 0.35 变成了 0.73。
    #[test]
    fn bg_opacity_lands_once() {
        let cfg = AppConfig::default();
        let r = render::Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = (32u32, 16u32);
        let tint = render::Rgb {
            r: 200,
            g: 100,
            b: 50,
        };

        let mut frame = Frame::default();
        frame.begin(w, h, 0.0, render::Rgb { r: 0, g: 0, b: 0 });
        // 左半:两层背景类填充叠在一起(相当于壁纸 + 标签栏底色)
        frame.rect_bg(0.0, 0.0, 16.0, 16.0, tint, 1.0);
        frame.rect_bg(0.0, 0.0, 16.0, 16.0, tint, 1.0);
        // 右半:内容类填充(不参与窗口不透明度)
        frame.rect(
            16.0,
            0.0,
            16.0,
            16.0,
            render::Rgb {
                r: 20,
                g: 220,
                b: 90,
            },
            1.0,
        );

        let mut gpu = match GpuRenderer::new_offscreen(w, h) {
            Ok(g) => g,
            Err(e) => {
                crate::wlog!("跳过背景透明度测试:没有可用适配器({e:#})");
                return;
            }
        };
        gpu.set_background_alpha(0.5);
        gpu.draw(&frame, &r).expect("GPU 绘制");
        let px = gpu.read_pixels();
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            (
                px[i] as i32,
                px[i + 1] as i32,
                px[i + 2] as i32,
                px[i + 3] as i32,
            )
        };
        let (bg_r, _, _, bg_a) = at(2, 8);
        assert!(
            (bg_a - 128).abs() <= 2,
            "背景叠两层后 alpha 仍应是 50%,实际 {bg_a}"
        );
        assert!(
            (bg_r - 100).abs() <= 3,
            "背景色只该被乘一次 opacity(200×0.5≈100),实际 {bg_r}"
        );
        let (_, _, _, fg_a) = at(30, 8);
        assert_eq!(fg_a, 255, "内容类填充必须保持不透明,否则文字会跟着变淡");
    }

    /// 显式设了底色的格子(SGR `48;2;…`)属于内容:玻璃窗口下也必须不透明。
    ///
    /// 把它当成"画布"处理的话,颜色会被乘上 `window_background_opacity` 再透出去,
    /// oh-my-posh 那种 `#8AADF4` 的提示符段落会变成半透明灰蓝 —— 而同一行里用
    /// 同色画出来的圆角字形仍然是亮的,一比就像"美化效果渲染坏了"。
    #[test]
    fn colored_cell_bg_stays_opaque() {
        let cfg = AppConfig::default();
        let (cols, rows) = (20usize, 3usize);
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, &cfg, Box::new(std::io::sink()));
        t.advance_bytes(
            b"\x1b[48;2;138;173;244m\x1b[38;2;73;77;100mDYBAG@HOST ~\x1b[0m\r\n\x1b[0mplain",
        );
        let (snap, cursor, palette) = render::snapshot(&t, cols, 0.0);
        let mut r = render::Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = (
            (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32,
            (rows as f64 * r.cell_h + 2.0 * r.pad_y) as u32,
        );
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

        let mut gpu = match GpuRenderer::new_offscreen(w, h) {
            Ok(g) => g,
            Err(e) => {
                crate::wlog!("跳过显式底色测试:没有可用适配器({e:#})");
                return;
            }
        };
        gpu.set_background_alpha(0.5);
        gpu.draw(&frame, &r).expect("GPU 绘制");
        let px = gpu.read_pixels();
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            (
                px[i] as i32,
                px[i + 1] as i32,
                px[i + 2] as i32,
                px[i + 3] as i32,
            )
        };

        // 第一行提示符段落:取格子顶部两像素,避开字形笔画
        let (x, y) = (
            r.pad_x.round() as u32 + 3,
            r.pad_y.round() as u32 + 1,
        );
        let (cr, cg, cb, ca) = at(x, y);
        assert_eq!(ca, 255, "显式底色必须不透明,否则提示符色块会发灰");
        assert!(
            (cr - 138).abs() <= 2 && (cg - 173).abs() <= 2 && (cb - 244).abs() <= 2,
            "显式底色不该被 opacity 衰减,实际 ({cr},{cg},{cb})"
        );

        // 第二行普通格子:仍是半透明的画布
        let y2 = r.pad_y.round() as u32 + r.cell_h.round() as u32 + 1;
        let (_, _, _, a2) = at(x, y2);
        assert!(
            (a2 - 128).abs() <= 2,
            "画布底色应当只有 50% 不透明,实际 {a2}"
        );
    }

    /// 横向裁剪两条后端都得认:CPU 夹 x 区间,GPU 收 scissor 宽度。
    ///
    /// 侧边栏滑入/滑出全靠它 —— 少一条的表现是"滑动中的面板糊到左邻窗格的
    /// 终端上",而且只在分屏时看得见,回归很容易漏。
    #[test]
    fn clip_x_holds_on_both_backends() {
        let cfg = AppConfig::default();
        let r = render::Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = (80u32, 24u32);
        let base = render::Rgb { r: 40, g: 40, b: 40 };
        let over = render::Rgb { r: 220, g: 60, b: 60 };
        let want_base = (base.r as u32) << 16 | (base.g as u32) << 8 | base.b as u32;
        let want_over = (over.r as u32) << 16 | (over.g as u32) << 8 | over.b as u32;

        let mut frame = Frame::default();
        frame.begin(w, h, 0.0, render::Rgb { r: 0, g: 0, b: 0 });
        // 先铺满:这条在裁剪开始之前,应当整条可见
        frame.rect(0.0, 0.0, w as f64, h as f64, base, 1.0);
        // 再开一条 [20, 60) 的横向裁剪带,然后画一条**越界**的宽矩形
        frame.clip_from_here(20.0, 60.0);
        frame.rect(-20.0, 0.0, 120.0, h as f64, over, 1.0);

        let mut cpu = vec![0u32; (w * h) as usize];
        r.paint_cpu(&mut cpu, w, h, &frame);
        let at = |buf: &[u32], x: u32| buf[(5 * w + x) as usize];
        for x in [0u32, 19] {
            assert_eq!(at(&cpu, x), want_base, "裁剪带左边 x={x} 不该被覆盖");
        }
        for x in [20u32, 40, 59] {
            assert_eq!(at(&cpu, x), want_over, "裁剪带里 x={x} 应该被覆盖");
        }
        for x in [60u32, 79] {
            assert_eq!(at(&cpu, x), want_base, "裁剪带右边 x={x} 不该被覆盖");
        }

        let mut gpu = match GpuRenderer::new_offscreen(w, h) {
            Ok(g) => g,
            Err(e) => {
                crate::wlog!("跳过 GPU 横向裁剪对比:没有可用适配器({e:#})");
                return;
            }
        };
        gpu.draw(&frame, &r).expect("GPU 绘制");
        let px = gpu.read_pixels();
        for x in [0u32, 19, 60, 79] {
            let i = ((5 * w + x) * 4) as usize;
            assert!(
                px[i] as i32 - 40 < 8 && px[i + 1] as i32 - 40 < 8,
                "GPU 在裁剪带外 x={x} 也画了({},{},{})",
                px[i],
                px[i + 1],
                px[i + 2]
            );
        }
        for x in [20u32, 40, 59] {
            let i = ((5 * w + x) * 4) as usize;
            assert!(
                px[i] > 200 && px[i + 1] < 90,
                "GPU 在裁剪带里 x={x} 没画({},{},{})",
                px[i],
                px[i + 1],
                px[i + 2]
            );
        }
    }

    /// GPU 与 CPU 必须画出同一幅画面:布局来自同一份显示列表,
    /// 差异只应出现在抗锯齿采样方式上(线性 vs 盒式)。
    #[test]
    fn gpu_matches_cpu() {
        let cfg = AppConfig::default();
        let (cols, rows) = (52usize, 10usize);
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 96,
        };
        let mut t = crate::term::create_terminal(size, &cfg, Box::new(std::io::sink()));
        t.advance_bytes(
            "GPU 与 CPU 对比:中文 English 混排\r\n\
             \x1b[48;2;137;180;250m\x1b[38;2;30;30;46m \u{f015} ~/dev \u{e0b0}\x1b[0m\r\n\
             \x1b[1m粗体\x1b[0m \x1b[3m斜体\x1b[0m \x1b[4m下划线\x1b[0m \u{2500}\u{2502}\u{250c}\u{2510}\r\n\
             \x1b[38;5;196m256色\x1b[0m \x1b[38;2;120;200;255m真彩色\x1b[0m"
                .as_bytes(),
        );
        let (snap, cursor, palette) = render::snapshot(&t, cols, 0.0);
        let mut r = render::Renderer::new(&cfg, 1.0).unwrap();

        let (w, h) = (
            (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32,
            (rows as f64 * r.cell_h + 2.0 * r.pad_y) as u32,
        );
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

        let mut cpu = vec![0u32; (w * h) as usize];
        r.paint_cpu(&mut cpu, w, h, &frame);

        let mut gpu = match GpuRenderer::new_offscreen(w, h) {
            Ok(g) => g,
            Err(e) => {
                crate::wlog!("跳过 GPU 对比测试:没有可用适配器({e:#})");
                return;
            }
        };
        gpu.draw(&frame, &r).expect("GPU 绘制");
        let px = gpu.read_pixels();
        assert_eq!(px.len(), (w * h * 4) as usize);

        let mut bad = 0usize;
        let mut sum = 0f64;
        for i in 0..(w * h) as usize {
            let c = cpu[i];
            let (r0, g0, b0) = ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
            let (r1, g1, b1) = (px[i * 4] as u32, px[i * 4 + 1] as u32, px[i * 4 + 2] as u32);
            let d = (r0 as i32 - r1 as i32).abs()
                + (g0 as i32 - g1 as i32).abs()
                + (b0 as i32 - b1 as i32).abs();
            sum += d as f64;
            if d > 96 {
                bad += 1;
            }
        }
        let total = (w * h) as f64;
        let mean = sum / total / 3.0;
        let bad_ratio = bad as f64 / total;
        crate::wlog!(
            "GPU vs CPU:{}x{} 平均通道误差 {:.2},差异像素占比 {:.2}%",
            w,
            h,
            mean,
            bad_ratio * 100.0
        );
        assert!(
            mean < 3.0,
            "GPU 与 CPU 输出差异过大(平均 {mean:.2}/255)"
        );
        assert!(
            bad_ratio < 0.05,
            "差异像素占比 {:.1}% 过高",
            bad_ratio * 100.0
        );
    }

    // ---- 适配器选择策略(纯函数,不需要真显卡)----

    #[test]
    fn software_adapters_rank_last() {
        // 默认策略下 WARP/lavapipe 的类型分最高,会被排到最后
        let warp = rank_key(wgpu::Backend::Dx12, wgpu::DeviceType::Cpu, false);
        let igpu = rank_key(wgpu::Backend::Dx12, wgpu::DeviceType::IntegratedGpu, false);
        let dgpu = rank_key(wgpu::Backend::Dx12, wgpu::DeviceType::DiscreteGpu, false);
        assert!(igpu < warp, "核显要排在软件适配器前面");
        assert!(dgpu < warp);
    }

    #[test]
    fn dx12_beats_other_backends() {
        // 同一张卡如果同时出现在 DX12 和 Vulkan 里,选 DX12(Windows 上更成熟)
        let dx = rank_key(wgpu::Backend::Dx12, wgpu::DeviceType::IntegratedGpu, false);
        let vk = rank_key(wgpu::Backend::Vulkan, wgpu::DeviceType::IntegratedGpu, false);
        let gl = rank_key(wgpu::Backend::Gl, wgpu::DeviceType::IntegratedGpu, false);
        assert!(dx < vk && vk < gl);
    }

    #[test]
    fn power_preference_flips_integrated_and_discrete() {
        // 默认(省电)核显优先;VECTORTERM_GPU_POWER=high 时独显优先
        let low_i = rank_key(wgpu::Backend::Dx12, wgpu::DeviceType::IntegratedGpu, false);
        let low_d = rank_key(wgpu::Backend::Dx12, wgpu::DeviceType::DiscreteGpu, false);
        assert!(low_i < low_d);

        let high_i = rank_key(wgpu::Backend::Dx12, wgpu::DeviceType::IntegratedGpu, true);
        let high_d = rank_key(wgpu::Backend::Dx12, wgpu::DeviceType::DiscreteGpu, true);
        assert!(high_d < high_i);
    }

    #[test]
    fn backend_list_covers_the_windows_path() {
        // 只开 DX12 的话,驱动坏掉的机器会直接掉到 CPU;GL 是最后一道保险
        assert!(BACKENDS.contains(wgpu::Backends::DX12));
        assert!(BACKENDS.contains(wgpu::Backends::VULKAN));
        assert!(BACKENDS.contains(wgpu::Backends::GL));
    }
}
