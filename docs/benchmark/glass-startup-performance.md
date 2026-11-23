# 毛玻璃启动与运行时性能评估

被测对象:`config/deepseek-glass.lua` / `config/deepseek-glass-minimal.lua`
(Acrylic 材质 + 逐像素 alpha 合成交换链)。只关心一件事:**开毛玻璃到底贵多少、
贵在哪一步、先改哪里**。

> 这份是**本机实测**,数字与 `terminal-performance-report.md` 的公开横评不可比,
> 与 `VectorTerm-performance-benchmark.md` 的 1920×1080 数据也不可直接相减
> (尺寸、配置、采样方式不同)。原始日志见 `data/glass-startup-2026-11-22.txt`。

## 1. 测量环境

| 项 | 值 |
|---|---|
| CPU / GPU | Intel Core Ultra 7 258V / Intel 核显(Arc 140V),DX12 |
| 系统 / 缩放 | Windows 11,125% |
| 窗口 | 1250×750 物理像素,14 号字,111×30 格(预设原样) |
| 构建 | `build.cmd build --release`(2026-11-22 22:44 的 exe),含 347 项测试的同一棵树 |
| 采样 | `VECTORTERM_PROFILE_FRAMES` + 应用日志时间戳(毫秒);启动 3 次、帧率 2 次取区间 |
| 桌面 | 同一张壁纸,窗口位置一致(减掉壁纸差异对"启动耗时"无影响,只影响下面的配色实测) |

`first_frame_ms` 是程序内计时(进程启动 → 首帧 `present` 返回),`window=` 那行
是"窗口建好 + 字体度量就绪",两者之间就是 GPU 后端的初始化。

## 2. 结论

| 场景 | 首帧 | 稳态 p50 | 稳态 fps | 常驻内存(8s) |
|---|---|---|---|---|
| CPU 软渲染(`deepseek.lua`) | **0.66~0.73 s** | 1.6 ms | 254~277 | **82 MB** |
| GPU 不透明(同配置 `render = "auto"`) | 1.79~1.91 s | 3.5~3.8 ms | 206~222 | 553~561 MB |
| 毛玻璃(`deepseek-glass.lua`) | **1.96~2.33 s** | 4.1~6.0 ms | 172~184 | 547~562 MB |

> 首帧区间的下沿 1.96 s 是驱动着色器/PSO 缓存已经预热过的状态;冷机第一次
> 启动是 2.3 s 那一档。同一个 exe 反复跑会稳定在下沿。

三句话:

1. **玻璃材质本身几乎不花钱。** 只开 Acrylic、窗口仍不透明时(`[C]` 组),
   首帧与帧时间和"完全不开材质"(`[B]` 组)的差别都落在噪声里。真正多出来的
   是逐像素 alpha 那条合成交换链:首帧 +0.17 s、每帧 +1~2 ms。大头不在这里。
2. **贵的是 GPU 后端初始化和启动期重复探测。** 不透明 GPU 那条路里
   "窗口就绪 → 后端就绪"是 0.99 s,枚举适配器就发生在这一段;玻璃把枚举
   提前到建窗之前(0.61 s),后面那段剩 0.59 s —— 同一件事**付了两遍**,
   只是分在两个阶段里(见 §4.1)。
3. **内存不是玻璃的锅。** GPU 后端一上来就 +480 MB,玻璃与不透明完全一样;
   这是 GPU 路径自身的问题,和材质无关。

## 3. 启动时间线(毛玻璃,缓存预热后的三次一致采样)

| 阶段 | 累计时刻 | 本段耗时 | 依据 |
|---|---|---|---|
| 进程启动 → 控制台/完整性检查 | 0.03 s | 0.03 s | `完整性级别:` 行 |
| 配置求值 + 建窗 + `Renderer::new`(字体度量) | 0.60 s | 0.57 s | 纯净配置(`config/config.lua`)实测 0.56 s |
| **`glass_will_use_gpu` 硬件探测** | **1.21 s** | **0.61 s** | 玻璃独有:窗口是"探完才建"的 |
| DX12 设备 + DComp visual + 合成交换链 configure | 1.76 s | 0.55 s | `呈现模式 ...` 行 |
| 图集 / 缓冲 / 着色器 / 两条管线 | 1.80 s | 0.04 s | `渲染后端:GPU(...)` 行 |
| 首帧绘制与呈现 | 1.97 s | 0.17 s | `first_frame_ms` |

0.55 s 那段里其实还夹着**第二遍** `enumerate_adapters`(`build()` 自己那次);
探测刚跑过,驱动侧缓存是热的,这一遍只要 0.1 s 左右 —— 冷机上会更贵,
这正是 §4.1 要消掉的东西。

> 感知上更糟:玻璃窗口是 `with_visible(false)` 建的,后端就绪才显示 ——
> 这 2 s 里用户看到的是**什么都没有**,而不是"先出来再变清楚"。

## 4. 根因

### 4.1 启动期把适配器枚举做了两遍

`src/gpu.rs`

* `GpuRenderer::hardware_adapter_available()` —— 窗口创建**之前**先问一次
  "这台机器有没有可用 GPU",决定要不要给 `WS_EX_NOREDIRECTIONBITMAP`
  (给了 GDI 就画不上东西,所以必须先确认)。它自己 `Instance::new` +
  `enumerate_adapters`。
* `GpuRenderer::build()` —— 真正建后端时**再** `Instance::new` +
  `enumerate_adapters` 一次。

DX12 上单次枚举实测 0.4~0.7 s(驱动要为每个适配器建 DXGI 对象、查能力)。
而它只是**查询**,结果在进程生命周期内不会变 —— 第二次开始完全是白等。

### 4.2 GPU 初始化同步阻塞,且发生在窗口显示之前

`build()` 里 `request_device` → `create_surface` → `configure` → 建图集/缓冲区 →
`create_shader_module` ×2 → `create_render_pipeline` ×2,全部同步,窗口此时还藏着。

实测这 0.6 s 里**几乎全是设备与合成交换链**:从"窗口就绪"到 `呈现模式` 那行
是 0.55 s(其中约 0.1 s 是热缓存的第二遍枚举),而从 `呈现模式` 到"渲染后端就绪"
只有 **0.04 s** —— 也就是说 DXIL 编译、两条管线、图集纹理加起来才 40 ms
(缓存在驱动侧),不是瓶颈。真正贵的是 `request_device` 与
`CreateSwapChainForComposition` 这条路本身。

### 4.3 每帧一次额外的 DirectComposition 提交

`present()` 末尾 `IDCompositionDevice::Commit()`(逐像素 alpha 路线必需 ——
`Present` 只更新内容,`Commit` 才交给 DWM)。实测玻璃比不透明每帧多 1~2 ms,
其中大部分是这条额外的合成往返。

### 4.4 常驻内存 +480 MB(GPU 路径共有)

`request_device` 用的是 `wgpu::MemoryHints::default()`,wgpu 24 默认是
`Performance`(为吞吐预留大块堆);再加上 Intel 驱动的私有分配。
CPU 后端 82 MB,GPU 后端 553~561 MB,玻璃与不透明无差别。

## 5. 优化方案(按"收益 ÷ 风险"排序)

### P0 探测挪到后台线程 + 结果复用 —— 预计 -0.4~0.6 s

两个动作,配合起来才划算:

1. **并行**:`glass_will_use_gpu` 只依赖 `cfg.gpu_power`,不依赖窗口。
   它那 0.6 s 完全可以和后面 0.57 s 的"配置求值 + 字体度量"重叠 ——
   在 `main` 里先 `thread::spawn` 出去,建窗之前再 `join`。
2. **复用**:探测算出的 `(Instance, 候选列表)` 缓存进 `OnceLock`,`build()`
   直接拿来用,不再枚举第二遍(热机 0.1 s、冷机 0.3~0.5 s)。

```rust
// gpu.rs
static PROBE: OnceLock<(wgpu::Instance, Vec<(wgpu::Adapter, String)>)> = OnceLock::new();

pub fn probe_candidates(prefer_discrete: bool)
    -> &'static (wgpu::Instance, Vec<(wgpu::Adapter, String)>)
{
    PROBE.get_or_init(|| {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: BACKENDS,
            ..Default::default()
        });
        let candidates = rank_adapters(&instance, &AdapterPolicy::from_env(prefer_discrete));
        (instance, candidates)
    })
}
```

`main.rs` 里:

```rust
let probe = std::thread::spawn(move || gpu::probe_candidates(prefer));
// …配置求值 / Renderer::new / 其它窗口属性,这段时间探测在后台跑…
let probed = probe.join().expect("探测线程 panic");
let has_gpu = !probed.1.is_empty();
```

`build()` 里把 `Instance::new` + `rank_adapters` 换成 `probe_candidates(prefer)`
的克隆(必须连 `Instance` 一起复用:交换链要建在同一个实例上,只存 bool 还得
再枚举一次,等于没省)。

* 风险:低。线程只碰 wgpu,不碰窗口;`join` 在建窗之前,失败路径与现在一致
  (探测失败 → 不给 `WS_EX_NOREDIRECTIONBITMAP` → 走 CPU)。
* 验证:窗口那行应从 1.21 s 回到 0.65~0.75 s(探测被藏在配置阶段里);
  `渲染后端:` 行不变,画面完全一样。

### P1 先亮窗、后补内容 —— 感知 -0.2~0.5 s(可选,需要产品判断)

P0 之后窗口在 1.2 s 建好、1.45 s 出内容,空窗只剩 0.25 s,这条的收益就小了。
仍可做的版本是:窗口一建好就 `apply_system_backdrop` + `set_visible(true)`,
后端就绪后再正常绘制 —— 用户先看到一块圆角磨砂玻璃(桌面透出来、模糊着),
文字随后浮现,而不是"什么都没有"然后整块出现。

* 风险:中低。空窗期间 `WM_SIZING` 钩子、输入法上下文、拖动带都已就绪;
  要确认没有交换链时 `paint` 不会被调用。
* 验证:录屏数"窗口出现 / 首个字形出现"两个时刻。

### P2 启动基线 0.57 s:先打点,再动刀 —— 收益待测

玻璃与 CPU 共用同一段 0.57 s(配置求值 + 建窗 + `Renderer::new`)。里面至少有
lua 求值、主题深合并、内嵌字体解析、字形度量/图集预热、窗口创建五件事,
但现在**没有分段计时**,不拆开就没法判断该动哪一个。

建议先加一组只在 `VECTORTERM_PROFILE_FRAMES` 下开启的 `Instant` 打点
(`config_loaded` / `window_created` / `renderer_ready`),跑一次拿到拆分,
再决定是缓存字形度量、懒加载非 ASCII 字体,还是别的。

### P3 稳态:合并管线与 present 路径 —— 每帧 -0.5~1 ms

* 两条管线(`quad` / `quad_bg`)只差 alpha 混合状态,着色器相同。可以合成一条
  管线 + 一个 uniform 分支,少一次 `set_pipeline` 与一次状态切换(段边界上本来
  就频繁切)。
* `Commit()` 现在每帧都调;只在"内容真的换了"时提交(有帧跳过绘制时)能省一次
  合成往返。玻璃比不透明每帧多的 1~2 ms 主要就在这里。

### P4 内存:`MemoryHints::MemoryUsage` —— 待实测

`wgpu::MemoryHints::default()` 换成 `MemoryHints::MemoryUsage`,预期能压掉
一部分预留堆;若驱动私有分配才是大头,则收益有限。这条**必须先量再改**,
不要凭"应该会小"就动。注意 480 MB 是 GPU 路径共有,不是玻璃引入的。

## 6. 不建议做的事

* **CI 里缓存 `target/`**:release + LTO 的产物几个 GB,10 GB 额度撑不住,
  冷编译一次反而更干净(现有 workflow 的注释已经写了这个判断)。
* **用 Mica 换 Acrylic 省性能**:两者的合成成本都在 DWM 侧,差别是"采壁纸"还是
  "实时模糊",实测首帧与帧时间都在噪声内;而 Mica 失焦会掉、观感也不对。
* **靠调低 `window_background_opacity` 提速**:透明度只影响混合结果的一个常数,
  与像素数量无关,实测 0.55 与 1.0 的帧时间没有可分辨差异。
* **关 vsync 换帧率**:玻璃路径每帧要过一次 DWM 合成,关 vsync 只会把撕裂和
  `Commit` 次数一起放大。

## 7. 复现

```bat
build.cmd build --release

set VECTORTERM_DEBUG_CONSOLE=0
set VECTORTERM_PROFILE_FRAMES=200
target\release\VectorTerm.exe --config config\deepseek-glass.lua
```

日志在 `%USERPROFILE%\.config\VectorTerm\VectorTerm.log`,每行带毫秒时间戳;
`first_frame_ms` 与 `backend=... frames=... p50=...` 两行就是要抄的数。

## 附:配色实测(底色由夜空蓝改为石墨灰)

同一位置、同一张壁纸下的窗口内空白像素:

| | 均值 RGB | 色相 | 饱和度 | 蓝偏 `B-(R+G)/2` |
|---|---|---|---|---|
| 改前(夜空蓝 `#0D1826`) | (42,46,57) | 220° | 28.6% | **+13** |
| 改后(石墨灰 `#15181D`) | (27,30,32) | 202° | 13.6% | **+3.5** |
| 参考:窗口外的桌面本身 | (31,29,35) | 260° | 19.2% | +5 |

结论:发蓝不是"蓝色的蒙版" —— 代码里 accent 模糊的渐变色一直是 `0`(不着色),
蓝全部来自预设的 `window_background_color` / `window_background_gradient` /
`ansi[0]` / 标签栏底色这几个值。改成中性灰之后,玻璃叠在桌面上只比桌面本身
多约 `-1.5` 的蓝偏(视觉上是中性偏冷),品牌蓝只留在光标、选区、活动标签上。
