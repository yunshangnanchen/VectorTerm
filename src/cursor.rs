//! Kitty 风格光标尾迹 —— `kitty/cursor_trail.c` 的移植,外加"积压输入"的合并与限速。
//!
//! 关键认识(与直觉相反):kitty 的尾迹**不是**一串渐隐的光标方块,也**不是**让
//! 光标块平滑滑行。它是:
//!
//! 1. 光标本体**立即**出现在目标位置(不滑行);
//! 2. 尾迹是**一个被拉伸/剪切的半透明四边形**:光标矩形的 4 个角各自以指数速度
//!    追赶自己的目标角,运动**前方**的角衰减快(迅速归位)、**后方**的角衰减慢
//!    (拉出长尾),于是四边形被拉长成一条"尾巴";
//! 3. 用纯色 × opacity 填充,并把**当前光标矩形区域挖空**,由光标本体保持清晰;
//! 4. 触发门:`cursor_trail` 是**毫秒停留时间**(光标静止够久才跟随),
//!    `cursor_trail_start_threshold` 是每轴的最小格数(小于则直接吸附,无尾迹)。
//!
//! # 连续输入:只保留最新目标,并给每段动画设上限
//!
//! 按住回车时,PTY 会以 ~30Hz 输出换行,光标每 33ms 就下移一行。如果按"每次移动
//! 生成一个动画"的思路实现,这些动画就会排队,逐个播完才轮到下一个,表现为光标
//! 后面拖着一长串旧动画、完全跟不上手。kitty / 现代终端的做法是**状态驱动**:
//!
//! 1. **重定向(retarget)而不是排队**:动画永远只有一个目标,新位置直接覆盖旧
//!    目标,中间位置从不入队,所以并发动画数恒为 1;
//! 2. **积压感知加速**:连续重定向会累积 `burst`(封顶),把四个角的衰减时间常数
//!    按倍率缩短,输入越快、积压越多,追得越快,松手后又平滑回落;
//! 3. **距离自适应**:长距离跳跃会按剩余格数额外加速,避免固定衰减在几十行位移
//!    上拖出一秒多的尾巴;
//! 4. **硬上限(`trail_max`)**:一段动画从开始到结束有时间上限,到点还没收敛就
//!    直接吸附。哪怕输入密到理论上永远追不上,也绝不会攒出"要逐个播完"的队列;
//! 5. **持久输入的兜底结算**:如果光标动得太快、dwell(停留时间)永远等不到,
//!    每 `max(dwell*3, 50ms)` 强制按最新位置结算一次,不让尾迹被冻住。

use crate::config::CursorConfig;
use std::time::{Duration, Instant};

/// `burst` 上限:最多记住这么多次连续重定向(再快也不会无限加速)
const MAX_BURST: f32 = 12.0;
/// `burst` 自然回落的时间常数(秒)
const BURST_TAU: f32 = 0.35;
/// 速度倍率上限
const MAX_SPEEDUP: f32 = 12.0;
/// 连续输入时尾迹的最大长度(格):保证松手时落后不到一行
const TAIL_CAP_CELLS: f32 = 1.0;
/// 连续输入停止多久后彻底收掉尾迹(松手即结束,不再"播完剩下的动画")
const SETTLE_AFTER: Duration = Duration::from_millis(40);
/// 倍率的平滑时间常数(秒):加速要快(跟手),减速要慢(不抽搐)
const SPEEDUP_UP_TAU: f32 = 0.02;
const SPEEDUP_DOWN_TAU: f32 = 0.09;
/// 收敛判定的像素阈值
const SETTLE_PX: f32 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point2 {
    pub x: f32,
    pub y: f32,
}

/// 单元格的像素矩形(供渲染层使用)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Point2 {
    fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// 一帧的尾迹绘制数据(像素坐标,顺序:右上、右下、左下、左上)
#[derive(Debug, Clone, Copy)]
pub struct TrailDraw {
    pub corners: [Point2; 4],
    pub opacity: f32,
}

/// 矩形 corners:[x0, x1, y0, y1](左上/右上/左下/右下)
pub fn corners_of(rect: &[f32; 4]) -> [Point2; 4] {
    [
        Point2::new(rect[1], rect[2]), // 右上
        Point2::new(rect[1], rect[3]), // 右下
        Point2::new(rect[0], rect[3]), // 左下
        Point2::new(rect[0], rect[2]), // 左上
    ]
}

fn rect_eq(a: &[f32; 4], b: &[f32; 4]) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 0.01)
}

pub struct CursorTrail {
    enabled: bool,
    dwell: Duration,
    /// 持续移动时强制结算的最长间隔(`max(dwell*3, 50ms)`)
    max_wait: Duration,
    /// 连续输入停止后,多久把尾迹一次收掉
    settle_after: Duration,
    thr_x: i64,
    thr_y: i64,
    decay_fast: f32,
    decay_slow: f32,
    /// 单段动画的硬上限;ZERO = 不限制
    max_ms: f32,

    initialized: bool,
    /// 当前目标光标矩形(动画的"吸引子",始终是最新位置)
    edge: [f32; 4],
    /// 最近观察到的光标矩形
    pending: [f32; 4],
    /// 光标最近一次改变位置的时刻
    last_change: Instant,
    /// 最近一次把 pending 结算成 edge 的时刻
    last_retarget: Instant,
    /// 四边形四个角的当前位置
    corners: [Point2; 4],
    opacity: f32,
    updated_at: Instant,

    /// 连续重定向的积压量:输入越密越大,静止后指数回落
    burst: f32,
    /// 本次连续输入是否已达到"密集"级别(松手后必须收尾,即使 burst 已回落)
    was_dense: bool,
    /// 平滑后的速度倍率(衰减时间常数会除以它)
    speedup: f32,
    /// 本段动画的起始时刻;None = 已收敛(下一段重新计时)
    motion_start: Option<Instant>,

    /// 是否还需要继续渲染尾迹
    pub needs_render: bool,
    /// 是否还需要继续调度帧(动画中,或正在等待"停留时间"到期)
    needs_frames: bool,
    /// 固定速度倍率;None = 按积压 / 距离自适应算。
    ///
    /// 跨窗格那条尾迹用它锁在 1.0:跨屏距离动辄几十格,自适应公式会立刻把
    /// 倍率顶到上限,decay 被缩到 1/4,四边形一两帧就收完 —— 屏幕上看不到尾巴。
    /// 锁成 1.0 之后,它和"窗格里挪一两行"是同一个速度,只是拉得更长。
    fixed_speedup: Option<f32>,
    /// 单帧最大推进量(秒);None = 用默认的 0.25s 上限。
    ///
    /// 只有跨窗格那条尾迹会收紧它:切换焦点的那一帧如果刚好掉帧,一次几十毫秒的
    /// dt 会让四边形**一步到位**,屏幕上看不到任何尾迹。
    dt_cap: Option<f32>,
}

impl CursorTrail {
    pub fn new(cfg: &CursorConfig) -> Self {
        let dwell = Duration::from_secs_f32((cfg.trail_dwell_ms / 1000.0).max(0.0));
        Self {
            enabled: cfg.trail_dwell_ms > 0.0,
            dwell,
            // 持续输入时的强制结算间隔:即使把 dwell 配得很大,也不能让尾迹
            // 冻住超过 120ms(否则按住回车时会看到"光标走了、尾巴还在原地")
            max_wait: (dwell * 3).clamp(
                Duration::from_millis(50),
                Duration::from_millis(120),
            ),
            // 松手后最多再留 40ms(约 2~3 帧)的尾巴惯性,之后直接收掉
            settle_after: SETTLE_AFTER,
            thr_x: cfg.trail_start_x,
            thr_y: cfg.trail_start_y,
            decay_fast: cfg.trail_decay_fast.max(0.01),
            decay_slow: cfg.trail_decay_slow.max(cfg.trail_decay_fast.max(0.01)),
            max_ms: cfg.trail_max_ms.max(0.0),
            initialized: false,
            edge: [0.0; 4],
            pending: [0.0; 4],
            last_change: Instant::now(),
            last_retarget: Instant::now(),
            corners: [Point2::new(0.0, 0.0); 4],
            opacity: 0.0,
            updated_at: Instant::now(),
            burst: 0.0,
            was_dense: false,
            speedup: 1.0,
            motion_start: None,
            needs_render: false,
            needs_frames: false,
            fixed_speedup: None,
            dt_cap: None,
        }
    }

    /// 锁死速度倍率(跨窗格尾迹用 1.0,和窗格内小位移同一个节奏)。
    pub fn set_fixed_speedup(&mut self, s: f32) {
        self.fixed_speedup = Some(s.clamp(1.0, MAX_SPEEDUP));
    }

    /// 收紧单帧推进量。跨窗格尾迹用 1/60 秒:即便首帧卡顿,也是按"一帧"推进,
    /// 拉开的四边形能正常显示出来;窗格内尾迹不设这一层。
    pub fn set_dt_cap(&mut self, cap: f32) {
        self.dt_cap = Some(cap.max(0.001));
    }

    /// 窗口尺寸变化时调用:丢弃动画状态(像素坐标已失效)
    pub fn reset(&mut self) {
        self.initialized = false;
        self.needs_render = false;
        self.needs_frames = false;
        self.opacity = 0.0;
        self.burst = 0.0;
        self.was_dense = false;
        self.speedup = 1.0;
        self.motion_start = None;
    }

    /// 键盘松开:如果刚才是一段密集输入,立刻把尾迹收掉。
    ///
    /// 比 40ms 定时收尾更"即时":用户松开回车的那一帧,屏幕上不该还有回车
    /// 留下的动画在跑。单次按键不受影响(`was_dense` 只在连续重定向时置位)。
    /// 返回是否真的改变了状态(供调用方决定是否重绘)。
    pub fn key_released(&mut self) -> bool {
        if !self.enabled || !self.was_dense {
            return false;
        }
        // 最后一帧可能还没把最新光标结算成目标:直接取 pending,避免松手后又追一下
        if !rect_eq(&self.edge, &self.pending) {
            self.edge = self.pending;
        }
        self.corners = corners_of(&self.edge);
        self.motion_start = None;
        self.was_dense = false;
        self.needs_render = false;
        self.needs_frames = false;
        true
    }

    /// 尾迹是否仍在收敛(供测试与调试使用;调度帧请用 `needs_frames()`)
    #[allow(dead_code)]
    pub fn is_animating(&self) -> bool {
        self.needs_render
    }

    /// 把尾迹"种"到 `from`(光标刚从别处跳过来的位置),之后每次 `update`
    /// 都会让它收拢到真实光标。
    ///
    /// 跨窗格切换用这个:不在外面另写一套飞行动画,而是**复用同一个状态机** ——
    /// 停留时间、起止阈值、距离加速、淡入曲线、`max_ms` 硬上限全都是窗格内
    /// 尾迹那一套,所以两处的手感(速度 / 颜色浓度 / 拖尾长度)天然一致。
    /// `from` 的坐标系与后续 `update` 的 `rect` 相同(这里都是内容区像素)。
    pub fn seed(&mut self, from: [f32; 4], now: Instant) {
        self.initialized = true;
        self.edge = from;
        self.pending = from;
        self.corners = corners_of(&from);
        // 与 `update` 的初始化分支同一口径:起点可见就直接是满不透明度
        self.opacity = 1.0;
        self.last_change = now;
        self.last_retarget = now;
        self.updated_at = now;
        self.burst = 0.0;
        self.was_dense = false;
        self.speedup = 1.0;
        self.motion_start = None;
        // 下一帧 update 会把 edge 换成真实光标,这里先声明"还要出帧",
        // 否则调度器会在切换的那一帧就停下来,尾迹永远不启动。
        self.needs_render = false;
        self.needs_frames = self.enabled;
    }

    /// 当前速度倍率(供测试使用)
    #[allow(dead_code)]
    pub fn speedup(&self) -> f32 {
        self.speedup
    }

    /// 是否处于"连续输入"(回车连发)状态。
    ///
    /// 滚动动画用它决定回底要不要抢时间:长按时视口必须赶在下一批输出之前
    /// 就位,否则视口会一直被新行推着走,看起来像"松手了还在滚"。
    pub fn is_dense(&self) -> bool {
        self.burst >= 2.0
    }

    /// 是否还需要继续出帧(包括"等待停留时间到期"的阶段)。
    ///
    /// 这一点很关键:回车后光标已停在最终位置、应用也不再输出时,如果只凭
    /// `needs_render` 决定是否调度帧,等待 dwell 的那段时间里不会有任何事件,
    /// 尾迹就永远不会启动 —— 表现为"回车时有时没有动画"。
    pub fn needs_frames(&self) -> bool {
        self.needs_frames
    }

    /// 每帧推进。`rect` = 光标当前矩形(像素),返回需要绘制的尾迹四边形。
    pub fn update(
        &mut self,
        now: Instant,
        rect: [f32; 4],
        visible: bool,
        cell_w: f32,
        cell_h: f32,
    ) -> Option<TrailDraw> {
        // 掉帧时不要按"截断后的 dt"慢慢补,否则一次卡顿就会把动画拉长。
        let dt = now
            .saturating_duration_since(self.updated_at)
            .as_secs_f32()
            .min(self.dt_cap.unwrap_or(0.25))
            .min(0.25);
        self.updated_at = now;

        if !self.enabled {
            self.needs_render = false;
            self.needs_frames = false;
            self.opacity = 0.0;
            return None;
        }

        if !self.initialized {
            self.initialized = true;
            self.edge = rect;
            self.pending = rect;
            self.corners = corners_of(&self.edge);
            self.opacity = if visible { 1.0 } else { 0.0 };
            self.last_change = now;
            self.last_retarget = now;
            self.needs_render = false;
            self.needs_frames = false;
            return None;
        }

        // 1) 记录观察到的光标位移。注意:这里只覆盖 pending,绝不入队 ——
        //    无论两帧之间移动了多少次,动画目标永远是最新位置。
        if !rect_eq(&rect, &self.pending) {
            // 与上一次移动隔了足够久 → 这是一段全新的输入,不要继承上一段的
            // 积压/收尾状态(否则松手后再按一次回车会被当成"密集输入"直接吞掉)
            if now.saturating_duration_since(self.last_change) >= self.settle_after {
                self.burst = 0.0;
                self.was_dense = false;
            }
            self.pending = rect;
            self.last_change = now;
            // 连续重定向计数:快速输入(按住回车)会让它迅速涨到上限
            self.burst = (self.burst + 1.0).min(MAX_BURST);
        }
        // 没有新目标时缓慢回落,松手后动画自然变"软"
        self.burst *= (-dt / BURST_TAU).exp();

        // 2) 触发门:光标在同一位置停留 ≥ dwell 后才让尾迹跟随。
        //    但如果光标动得太快、dwell 永远等不到,就每 max_wait 强制结算一次,
        //    否则尾迹会被冻结在旧位置,松手后再一次性猛追几十行。
        let waited = now.saturating_duration_since(self.last_change);
        let since_retarget = now.saturating_duration_since(self.last_retarget);
        let force = self.burst >= 2.5 && since_retarget >= self.max_wait;
        // 连续输入(eager)时不再等 dwell:每次移动立刻重定向,否则 dwell 会让
        // 已结算目标落后真实光标一行,叠加尾迹长度就超过"一行以内"的要求。
        let eager = self.burst >= 2.0;
        let target_changed = !rect_eq(&self.edge, &self.pending)
            && (eager || waited >= self.dwell || force);

        let mut snap = false;
        if target_changed {
            // 3) 起止阈值(按轴,单位格):两轴都小于阈值时直接吸附,不产生尾迹
            let dx = ((self.pending[0] - self.edge[0]) / cell_w.max(1.0)).round() as i64;
            let dy = ((self.pending[2] - self.edge[2]) / cell_h.max(1.0)).round() as i64;
            snap = !self.needs_render
                && (self.thr_x > 0 || self.thr_y > 0)
                && dx.abs() <= self.thr_x
                && dy.abs() <= self.thr_y;
            self.edge = self.pending;
            self.last_retarget = now;
            if self.burst >= 2.5 {
                // 记下"这次是密集输入":松手时要收尾,不能等 burst 回落就装作没发生过
                self.was_dense = true;
            }
            // 只有在"静止 → 运动"时才重置本段动画的计时;动画进行中被重定向
            // 不会续命,保证总时长有上限(不做无限接力的旧动画)。
            if self.motion_start.is_none() {
                self.motion_start = Some(now);
            }
        }

        // 4) 硬上限:到点直接吸附。哪怕目标一直在变,也绝不留下一段播不完的动画。
        if let Some(start) = self.motion_start {
            if self.max_ms > 0.0
                && now.saturating_duration_since(start).as_secs_f32() * 1000.0 >= self.max_ms
            {
                self.corners = corners_of(&self.edge);
                self.motion_start = None;
                self.needs_render = false;
                self.needs_frames = !rect_eq(&self.edge, &self.pending);
                return None;
            }
        }

        if snap {
            self.corners = corners_of(&self.edge);
            self.motion_start = None;
        } else if dt > 0.0 {
            self.step_corners(dt, cell_w, cell_h);
        }

        // 4.5) 连续输入时把尾巴整体压到 1 格以内。
        //      这是"跟手"的关键:不管之前积压了多少段动画,视觉上尾迹最多拖一格,
        //      松手时自然不可能落后好几行。只缩短、不改形状,所以看起来仍是一条
        //      被拉长的四边形,而不是突然跳变。
        if self.burst >= 2.0 {
            self.clamp_tail(cell_w, cell_h, TAIL_CAP_CELLS);
        }

        // 4.6) 松手即结束:连续输入停止超过 settle_after 后,剩余的一点尾巴
        //      直接收掉,不再按 decay 慢慢播完 —— 用户已经松手了,屏幕上不该
        //      还在"继续回车"。
        if self.was_dense && waited >= self.settle_after {
            self.corners = corners_of(&self.edge);
            self.motion_start = None;
            self.was_dense = false;
        }

        // 5) 不透明度:随光标可见性淡入/淡出(速度随倍率一起加快)
        let fade_secs = (self.decay_slow / self.speedup.max(1.0)).max(0.01);
        if visible {
            self.opacity = (self.opacity + dt / fade_secs).min(1.0);
        } else {
            self.opacity = (self.opacity - dt / fade_secs).max(0.0);
        }

        // 6) 是否需要继续渲染:任一角距目标超过半个像素
        let target = corners_of(&self.edge);
        let needs = target.iter().zip(self.corners.iter()).any(|(a, b)| {
            (a.x - b.x).abs() >= SETTLE_PX || (a.y - b.y).abs() >= SETTLE_PX
        });
        if !needs {
            // 收尾时把亚像素残差抹平,避免长时间"卡在 0.4px"出帧
            self.corners = target;
            self.motion_start = None;
        }
        self.needs_render = needs;

        // 还需要出帧的条件:动画未收敛,或还有未结算的目标(等 dwell / 等强制结算)。
        // 空闲时 edge == pending → 不空转。
        self.needs_frames = needs || !rect_eq(&self.edge, &self.pending);

        if self.opacity > 0.01 && needs {
            Some(TrailDraw {
                corners: self.corners,
                opacity: self.opacity,
            })
        } else {
            None
        }
    }

    /// 把四个角到目标的最大轴向距离压到 `max_cells` 格以内(等比缩短,保持形状)。
    /// 连续输入时用它给尾迹长度封顶,避免多段动画的位移在视觉上叠成一条长尾。
    fn clamp_tail(&mut self, cell_w: f32, cell_h: f32, max_cells: f32) {
        let target = corners_of(&self.edge);
        let mut longest = 0.0f32;
        for (c, t) in self.corners.iter().zip(target.iter()) {
            let cw = (c.x - t.x).abs() / cell_w.max(1.0);
            let ch = (c.y - t.y).abs() / cell_h.max(1.0);
            longest = longest.max(cw.max(ch));
        }
        if longest <= max_cells {
            return;
        }
        let k = max_cells / longest;
        for (c, t) in self.corners.iter_mut().zip(target.iter()) {
            c.x = t.x + (c.x - t.x) * k;
            c.y = t.y + (c.y - t.y) * k;
        }
    }

    /// 四个角以指数速度追赶目标,衰减速度按"角与运动方向的夹角"逐角变化:
    /// 运动前方的角 decay 更快(先归位),后方的角更慢(留下尾巴)。
    ///
    /// 这里在 kitty 的固定衰减上叠了两层倍率:
    /// - `burst`:连续重定向(快速输入)越多,追得越快;
    /// - `dist`:剩余距离越长,追得越快(否则几十行的跳跃要拖一秒多)。
    ///
    /// 倍率由 [`speedup_target`] / [`approach_speedup`] 算出来。跨窗格那条
    /// 尾迹把这个值锁在 1.0(见 `fixed_speedup`),否则跨屏几十格会一闪即收。
    fn step_corners(&mut self, dt: f32, cell_w: f32, cell_h: f32) {
        let target = corners_of(&self.edge);
        let dist_cells = max_dist_cells(&self.corners, &target, cell_w, cell_h);

        // 目标倍率:积压 + 距离,封顶。第一次移动不因积压加速(单次回车保留
        // kitty 原本的手感),连续重定向才开始"追赶";长距离则始终额外加速。
        let want = speedup_target(dist_cells, (self.burst - 1.0).max(0.0));
        self.speedup = approach_speedup(self.speedup, want, dt);

        chase_corners(
            &mut self.corners,
            self.edge,
            dt,
            self.decay_fast,
            self.decay_slow,
            self.speedup,
        );
    }
}

/// 把四个角朝目标矩形 `target = [x0, x1, y0, y1]` 推进一帧。
///
/// 这是 kitty `cursor_trail.c` 的核心,也是**唯一**的尾迹运动学:
///
/// * 每个角独立追赶自己的目标角,`step = 1 - 2^(-10*dt/decay)`;
/// * decay 逐角不同 —— 按"该角的位移方向与目标中心→该角方向"的夹角插值,
///   运动**前方**的角先归位、**后方**的角被留在原处,四边形因此被拉成一条尾巴;
/// * `speedup` 是统一的倍率(积压 / 距离自适应),把 decay 整体缩短。
///
/// 窗格内的光标尾迹和跨窗格尾迹**是同一个 [`CursorTrail`]**,都走这一个函数 ——
/// 后者只是用 [`CursorTrail::seed`] 把起点搬到旧窗格的光标上,不存在"两套动画"。
/// 跨窗格实例另外锁了速度倍率和单帧步长(见 `App::focus_trail`),否则长距离
/// 会把倍率顶满、尾巴一两帧收完;衰减/淡入/硬上限仍是同一套。
pub fn chase_corners(
    corners: &mut [Point2; 4],
    target: [f32; 4],
    dt: f32,
    decay_fast: f32,
    decay_slow: f32,
    speedup: f32,
) -> bool {
    let decay_fast = decay_fast.max(0.01);
    let decay_slow = decay_slow.max(decay_fast);
    let speedup = speedup.clamp(1.0, MAX_SPEEDUP);
    let tgt = corners_of(&target);
    let ccx = (target[0] + target[1]) * 0.5;
    let ccy = (target[2] + target[3]) * 0.5;
    let diag = ((target[1] - target[0]).powi(2) + (target[3] - target[2]).powi(2))
        .sqrt()
        .max(1.0);
    let diag_2 = diag * 0.5;

    let mut dx = [0.0f32; 4];
    let mut dy = [0.0f32; 4];
    let mut dot = [0.0f32; 4];
    for i in 0..4 {
        dx[i] = tgt[i].x - corners[i].x;
        dy[i] = tgt[i].y - corners[i].y;
        let len = (dx[i] * dx[i] + dy[i] * dy[i]).sqrt();
        if len < 1e-6 {
            dx[i] = 0.0;
            dy[i] = 0.0;
            dot[i] = 0.0;
            continue;
        }
        dot[i] = (dx[i] * (tgt[i].x - ccx) + dy[i] * (tgt[i].y - ccy)) / diag_2 / len;
    }
    let min_dot = dot.iter().cloned().fold(f32::MAX, f32::min);
    let max_dot = dot.iter().cloned().fold(f32::MIN, f32::max);

    let mut moving = false;
    for i in 0..4 {
        if dx[i] == 0.0 && dy[i] == 0.0 {
            continue;
        }
        let decay = if (max_dot - min_dot).abs() < 1e-6 {
            decay_slow
        } else {
            decay_slow + (decay_fast - decay_slow) * (dot[i] - min_dot) / (max_dot - min_dot)
        };
        // kitty: step = 1 - 2^(-10 * dt / decay);倍率把 decay 整体缩短
        let decay_eff = (decay / speedup).max(0.005);
        let step = 1.0 - (-(10.0 * dt) / decay_eff).exp2();
        corners[i].x += dx[i] * step;
        corners[i].y += dy[i] * step;
        if (dx[i] * dx[i] + dy[i] * dy[i]).sqrt() >= SETTLE_PX {
            moving = true;
        }
    }
    moving
}

/// 四个角里"还在路上"的最大距离(以字符格为单位)。
///
/// 窗格内尾迹和跨窗格飞行都用它决定要不要加速,单位一致才能保证两处的
/// 手感一致(同一个字符距离 → 同一个倍率)。
fn max_dist_cells(corners: &[Point2; 4], target: &[Point2; 4], cell_w: f32, cell_h: f32) -> f32 {
    let mut dist_cells = 0.0f32;
    for i in 0..4 {
        let dx = (target[i].x - corners[i].x) / cell_w.max(1.0);
        let dy = (target[i].y - corners[i].y) / cell_h.max(1.0);
        dist_cells = dist_cells.max(dx.abs().max(dy.abs()));
    }
    dist_cells
}

/// 速度倍率的目标值:**积压**(连续输入的回车数,飞行时为 0)+ **剩余距离**,
/// 封顶 [`MAX_SPEEDUP`]。窗格内尾迹与跨窗格飞行共用这一个公式。
fn speedup_target(dist_cells: f32, backlog: f32) -> f32 {
    (1.0 + backlog.max(0.0) + (dist_cells * 0.45).min(3.5)).min(MAX_SPEEDUP)
}

/// 让倍率平滑地逼近目标值:变大用 [`SPEEDUP_UP_TAU`]、变小用
/// [`SPEEDUP_DOWN_TAU`],这样"开始加速"和"松手收拢"的段落感一致。
fn approach_speedup(cur: f32, want: f32, dt: f32) -> f32 {
    let tau = if want > cur {
        SPEEDUP_UP_TAU
    } else {
        SPEEDUP_DOWN_TAU
    };
    cur + (want - cur) * (1.0 - (-dt / tau).exp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CursorConfig;

    fn cfg() -> CursorConfig {
        CursorConfig {
            trail_dwell_ms: 30.0,
            trail_start_x: 2,
            trail_start_y: 2,
            ..CursorConfig::default()
        }
    }

    fn cell(c: f32, r: f32) -> [f32; 4] {
        [c * 9.0, (c + 1.0) * 9.0, r * 18.0, (r + 1.0) * 18.0]
    }

    /// 等待停留时间期间必须持续要求出帧,否则回车后的静默期里尾迹永远不启动
    #[test]
    fn gate_fires_without_extra_events() {
        let mut t = CursorTrail::new(&cfg());
        let t0 = Instant::now();
        let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);
        // 一次 10 格跳跃;之后不再有任何外部事件,只靠定时帧推进
        let _ = t.update(t0 + Duration::from_millis(16), cell(10.0, 0.0), true, 9.0, 18.0);
        let mut now = t0 + Duration::from_millis(16);
        let mut started = false;
        for _ in 0..10 {
            now += Duration::from_millis(8);
            assert!(t.needs_frames(), "等待停留期应要求继续出帧");
            if t
                .update(now, cell(10.0, 0.0), true, 9.0, 18.0)
                .is_some()
            {
                started = true;
                break;
            }
        }
        assert!(started, "仅靠定时帧也应启动尾迹");
    }

    /// 空闲时不应要求出帧(否则会一直以 8ms 定时器空转)
    #[test]
    fn idle_does_not_request_frames() {
        let mut t = CursorTrail::new(&cfg());
        let t0 = Instant::now();
        let c = cell(0.0, 0.0);
        let _ = t.update(t0, c, true, 9.0, 18.0);
        let _ = t.update(t0 + Duration::from_millis(16), c, true, 9.0, 18.0);
        assert!(!t.needs_frames());
    }

    /// 单格移动低于阈值 → 直接吸附,无尾迹
    #[test]
    fn small_move_snaps_without_trail() {
        let mut t = CursorTrail::new(&cfg());
        let t0 = Instant::now();
        let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);
        let _ = t.update(t0 + Duration::from_millis(10), cell(1.0, 0.0), true, 9.0, 18.0);
        let out = t.update(t0 + Duration::from_millis(120), cell(1.0, 0.0), true, 9.0, 18.0);
        assert!(out.is_none(), "单格移动应直接吸附,不产生尾迹");
    }

    /// 超过阈值的跳跃 → 产生尾迹,并在衰减后自行结束
    #[test]
    fn jump_creates_trail_then_settles() {
        let mut t = CursorTrail::new(&cfg());
        let t0 = Instant::now();
        let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);
        let _ = t.update(t0 + Duration::from_millis(10), cell(10.0, 0.0), true, 9.0, 18.0);
        // 停留时间未到 → 尚未开始
        assert!(t
            .update(t0 + Duration::from_millis(20), cell(10.0, 0.0), true, 9.0, 18.0)
            .is_none());
        // 超过停留时间 → 开始出现尾迹
        let out = t
            .update(t0 + Duration::from_millis(60), cell(10.0, 0.0), true, 9.0, 18.0)
            .expect("应产生尾迹");
        assert!(out.opacity > 0.5);
        assert!(t.is_animating());
        // 持续推进 2 秒后应收敛结束
        let mut ended = false;
        for i in 0..200 {
            let now = t0 + Duration::from_millis(60 + i * 16);
            if t.update(now, cell(10.0, 0.0), true, 9.0, 18.0).is_none() {
                ended = true;
                break;
            }
        }
        assert!(ended, "尾迹应在衰减后结束");
        assert!(!t.is_animating());
    }

    /// 光标隐藏 → 不透明度逐渐降为 0
    #[test]
    fn hidden_cursor_fades_out() {
        let mut t = CursorTrail::new(&cfg());
        let t0 = Instant::now();
        let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);
        let _ = t.update(t0 + Duration::from_millis(10), cell(10.0, 0.0), true, 9.0, 18.0);
        let _ = t.update(t0 + Duration::from_millis(60), cell(10.0, 0.0), true, 9.0, 18.0);
        // 隐藏 1 秒
        let mut now = t0 + Duration::from_millis(60);
        for _ in 0..70 {
            now += Duration::from_millis(16);
            let _ = t.update(now, cell(10.0, 0.0), false, 9.0, 18.0);
        }
        assert!(!t.is_animating());
    }

    /// 重置后状态干净(窗口缩放等)
    #[test]
    fn reset_clears_state() {
        let mut t = CursorTrail::new(&cfg());
        let t0 = Instant::now();
        let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);
        let _ = t.update(t0 + Duration::from_millis(10), cell(10.0, 0.0), true, 9.0, 18.0);
        let _ = t.update(t0 + Duration::from_millis(60), cell(10.0, 0.0), true, 9.0, 18.0);
        t.reset();
        assert!(!t.is_animating());
        assert!(t
            .update(t0 + Duration::from_millis(70), cell(10.0, 0.0), true, 9.0, 18.0)
            .is_none());
        assert_eq!(t.speedup(), 1.0);
    }

    /// trail = 0 → 完全关闭
    #[test]
    fn zero_dwell_disables() {
        let mut c = cfg();
        c.trail_dwell_ms = 0.0;
        let mut t = CursorTrail::new(&c);
        let t0 = Instant::now();
        let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);
        let out = t.update(
            t0 + Duration::from_millis(16),
            cell(100.0, 0.0),
            true,
            9.0,
            18.0,
        );
        assert!(out.is_none());
    }

    /// 按住回车(每 33ms 下移一行、持续 1.2 秒):
    /// 稳定后尾迹必须始终贴在 1 行以内,松手后 40ms 内彻底收掉。
    #[test]
    fn rapid_input_keeps_tail_under_one_line() {
        let mut t = CursorTrail::new(&cfg());
        let t0 = Instant::now();
        let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);

        let mut now = t0;
        let mut row = 0.0f32;
        for i in 0..150 {
            // 每 32ms 一次光标下移(键盘自动重复的典型节奏),每 8ms 出一帧
            if i % 4 == 0 {
                row += 1.0;
            }
            now += Duration::from_millis(8);
            let _ = t.update(now, cell(0.0, row), true, 9.0, 18.0);
            // 跳过前 200ms 的起步阶段(dwell + 倍率爬升)
            if i > 25 {
                let lag = (row * 18.0 - t.corners[0].y).abs() / 18.0;
                assert!(
                    lag <= 1.05,
                    "按住回车时第 {i} 帧落后 {lag:.2} 行,超过 1 行"
                );
            }
        }
        let lag = (row * 18.0 - t.corners[0].y).abs() / 18.0;
        assert!(lag <= 1.05, "松手瞬间落后 {lag:.2} 行");
        assert!(t.speedup() > 2.0, "连续输入应触发加速,当前倍率 {}", t.speedup());

        // 松手:40ms 宽限期 + 一帧余量后必须完全收掉,不能还在"回车"
        for _ in 0..6 {
            now += Duration::from_millis(8);
            let _ = t.update(now, cell(0.0, row), true, 9.0, 18.0);
        }
        let lag = (row * 18.0 - t.corners[0].y).abs() / 18.0;
        assert!(lag < 0.05, "松手后仍落后 {lag:.2} 行");
        assert!(!t.needs_frames(), "松手后不应继续出帧");
    }

    /// 同一帧里灌进多次移动(PTY 一次输出多行)时,只保留最新目标:
    /// 中间位置从不入队,不会出现"一个动画接一个动画播"。
    #[test]
    fn moves_in_one_frame_coalesce_to_latest() {
        let mut t = CursorTrail::new(&cfg());
        let t0 = Instant::now();
        let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);

        let now = t0 + Duration::from_millis(10);
        for row in 1..=5 {
            let _ = t.update(now, cell(0.0, row as f32), true, 9.0, 18.0);
        }
        // 只有"最新位置"被记住;中间位置直接被覆盖,不会入队
        assert_eq!(t.pending[2], cell(0.0, 5.0)[2], "pending 应只保留最新位置");

        // 过了 dwell 后一次性结算到第 5 行(而不是从 1 行开始排队播)
        let mut now = now;
        for _ in 0..20 {
            now += Duration::from_millis(8);
            let _ = t.update(now, cell(0.0, 5.0), true, 9.0, 18.0);
        }
        assert_eq!(t.edge[2], cell(0.0, 5.0)[2], "应直接结算到最新位置");
        assert!(
            (t.corners[0].y - cell(0.0, 5.0)[2]).abs() < 1.0,
            "应直接追到最新位置,而不是停在中间某一行"
        );
    }

    /// 松手后再按一次回车必须重新有尾迹,不能被上一段密集输入的收尾逻辑吞掉
    #[test]
    fn single_input_after_burst_still_animates() {
        let mut t = CursorTrail::new(&cfg());
        let t0 = Instant::now();
        let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);

        // 按住回车 0.5s → 进入密集模式
        let mut now = t0;
        let mut row = 0.0f32;
        for i in 0..60 {
            if i % 4 == 0 {
                row += 1.0;
            }
            now += Duration::from_millis(8);
            let _ = t.update(now, cell(0.0, row), true, 9.0, 18.0);
        }
        // 松手 100ms(收尾 + 新一段输入的间隔都已满足)
        for _ in 0..13 {
            now += Duration::from_millis(8);
            let _ = t.update(now, cell(0.0, row), true, 9.0, 18.0);
        }

        // 单次大跳(30 行),dwell 之后应重新出现尾迹
        let target = row + 30.0;
        let _ = t.update(now, cell(0.0, target), true, 9.0, 18.0);
        now += Duration::from_millis(40);
        let out = t.update(now, cell(0.0, target), true, 9.0, 18.0);
        assert!(out.is_some(), "松手后的单次回车应重新出现尾迹");
    }

    /// 松开按键的那一帧就要结束密集输入的尾迹(不能等 40ms 定时),
    /// 但单次按键的尾迹不能被"松手"吞掉。
    #[test]
    fn key_release_ends_dense_animation_immediately() {
        let mut t = CursorTrail::new(&cfg());
        let t0 = Instant::now();
        let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);

        let mut now = t0;
        let mut row = 0.0f32;
        for i in 0..60 {
            if i % 4 == 0 {
                row += 1.0;
            }
            now += Duration::from_millis(8);
            let _ = t.update(now, cell(0.0, row), true, 9.0, 18.0);
        }
        // 按住期间有尾迹,松手立刻归位
        assert!(t.key_released());
        assert_eq!(t.corners[0].y, cell(0.0, row)[2], "松手应直接吸附到最新位置");
        assert!(!t.needs_frames());
        assert!(!t.is_animating());

        // 单次按键:松手不应触发收尾
        let mut s = CursorTrail::new(&cfg());
        let _ = s.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);
        let _ = s.update(t0 + Duration::from_millis(10), cell(0.0, 10.0), true, 9.0, 18.0);
        let _ = s.update(t0 + Duration::from_millis(60), cell(0.0, 10.0), true, 9.0, 18.0);
        assert!(!s.key_released(), "单次按键的尾迹不应被松手吞掉");
    }

    /// 硬上限:长距离跳跃不会拖着尾巴超过 trail_max(到点直接吸附)
    #[test]
    fn hard_deadline_caps_animation() {
        let mut c = cfg();
        c.trail_max_ms = 100.0;
        let mut t = CursorTrail::new(&c);
        let t0 = Instant::now();
        let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);
        let _ = t.update(t0 + Duration::from_millis(10), cell(0.0, 80.0), true, 9.0, 18.0);
        // 触发 retarget
        let mut now = t0 + Duration::from_millis(50);
        let _ = t.update(now, cell(0.0, 80.0), true, 9.0, 18.0);
        // 100ms 上限 + 一帧余量之内必须结束
        for _ in 0..20 {
            now += Duration::from_millis(8);
            let _ = t.update(now, cell(0.0, 80.0), true, 9.0, 18.0);
        }
        assert!(!t.needs_frames(), "超过 trail_max 后不应继续出帧");
        assert!(!t.is_animating(), "超过 trail_max 后应已吸附");
    }



    /// 输入密度相同时,长距离应比短距离追得更快(距离自适应)
    #[test]
    fn long_jump_is_faster_than_short_jump() {
        let run = |rows: f32| {
            let mut t = CursorTrail::new(&cfg());
            let t0 = Instant::now();
            let _ = t.update(t0, cell(0.0, 0.0), true, 9.0, 18.0);
            let mut now = t0 + Duration::from_millis(10);
            let _ = t.update(now, cell(0.0, rows), true, 9.0, 18.0);
            let mut frames = 0;
            loop {
                now += Duration::from_millis(8);
                frames += 1;
                if t.update(now, cell(0.0, rows), true, 9.0, 18.0).is_none() {
                    break;
                }
                if frames > 500 {
                    break;
                }
            }
            frames
        };
        let short = run(6.0);
        let long = run(40.0);
        // 长距离多花一点帧是正常的,但不应该按距离线性放大(那样就是排队感)
        assert!(
            long < short * 3,
            "长距离 {long} 帧 vs 短距离 {short} 帧,加速没有生效"
        );
    }
}
