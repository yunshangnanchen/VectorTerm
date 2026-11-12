//! VectorTerm 性能基准套件(仅 `cargo test` 下编译)
//!
//! 运行(必须在 release 下,debug 数字没有可比性):
//! ```bat
//! powershell -ExecutionPolicy Bypass -File benchmark\run.ps1
//! ```
//! 单项(统一走构建入口,它负责 vcvars64 + `CRATE_CC_NO_DEFAULTS=1`):
//! ```bat
//! build.cmd test --release bench_ -- --ignored --nocapture
//! ```
//! 报告、配置与原始数据统一在 `benchmark/`,见 `benchmark/README.md`。
//!
//! 分项:
//! - `bench_frames`        渲染管线分阶段耗时,3 种画面 × 4 种窗口尺寸 × CPU/GPU
//! - `bench_parse`         VT 解析吞吐(ASCII / SGR / CSI / 中文),MB/s
//! - `bench_pty_pipeline`  真实 ConPTY 端到端:cmd.exe 输出 → 读取 → 解析 → 渲染
//! - `bench_env`           打印编译/运行环境,便于复核
//!
//! 所有耗时都是**同一进程内串行整帧计时**(snapshot → 布局 → 呈现),不是把
//! 各阶段拆开单独循环,这样缓存热度与真实主循环一致。GPU 路径额外调用
//! `sync()` 等待设备完成,把异步 submit 变成可测的端到端帧时间。

use crate::config::AppConfig;
use crate::frame::Frame;
use crate::gpu::GpuRenderer;
use crate::render::{self, Renderer};
use crate::term;
use std::fmt::Write as FmtWrite;
use std::hint::black_box;
use std::io::Write as IoWrite;
use std::time::Instant;
use wezterm_term::TerminalSize;

/// 基准产物统一落到 benchmark/data/(可用 VECTORTERM_BENCH_OUT 覆盖)。
/// 报告、配置、原始数据都在同一个目录,避免散落在 target/ 里被清掉。
fn bench_out() -> std::path::PathBuf {
    let p = std::path::PathBuf::from(
        std::env::var("VECTORTERM_BENCH_OUT").unwrap_or_else(|_| "benchmark/data".to_string()),
    );
    let _ = std::fs::create_dir_all(&p);
    p
}

// ---------------------------------------------------------------------------
// 统计
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Stat {
    mean: f64,
    p50: f64,
    p95: f64,
    min: f64,
    max: f64,
}

impl Stat {
    fn of(xs: &[f64]) -> Self {
        let mut v = xs.to_vec();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = v.len().max(1);
        Self {
            mean: v.iter().sum::<f64>() / n as f64,
            p50: v[n / 2],
            p95: v[((n as f64 * 0.95) as usize).min(n - 1)],
            min: v[0],
            max: v[n - 1],
        }
    }
}

/// 每帧各阶段的累加器
#[derive(Default)]
struct Stages {
    snap: Vec<f64>,
    layout: Vec<f64>,
    paint: Vec<f64>,
    gpu_encode: Vec<f64>,
    gpu_wait: Vec<f64>,
    total: Vec<f64>,
}

impl Stages {
    fn push(
        &mut self,
        snap: f64,
        layout: f64,
        paint: f64,
        gpu_encode: f64,
        gpu_wait: f64,
    ) {
        self.snap.push(snap);
        self.layout.push(layout);
        self.paint.push(paint);
        self.gpu_encode.push(gpu_encode);
        self.gpu_wait.push(gpu_wait);
        self.total.push(snap + layout + paint + gpu_encode + gpu_wait);
    }
}

/// 相邻两个采样点之间的毫秒数(用 duration_since,避免多次读时钟引入误差)
fn dur(a: Instant, b: Instant) -> f64 {
    b.duration_since(a).as_secs_f64() * 1000.0
}

// ---------------------------------------------------------------------------
// 测试画面
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scene {
    /// 空屏:只有整屏底色,量出固定成本
    Blank,
    /// 常见 shell/cat 输出:纯 ASCII 文本,单色
    Plain,
    /// 最坏情况:每格都有字形且前景/背景逐格切换(大量 SGR + 大量小矩形)
    Color,
    /// 中文 + emoji,宽字符与回退字体
    Cjk,
}

impl Scene {
    fn name(self) -> &'static str {
        match self {
            Scene::Blank => "blank",
            Scene::Plain => "plain",
            Scene::Color => "color",
            Scene::Cjk => "cjk",
        }
    }
}

/// 生成正好铺满 rows×cols 的 VT 字节流
fn scene_bytes(rows: usize, cols: usize, s: Scene) -> Vec<u8> {
    let mut out = String::with_capacity(rows * cols * 4);
    for r in 0..rows {
        match s {
            Scene::Blank => {}
            Scene::Plain => {
                let base = b"The quick brown fox jumps over the lazy dog 0123456789 ";
                for c in 0..cols {
                    out.push(base[c % base.len()] as char);
                }
            }
            Scene::Color => {
                for c in 0..cols {
                    if (r + c) % 2 == 0 {
                        out.push_str("\x1b[38;5;196mW\x1b[0m");
                    } else {
                        out.push_str("\x1b[48;5;24;38;5;231mM\x1b[0m");
                    }
                }
            }
            Scene::Cjk => {
                let glyphs = ['中', '文', '😀', 'a', 'b', '　'];
                let mut c = 0;
                while c + 1 < cols {
                    out.push(glyphs[(r + c) % glyphs.len()]);
                    c += 2; // 这些字符按宽字符占两格
                }
            }
        }
        if r + 1 < rows {
            out.push_str("\r\n");
        }
    }
    out.into_bytes()
}

fn make_term(rows: usize, cols: usize, cfg: &AppConfig, data: &[u8]) -> wezterm_term::Terminal {
    let size = TerminalSize {
        rows,
        cols,
        pixel_width: cols * 10,
        pixel_height: rows * 20,
        dpi: 96,
    };
    let mut t = term::create_terminal(size, cfg, Box::new(std::io::sink()));
    if !data.is_empty() {
        t.advance_bytes(data);
    }
    t
}

fn canvas_size(r: &Renderer, cols: usize, rows: usize) -> (u32, u32) {
    (
        (cols as f64 * r.cell_w + 2.0 * r.pad_x) as u32,
        (rows as f64 * r.cell_h + 2.0 * r.pad_y) as u32,
    )
}

// ---------------------------------------------------------------------------
// 1. 渲染管线
// ---------------------------------------------------------------------------

/// `build.cmd test --release bench_frames -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_frames() {
    let mut cfg = AppConfig::default();
    cfg.fonts.size = 16.0;
    let sizes: [(usize, usize); 4] = [(80, 24), (120, 40), (200, 60), (300, 80)];
    let scenes = [Scene::Blank, Scene::Plain, Scene::Color, Scene::Cjk];
    let (warmup, iters) = (8usize, 40usize);

    let mut csv = String::from(
        "backend,scene,cols,rows,pixels,mean_ms,p50_ms,p95_ms,min_ms,max_ms,snap_ms,layout_ms,raster_ms,gpu_encode_ms,gpu_wait_ms,theoretical_fps,items\n",
    );

    println!("\n=== 渲染管线基准 (release, 串行整帧计时, warmup={warmup}, iters={iters}) ===");
    println!(
        "{:<5} {:<6} {:<9} {:<11} {:>8} {:>8} {:>8} {:>8}  {:>7}",
        "路径", "画面", "格数", "像素", "均值ms", "p95ms", "最大ms", "理论FPS", "分项"
    );

    for &(cols, rows) in &sizes {
        for scene in scenes {
            let data = scene_bytes(rows, cols, scene);
            let t = make_term(rows, cols, &cfg, &data);
            let mut r = Renderer::new(&cfg, 1.0).unwrap();
            let (w, h) = canvas_size(&r, cols, rows);
            let mut buf = vec![0u32; (w * h) as usize];
            let mut frame = Frame::default();

            // 预热:字形栅格化缓存 + 图集
            let (snap, cursor, pal) = render::snapshot(&t, cols, 0.0);
            r.build_frame(&mut frame, w, h, &snap, &cursor, &pal, None, pal.cursor_bg, true);
            r.paint_cpu(&mut buf, w, h, &frame);
            let mut gpu = GpuRenderer::new_offscreen(w, h).ok();

            // ---------- CPU 路径 ----------
            let mut cpu = Stages::default();
            for _ in 0..warmup {
                let (s, c, p) = render::snapshot(&t, cols, 0.0);
                r.build_frame(&mut frame, w, h, &s, &c, &p, None, p.cursor_bg, true);
                r.paint_cpu(&mut buf, w, h, &frame);
            }
            for _ in 0..iters {
                let t0 = Instant::now();
                let (s, c, p) = render::snapshot(&t, cols, 0.0);
                let t1 = Instant::now();
                r.build_frame(&mut frame, w, h, &s, &c, &p, None, p.cursor_bg, true);
                let t2 = Instant::now();
                r.paint_cpu(&mut buf, w, h, &frame);
                let t3 = Instant::now();
                black_box(buf[0]);
                cpu.push(dur(t0, t1), dur(t1, t2), dur(t2, t3), 0.0, 0.0);
            }
            emit_row(&mut csv, "cpu", scene, cols, rows, w, h, &cpu, frame.items.len());

            // ---------- GPU 路径 ----------
            if let Some(g) = gpu.as_mut() {
                let mut gp = Stages::default();
                for _ in 0..warmup {
                    let (s, c, p) = render::snapshot(&t, cols, 0.0);
                    r.build_frame(&mut frame, w, h, &s, &c, &p, None, p.cursor_bg, true);
                    g.draw(&frame, &r).unwrap();
                    g.sync();
                }
                for _ in 0..iters {
                    let t0 = Instant::now();
                    let (s, c, p) = render::snapshot(&t, cols, 0.0);
                    let t1 = Instant::now();
                    r.build_frame(&mut frame, w, h, &s, &c, &p, None, p.cursor_bg, true);
                    let t2 = Instant::now();
                    g.draw(&frame, &r).unwrap();
                    let t3 = Instant::now();
                    g.sync();
                    let t4 = Instant::now();
                    black_box(frame.items.len());
                    gp.push(
                        dur(t0, t1),
                        dur(t1, t2),
                        0.0,
                        dur(t2, t3),
                        dur(t3, t4),
                    );
                }
                emit_row(&mut csv, "gpu", scene, cols, rows, w, h, &gp, frame.items.len());
            } else {
                println!("  GPU  {:<6} {:<9} —— 无 DX12 适配器,跳过", scene.name(), format!("{cols}x{rows}"));
            }
        }
    }

    let out = bench_out().join("bench-frames.csv");
    if let Ok(mut f) = std::fs::File::create(&out) {
        let _ = f.write_all(csv.as_bytes());
    }
    println!("\n原始数据 -> {}", out.display());
}

#[allow(clippy::too_many_arguments)]
fn emit_row(
    csv: &mut String,
    backend: &str,
    scene: Scene,
    cols: usize,
    rows: usize,
    w: u32,
    h: u32,
    st: &Stages,
    items: usize,
) {
    let tot = Stat::of(&st.total);
    let g = |v: &Vec<f64>| Stat::of(v).mean;
    let _ = writeln!(
        csv,
        "{backend},{},{cols},{rows},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.1},{items}",
        scene.name(),
        w * h,
        tot.mean,
        tot.p50,
        tot.p95,
        tot.min,
        tot.max,
        g(&st.snap),
        g(&st.layout),
        g(&st.paint),
        g(&st.gpu_encode),
        g(&st.gpu_wait),
        1000.0 / tot.mean,
    );
    let detail = if backend == "cpu" {
        format!(
            "snap {:.2} + layout {:.2} + raster {:.2}",
            g(&st.snap),
            g(&st.layout),
            g(&st.paint)
        )
    } else {
        format!(
            "snap {:.2} + layout {:.2} + encode {:.2} + wait {:.2}",
            g(&st.snap),
            g(&st.layout),
            g(&st.gpu_encode),
            g(&st.gpu_wait)
        )
    };
    println!(
        "{:<5} {:<6} {:<9} {:<11} {:>8.2} {:>8.2} {:>8.2} {:>8.0}  {} ms ({} 项)",
        backend,
        scene.name(),
        format!("{cols}x{rows}"),
        format!("{w}x{h}"),
        tot.mean,
        tot.p95,
        tot.max,
        1000.0 / tot.mean,
        detail,
        items,
    );
}

/// 对照实验:`device.poll(Maintain::Wait)` 每帧强制同步会把 DX12 的
/// fence/队列延迟算进"帧时间",而真实窗口路径靠 present + vsync 流水线化,
/// 从不逐帧等待。这里把两种口径分开量,避免误判"GPU 比 CPU 慢"。
///
/// `build.cmd test --release bench_gpu_pipeline -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_gpu_pipeline() {
    let mut cfg = AppConfig::default();
    cfg.fonts.size = 16.0;
    let n = 60usize;
    println!("\n=== GPU 提交口径对照 (color 满屏, N={n}) ===");
    println!(
        "{:<10} {:>12} {:>14} {:>14} {:>12}",
        "格数", "空sync ms", "逐帧sync ms", "流水线/帧 ms", "流水线FPS"
    );
    for (cols, rows) in [(80, 24), (200, 60), (300, 80)] {
        let data = scene_bytes(rows, cols, Scene::Color);
        let t = make_term(rows, cols, &cfg, &data);
        let mut r = Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = canvas_size(&r, cols, rows);
        let mut frame = Frame::default();
        let mut g = GpuRenderer::new_offscreen(w, h).unwrap();
        macro_rules! one_frame {
            () => {{
                let (s, c, p) = render::snapshot(&t, cols, 0.0);
                r.build_frame(&mut frame, w, h, &s, &c, &p, None, p.cursor_bg, true);
                g.draw(&frame, &r).unwrap();
            }};
        }
        for _ in 0..8 {
            one_frame!();
        }
        g.sync();

        // 1) 队列里没有待办工作时,sync 自身的固定开销
        let t0 = Instant::now();
        for _ in 0..n {
            g.sync();
        }
        let idle = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
        g.sync();

        // 2) 每帧都等 GPU:离屏基准的口径(含气泡)
        let t0 = Instant::now();
        for _ in 0..n {
            one_frame!();
            g.sync();
        }
        let per_frame = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
        g.sync();

        // 3) 连续提交 N 帧,最后等一次:真实窗口的流水线口径
        let t0 = Instant::now();
        for _ in 0..n {
            one_frame!();
        }
        g.sync();
        let pipeline = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;

        println!(
            "{:<10} {:>12.3} {:>14.3} {:>14.3} {:>12.0}",
            format!("{cols}x{rows}"),
            idle,
            per_frame,
            pipeline,
            1000.0 / pipeline,
        );
    }
    println!("注:空 sync 若不为 0,说明逐帧同步口径包含 DX12 fence 固定延迟;");
    println!("    真实窗口的帧间隔还受 AutoVsync(通常 60Hz ≈ 16.7ms)限制,见 VECTORTERM_PROFILE_FRAMES 实测。");
}

// ---------------------------------------------------------------------------
// 2. VT 解析吞吐
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Payload {
    Ascii,
    Sgr,
    Csi,
    Unicode,
}

impl Payload {
    fn name(self) -> &'static str {
        match self {
            Payload::Ascii => "ascii",
            Payload::Sgr => "sgr",
            Payload::Csi => "csi(cursor-move)",
            Payload::Unicode => "unicode(cjk)",
        }
    }
}

fn gen_payload(kind: Payload, target: usize) -> Vec<u8> {
    let unit: String = match kind {
        Payload::Ascii => "The quick brown fox jumps over the lazy dog 0123456789 abcdefgh\r\n".into(),
        Payload::Sgr => "\x1b[38;5;196;48;5;24mX\x1b[0m\x1b[1;4mY\x1b[0m".into(),
        Payload::Csi => "\x1b[12;34H\x1b[2CZ\x1b[K".into(),
        Payload::Unicode => "中文测试😀 café naïve \r\n".into(),
    };
    let mut out = Vec::with_capacity(target + unit.len());
    // 按完整单元填充,不 truncate:避免在 UTF-8 或多字节转义序列中间截断
    while out.len() + unit.len() <= target {
        out.extend_from_slice(unit.as_bytes());
    }
    out
}

/// `build.cmd test --release bench_parse -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_parse() {
    let cfg = AppConfig::default();
    let target = 8 * 1024 * 1024usize; // 8 MiB / 每种
    let reps = 3;
    println!("\n=== VT 解析吞吐 (wezterm-term advance_bytes, {:.0} MiB/次, {reps} 次取中位) ===", target as f64 / 1048576.0);
    println!("{:<18} {:>10} {:>10} {:>10}   {}", "负载", "中位MB/s", "最快MB/s", "耗时ms", "说明");

    let mut csv = String::from("payload,bytes,median_mbs,best_mbs,median_ms,runs\n");
    for kind in [Payload::Ascii, Payload::Sgr, Payload::Csi, Payload::Unicode] {
        let data = gen_payload(kind, target);
        let mut speeds = Vec::new();
        let mut times = Vec::new();
        for _ in 0..reps {
            let mut t = make_term(60, 200, &cfg, &[]);
            let t0 = Instant::now();
            t.advance_bytes(&data);
            let dt = t0.elapsed().as_secs_f64();
            // 保证状态机结果真的被用到
            black_box(t.screen().physical_rows);
            times.push(dt * 1000.0);
            speeds.push(target as f64 / dt / 1e6);
        }
        speeds.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = speeds[reps / 2];
        let best = speeds[reps - 1];
        let mut msorted = times.clone();
        msorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "{:<18} {:>10.1} {:>10.1} {:>10.1}   {}",
            kind.name(),
            median,
            best,
            msorted[reps / 2],
            match kind {
                Payload::Ascii => "纯文本,无转义序列",
                Payload::Sgr => "每 1 字符约 2 组 SGR",
                Payload::Csi => "大量光标定位/擦除(TUI 重绘)",
                Payload::Unicode => "宽字符 + emoji + 组合字符",
            }
        );
        let _ = writeln!(
            csv,
            "{},{target},{median:.2},{best:.2},{:.2},{reps}",
            kind.name(),
            msorted[reps / 2]
        );
    }
    let out = bench_out().join("bench-parse.csv");
    if let Ok(mut f) = std::fs::File::create(&out) {
        let _ = f.write_all(csv.as_bytes());
    }
    println!("\n注:这是纯状态机解析(无渲染、无 PTY),口径接近 kitty `kitten __benchmark__` 的解析吞吐。");
    println!("原始数据 -> {}", out.display());
}

// ---------------------------------------------------------------------------
// 3. 真实 ConPTY 端到端
// ---------------------------------------------------------------------------

struct PtyResult {
    secs: f64,
    bytes: usize,
    frames: usize,
    note: String,
    first: String,
}

/// 终端状态机的应答(DSR `\x1b[6n`、DA `\x1b[c` 等)必须回送 PTY 输入。
/// 用 sink 丢掉应答会让 ConPTY 一直等查询响应,shell 输出根本不会开始。
struct ReplyWriter(std::sync::Arc<std::sync::Mutex<Box<dyn IoWrite + Send>>>);

impl IoWrite for ReplyWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write_all(b)?;
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}

/// 把 PTY 读取放到独立线程(与真实主程序的 vectorterm-parser 线程一致),
/// 主线程用 recv_timeout 消费,避免 ConPTY 的阻塞读把基准挂死。
fn run_pty(cfg: &AppConfig, with_render: bool, timeout: std::time::Duration) -> PtyResult {
    use crate::pty;
    use std::io::Read;
    use std::sync::mpsc;

    let mut parts = pty::spawn(cfg, 60, 200, 2000, 1200).expect("打开 ConPTY 失败");
    let mut reader = parts.reader;
    let shared_writer = std::sync::Arc::new(std::sync::Mutex::new(parts.writer));
    let size = TerminalSize {
        rows: 60,
        cols: 200,
        pixel_width: 2000,
        pixel_height: 1200,
        dpi: 96,
    };
    let mut t = term::create_terminal(size, cfg, Box::new(ReplyWriter(shared_writer)));
    let mut r = Renderer::new(cfg, 1.0).unwrap();
    let (w, h) = canvas_size(&r, 200, 60);
    let mut buf = vec![0u32; (w * h) as usize];
    let mut frame = Frame::default();

    let (tx, rx) = mpsc::sync_channel::<Option<Vec<u8>>>(16);
    std::thread::Builder::new()
        .name("bench-pty-reader".into())
        .spawn(move || {
            let mut raw = vec![0u8; 16 * 1024];
            loop {
                match reader.read(&mut raw) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(None);
                        break;
                    }
                    Ok(n) => {
                        if tx.send(Some(raw[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        })
        .unwrap();

    let t0 = Instant::now();
    let (mut bytes, mut frames) = (0usize, 0usize);
    let mut note = String::from("输出流关闭(正常)");
    let mut first = String::new();
    loop {
        let chunk = match rx.recv_timeout(timeout) {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                note = "等待下一块数据超时".into();
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                note = "读取线程退出".into();
                break;
            }
        };
        if first.is_empty() {
            first = String::from_utf8_lossy(&chunk[..chunk.len().min(80)])
                .escape_debug()
                .to_string();
        }
        bytes += chunk.len();
        t.advance_bytes(&chunk);
        if with_render {
            let (s, c, p) = render::snapshot(&t, 200, 0.0);
            r.build_frame(&mut frame, w, h, &s, &c, &p, None, p.cursor_bg, true);
            r.paint_cpu(&mut buf, w, h, &frame);
            frames += 1;
        }
        if t0.elapsed() > timeout {
            note = "总时限到".into();
            break;
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    parts.session.kill();
    PtyResult {
        secs,
        bytes,
        frames,
        note,
        first,
    }
}

/// `build.cmd test --release bench_pty_pipeline -- --ignored --nocapture`
///
/// 与第三方 `cat 16MiB` 基准同口径:子进程把大文件吐进 ConPTY,父进程读取
/// 并喂给 wezterm-term。`VECTORTERM_BENCH_MIB` 可调载荷大小(默认 2 MiB)。
#[test]
#[ignore]
fn bench_pty_pipeline() {
    let mib: usize = std::env::var("VECTORTERM_BENCH_MIB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2)
        .clamp(1, 32);
    let want = mib * 1048576;
    // 与报告/配置同目录;路径含空格,交给 cmd 时用引号包住
    let payload_path = bench_out().join("bench-payload.txt");
    let line = "The quick brown fox jumps over the lazy dog 0123456789 abcdefghijklmnop\r\n";
    let need = std::fs::metadata(&payload_path)
        .map(|m| m.len() != want as u64)
        .unwrap_or(true);
    if need {
        let mut f = std::fs::File::create(&payload_path).unwrap();
        let mut written = 0usize;
        while written + line.len() <= want {
            f.write_all(line.as_bytes()).unwrap();
            written += line.len();
        }
    }

    let mut cfg = AppConfig::default();
    cfg.fonts.size = 16.0;
    cfg.shell = "cmd.exe".to_string();

    // 先用 echo 探一下 ConPTY 通道是否通,避免把"命令没跑起来"当成性能数字
    cfg.cwd = std::env::current_dir().ok();
    cfg.shell_args = vec!["/c".to_string(), "echo VECTORTERM_PTY_PROBE".to_string()];
    let probe = run_pty(&cfg, false, std::time::Duration::from_secs(3));
    println!("\n=== ConPTY 端到端 (cmd.exe type {mib} MiB 文本, 200x60 格) ===");
    println!(
        "探针: {:.0}ms / {} 字节 / {} / 首包={:?}",
        probe.secs * 1000.0,
        probe.bytes,
        probe.note,
        probe.first
    );
    if !probe.note.starts_with("输出流关闭") || probe.secs > 2.0 {
        println!("!! 测试进程内 ConPTY 输出通道不通(只收到初始化序列后阻塞),跳过端到端吞吐;");
        println!("   渲染与解析数据来自独立基准,不受此项影响。真实窗口请用 session_log 校验。");
        return;
    }

    // 相对路径、无空格:CommandBuilder 会转义参数里的双引号,带引号的绝对路径
    // 会被 cmd 看成 `\"F:\...\"` 而找不到文件
    cfg.shell_args = vec![
        "/c".to_string(),
        format!("type {}", payload_path.display()).replace('/', "\\"),
    ];
    println!("{:<24} {:>11} {:>11} {:>10} {:>8}  {}", "负载", "耗时ms", "MB/s", "字节", "帧数", "结果");

    for with_render in [false, true] {
        let p = run_pty(&cfg, with_render, std::time::Duration::from_secs(90));
        println!(
            "{:<24} {:>11.1} {:>11.2} {:>10} {:>8}  {}",
            if with_render { "解析 + 每块渲染 1 帧" } else { "仅 PTY 读取 + 解析" },
            p.secs * 1000.0,
            p.bytes as f64 / p.secs / 1e6,
            p.bytes,
            p.frames,
            p.note,
        );
    }
    println!("\n注:ConPTY 会对输出做 VT 转换/行缓冲,字节数按实际读到的 PTY 流计;");
    println!("    与 Linux 上的 `cat 16MiB` 基准不同平台、不同 PTY 实现,只能量级参照。");
}

// ---------------------------------------------------------------------------
// 4. 大输出洪泛(应用内端到端,不含 OS PTY)
// ---------------------------------------------------------------------------

/// 模拟 `cat 16MiB`:解析线程连续把 16 MiB 文本灌进状态机,主线程按 60Hz
/// 节流渲染(与真实主循环同为"独立解析线程 + 锁内快照 + 锁外绘制")。
/// 观察大输出期间帧时间是否还守得住 16.7ms 预算。
///
/// `build.cmd test --release bench_burst -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_burst() {
    use parking_lot::Mutex; // 与主程序一致:真实 redraw() 拿的是 parking_lot 锁
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let mut cfg = AppConfig::default();
    cfg.fonts.size = 16.0;
    let bytes = 16 * 1024 * 1024usize;
    let data = gen_payload(Payload::Ascii, bytes);
    let step = std::time::Duration::from_micros(16_667);

    println!("\n=== 大输出洪泛 ({:.0} MiB ASCII, 200x60 格, 渲染按 60Hz 节流) ===", bytes as f64 / 1048576.0);
    println!("{:<6} {:>10} {:>10} {:>9} {:>7} {:>10} {:>10} {:>10} {:>9}", "后端", "墙钟ms", "解析ms", "MB/s", "帧数", "等锁ms", "快照ms", "绘制ms", "超预算帧");

    for gpu_mode in [false, true] {
        let term = Arc::new(Mutex::new(make_term(60, 200, &cfg, &[])));
        // 先把 scrollback 灌到稳态(默认 3500 行上限):wezterm-term 在回看缓冲
        // 尚未填满时取快照会在 screen.rs:932 越界,而真实终端长时间运行后
        // 总是处于"scrollback 已满"的状态,预热后基准才代表常态。
        {
            let mut t = term.lock();
            t.advance_bytes(&gen_payload(Payload::Ascii, 4000 * 68));
        }
        let mut r = Renderer::new(&cfg, 1.0).unwrap();
        let (w, h) = canvas_size(&r, 200, 60);
        let mut buf = vec![0u32; (w * h) as usize];
        let mut frame = Frame::default();
        let mut gpu = if gpu_mode {
            GpuRenderer::new_offscreen(w, h).ok()
        } else {
            None
        };
        // 预热字形缓存与图集
        {
            let t = term.lock();
            let (s, c, p) = render::snapshot(&t, 200, 0.0);
            r.build_frame(&mut frame, w, h, &s, &c, &p, None, p.cursor_bg, true);
            r.paint_cpu(&mut buf, w, h, &frame);
        }

        let done = Arc::new(AtomicBool::new(false));
        let (term2, done2, data2) = (term.clone(), done.clone(), data.clone());
        let parser = std::thread::Builder::new()
            .name("bench-burst-parser".into())
            .spawn(move || {
                let t0 = Instant::now();
                for chunk in data2.chunks(16 * 1024) {
                    term2.lock().advance_bytes(chunk);
                }
                done2.store(true, Ordering::SeqCst);
                t0.elapsed()
            })
            .unwrap();

        let (mut waits, mut snaps, mut draws, mut costs) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let wall0 = Instant::now();
        let mut next = Instant::now() + step;
        loop {
            let now = Instant::now();
            if now < next {
                std::thread::sleep(next - now);
            }
            let f0 = Instant::now();
            let l0 = Instant::now();
            let guard = term.lock();
            let wait = dur(l0, Instant::now());
            let s0 = Instant::now();
            let (s, c, p) = render::snapshot(&guard, 200, 0.0);
            let snap = dur(s0, Instant::now());
            drop(guard);
            let d0 = Instant::now();
            r.build_frame(&mut frame, w, h, &s, &c, &p, None, p.cursor_bg, true);
            if let Some(g) = gpu.as_mut() {
                g.draw(&frame, &r).unwrap();
            } else {
                r.paint_cpu(&mut buf, w, h, &frame);
            }
            waits.push(wait);
            snaps.push(snap);
            draws.push(dur(d0, Instant::now()));
            costs.push(dur(f0, Instant::now()));
            if done.load(Ordering::SeqCst) {
                break;
            }
            next += step;
            if next < Instant::now() {
                next = Instant::now(); // 掉帧后不补帧,和真实主循环一致
            }
        }
        let wall = wall0.elapsed();
        let parse = parser.join().unwrap();
        let avg = |v: &Vec<f64>| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let over = costs.iter().filter(|&&v| v > 16.667).count();
        println!(
            "{:<6} {:>10.1} {:>10.1} {:>9.1} {:>7} {:>10.2} {:>10.2} {:>10.2} {:>9}/{}",
            if gpu_mode { "gpu" } else { "cpu" },
            wall.as_secs_f64() * 1000.0,
            parse.as_secs_f64() * 1000.0,
            bytes as f64 / wall.as_secs_f64() / 1e6,
            costs.len(),
            avg(&waits),
            avg(&snaps),
            avg(&draws),
            over,
            costs.len(),
        );
        if let Some(g) = gpu.as_ref() {
            g.sync();
        }
    }
    println!("注:不含 ConPTY 转换/子进程启动,是应用内部链路的下界;跨平台的 `cat 16MiB` 数字不可直接相减。");
}

// ---------------------------------------------------------------------------
// 5. 环境
// ---------------------------------------------------------------------------

/// 回归:scrollback 填满并让 VecDeque 回绕后,取快照不能越界。
/// 上游 `Screen::with_phys_lines` 曾用绝对物理行号索引第二段切片,
/// 长时间运行的终端一滚动就会 panic;补丁见 patches/。
#[test]
fn snapshot_survives_scrollback_wrap() {
    let cfg = AppConfig::default();
    let data = gen_payload(Payload::Ascii, 5000 * 68); // > 3500 行回看上限
    let t = make_term(60, 200, &cfg, &data);
    let (bottom, _, _) = render::snapshot(&t, 200, 0.0);
    assert_eq!(bottom.rows.len(), 60);
    // 亚行滚动会多取一行填底部,所以是 >= 而不是 == 60
    let (scrolled, _, _) = render::snapshot(&t, 200, 100.5);
    assert!(scrolled.rows.len() >= 60);
}

#[test]
#[ignore]
fn bench_env() {
    println!("\n=== 基准环境 ===");
    println!("crate      : {} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    println!("profile    : {}", if cfg!(debug_assertions) { "DEBUG(不可用于对比)" } else { "RELEASE (lto=thin)" });
    println!("target     : {} / {}", std::env::consts::OS, std::env::consts::ARCH);
    println!("cpu        : {}", std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "?".into()));
    println!("logical cpu: {}", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0));
    if let Ok(g) = GpuRenderer::new_offscreen(64, 64) {
        println!("gpu        : {}", g.adapter_name());
    } else {
        println!("gpu        : 无 DX12 适配器");
    }
}
