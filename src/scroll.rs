//! 平滑回看滚动:缓入缓出的亚行动画 + "输入优先"的回底策略。
//!
//! 传统实现的回看是整数行瞬移:滚轮一转,画面直接跳 3 行。现代终端(Windows
//! Terminal 的 smooth scroll、kitty 的 `scrollback_lines` 动画)把它当成一段
//! 短动画:目标是"第几行",每帧插值出**亚行位置**,渲染时整体上移不到一行的
//! 像素量,于是画面是连续滑动而不是逐行跳。
//!
//! 这里的几条规则:
//!
//! 1. **缓入缓出**:从静止开始的一段滚动用 cubic ease-in-out(与 CSS
//!    `ease-in-out` 同一条曲线),起步和收尾都慢,中段快;
//! 2. **连续滚动不重启**:滚轮快速连发时,如果上一段还在跑,新的一段用
//!    ease-out(先快后慢)接上 —— 此时已经有速度,再从 0 加速会明显"一顿一顿";
//!    目标累加,时长按剩余距离的平方根增长,并受 `max` 硬上限约束,
//!    所以怎么连发都不会攒出一段要播很久的动画;
//! 3. **输入优先、提前回底**:按键/粘贴时视口必须回到底部。回底用 ease-out,
//!    并在 `back`(长按连续输入时 `back_burst`)毫秒内走完 —— 远早于长按回车
//!    结束,于是"回底"和"光标尾迹"不互相拖:滚动先到位,尾迹再自己播完。
//!    已经在回底的途中不会因为下一次按键重新起跑(否则每 30ms 重启一次,
//!    永远走不完)。

use crate::config::ScrollConfig;
use std::time::Instant;

/// 缓动曲线
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Curve {
    /// cubic ease-in-out:起步慢、中段快、收尾慢
    InOut,
    /// cubic ease-out:起步最快、收尾慢(用于接着已有速度的连发滚动)
    Out,
}

impl Curve {
    fn ease(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::InOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    let f = -2.0 * t + 2.0;
                    1.0 - f * f * f / 2.0
                }
            }
            Curve::Out => {
                let f = 1.0 - t;
                1.0 - f * f * f
            }
        }
    }
}

pub struct ScrollAnim {
    enabled: bool,
    /// 一个滚轮档位的基准时长(毫秒)
    base_ms: f32,
    /// 单段动画的硬上限(毫秒)
    max_ms: f32,
    /// 按键后回到底部的时长(毫秒)
    back_ms: f32,
    /// 长按连续输入时回到底部的时长(毫秒)
    back_burst_ms: f32,

    /// 当前亚行位置(0 = 贴底,越大越往回看)
    pos: f32,
    /// 动画终点
    target: f32,
    /// 本段动画的起点
    from: f32,
    start: Instant,
    /// 本段动画时长(秒)
    dur: f32,
    curve: Curve,
    animating: bool,
    /// 回看行数上限(由调用方每帧同步)
    max: usize,
}

impl ScrollAnim {
    pub fn new(cfg: &ScrollConfig) -> Self {
        Self {
            enabled: cfg.anim_ms > 0.0,
            base_ms: cfg.anim_ms.max(0.0),
            max_ms: cfg.max_ms.max(1.0),
            back_ms: cfg.back_ms.max(0.0),
            back_burst_ms: cfg.back_burst_ms.max(0.0),
            pos: 0.0,
            target: 0.0,
            from: 0.0,
            start: Instant::now(),
            dur: 0.0,
            curve: Curve::InOut,
            animating: false,
            max: 0,
        }
    }

    /// 当前亚行位置(0 = 贴底)
    pub fn pos(&self) -> f32 {
        self.pos
    }

    /// 是否还在动画中(供事件循环决定要不要继续出帧)
    pub fn needs_frames(&self) -> bool {
        self.animating
    }

    /// 是否回看中(不是贴底);用于隐藏光标本体与尾迹
    pub fn is_scrolled(&self) -> bool {
        self.pos > 0.001
    }

    /// 回看行数上限变化(内容增长/窗口缩放时调用)
    pub fn set_max(&mut self, max: usize) {
        self.max = max;
        let cap = max as f32;
        if self.pos > cap {
            self.pos = cap;
        }
        if self.target > cap {
            self.target = cap;
        }
    }

    /// 窗口尺寸变化:亚行位置失去意义,直接贴底
    pub fn reset(&mut self) {
        self.pos = 0.0;
        self.target = 0.0;
        self.animating = false;
    }

    /// 直接落到某个位置(不做动画)。窗口尺寸变化时用来保持"正在看的那一行"
    /// 不动 —— 补白量一变,同样的回看偏移会指向别的行,得把差值补回去。
    pub fn jump_to(&mut self, pos: f32, max: usize) {
        self.max = max;
        self.pos = pos.clamp(0.0, max as f32);
        self.target = self.pos;
        self.animating = false;
    }

    /// 热重载:换一套参数,但保留当前视口位置(别让改配置把人弹回底部)
    pub fn reconfigure(&mut self, cfg: &ScrollConfig) {
        let (pos, target, max) = (self.pos, self.target, self.max);
        *self = Self::new(cfg);
        self.max = max;
        self.pos = pos.min(max as f32);
        self.target = target.min(max as f32);
    }

    /// 推进到 `now`;返回位置是否真的变了(供调用方决定是否重绘)。
    pub fn tick(&mut self, now: Instant) -> bool {
        if !self.animating {
            return false;
        }
        let before = self.pos;
        let t = now.saturating_duration_since(self.start).as_secs_f32() / self.dur.max(1e-4);
        if t >= 1.0 {
            self.pos = self.target;
            self.animating = false;
        } else {
            self.pos = self.from + (self.target - self.from) * self.curve.ease(t);
        }
        (self.pos - before).abs() > 1e-4 || !self.animating
    }

    /// 滚轮 / Shift+PageUp 的滚动:正值向上(更早的行)。
    ///
    /// 目标累加(连续滚轮不会丢档),时长按剩余距离增长并封顶;上一段还在跑
    /// 时用 ease-out 接上,避免每档都从静止重新加速。
    pub fn scroll_by(&mut self, lines: i64, now: Instant, max: usize) {
        self.max = max;
        let target = (self.target + lines as f32).clamp(0.0, max as f32);
        if self.animating && (target - self.target).abs() < 0.01 {
            return; // 已经朝同一个终点在跑
        }
        self.target = target;
        self.start_new(now, Curve::InOut);
    }

    /// 输入(按键/粘贴)触发:提前回到底部。
    ///
    /// `urgent` 表示"正在长按连续输入"(回车连发):此时用更短的时长,让视口
    /// 赶在下一批行输出之前就位,剩下的位移交给光标尾迹自己播完。
    /// 已经在回底的途中且不比这次慢的话不会重新起跑 —— 否则每次按键都会把
    /// 动画重置到 0 速度,长按时视口就永远回不来。
    pub fn back_to_bottom(&mut self, now: Instant, urgent: bool) {
        if self.pos <= 0.001 && self.target <= 0.001 && !self.animating {
            return;
        }
        self.target = 0.0;
        if !self.enabled {
            self.pos = 0.0;
            self.animating = false;
            return;
        }
        let want = if urgent { self.back_burst_ms } else { self.back_ms };
        if self.animating {
            let left = (self.dur - now.saturating_duration_since(self.start).as_secs_f32()).max(0.0);
            if left * 1000.0 <= want {
                return; // 现有动画已经够快,别打断
            }
        }
        self.start_new(now, Curve::Out);
        self.dur = (want / 1000.0).max(0.001);
    }

    /// 以当前位置为起点开始一段动画
    fn start_new(&mut self, now: Instant, curve: Curve) {
        if !self.enabled {
            self.pos = self.target;
            self.animating = false;
            return;
        }
        let dist = (self.target - self.pos).abs();
        if dist < 0.02 {
            self.pos = self.target;
            self.animating = false;
            return;
        }
        // 上一段还在跑 → 已经有速度,用 ease-out 接上;否则从静止缓入缓出
        let continuing = self.animating;
        self.curve = match curve {
            Curve::InOut if continuing => Curve::Out,
            other => other,
        };
        self.from = self.pos;
        self.start = now;
        // 时长按距离 √ 增长:一档(3 行)≈ base,十档 ≈ 1.8×base,封顶 max_ms
        let ms = (self.base_ms * (dist / 3.0).sqrt().clamp(0.55, 2.5)).min(self.max_ms);
        self.dur = (ms / 1000.0).max(0.001);
        self.animating = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn cfg() -> ScrollConfig {
        ScrollConfig {
            anim_ms: 90.0,
            max_ms: 200.0,
            back_ms: 90.0,
            back_burst_ms: 40.0,
        }
    }

    fn run_to_end(s: &mut ScrollAnim, mut now: Instant) -> u32 {
        let mut frames = 0;
        while s.needs_frames() && frames < 500 {
            now += Duration::from_millis(4);
            s.tick(now);
            frames += 1;
        }
        frames
    }

    /// 从静止开始:位置单调推进,并准确停在目标上
    #[test]
    fn wheel_scroll_eases_to_target() {
        let mut s = ScrollAnim::new(&cfg());
        let t0 = Instant::now();
        s.scroll_by(3, t0, 1000);
        assert_eq!(s.pos(), 0.0, "刚触发时还没动(缓入)");

        let mut now = t0;
        let mut prev = 0.0f32;
        let mut mid_speed = 0.0f32;
        let mut first_step = 0.0f32;
        let mut last_step = 0.0f32;
        let mut n = 0;
        while s.needs_frames() && n < 200 {
            now += Duration::from_millis(8);
            s.tick(now);
            let step = s.pos() - prev;
            assert!(step >= -1e-4, "回看方向应单调向上,实际 {:?}", step);
            if n == 0 {
                first_step = step;
            }
            if n == 6 {
                mid_speed = step;
            }
            last_step = step;
            prev = s.pos();
            n += 1;
        }
        assert!((s.pos() - 3.0).abs() < 0.01, "应停在 3 行,实际 {}", s.pos());
        assert!(!s.needs_frames());
        assert!(
            mid_speed > first_step * 1.5,
            "中段应比起步快(缓入),首帧 {first_step:.3} 中段 {mid_speed:.3}"
        );
        assert!(
            mid_speed > last_step,
            "收尾应比中段慢(缓出),中段 {mid_speed:.3} 末帧 {last_step:.3}"
        );
    }

    /// 连续滚轮:目标累加,不会因为每档重启而永远追不上
    #[test]
    fn rapid_wheel_accumulates_target() {
        let mut s = ScrollAnim::new(&cfg());
        let mut now = Instant::now();
        for _ in 0..10 {
            s.scroll_by(3, now, 1000);
            now += Duration::from_millis(30);
            s.tick(now);
        }
        // 10 档 × 3 行 = 30 行,目标必须全部记上
        let frames = run_to_end(&mut s, now);
        assert!(
            (s.pos() - 30.0).abs() < 0.01,
            "连续 10 档应累积到 30 行,实际 {}",
            s.pos()
        );
        // 总时长受 max_ms 约束:每段最多 200ms,收尾总共不该拖太久
        assert!(frames * 4 <= 260, "连续滚动收尾用了 {} 帧,超过上限", frames);
    }

    /// 滚到顶就停,不会越界
    #[test]
    fn clamps_to_max() {
        let mut s = ScrollAnim::new(&cfg());
        let t0 = Instant::now();
        s.scroll_by(100, t0, 12);
        run_to_end(&mut s, t0);
        assert!((s.pos() - 12.0).abs() < 0.01, "应钳制在 12 行,实际 {}", s.pos());
    }

    /// 输入回底要"提前"完成:长按场景 40ms 内到 0,并且之后的按键不再重启
    #[test]
    fn back_to_bottom_is_fast_and_not_restarted() {
        let mut s = ScrollAnim::new(&cfg());
        let mut now = Instant::now();
        s.scroll_by(120, now, 5000);
        now += Duration::from_millis(100);
        s.tick(now);

        // 长按回车:每 30ms 一次,持续 300ms
        let mut arrived = None;
        for i in 0..11 {
            s.back_to_bottom(now, true);
            if s.pos() <= 0.001 && arrived.is_none() {
                arrived = Some(i);
            }
            now += Duration::from_millis(30);
            s.tick(now);
            if s.pos() <= 0.001 && arrived.is_none() {
                arrived = Some(i);
            }
        }
        assert!(
            (s.pos()).abs() < 0.01,
            "长按期间必须已经回到底部,实际 {}",
            s.pos()
        );
        assert!(arrived.is_some(), "应在长按期间就回到底部");
        assert!(!s.needs_frames(), "到位后不应继续占帧");
    }

    /// 回底途中不会因为一次更慢的请求而重置(否则永远走不完)
    #[test]
    fn back_to_bottom_does_not_extend_animation() {
        let mut s = ScrollAnim::new(&cfg());
        let mut now = Instant::now();
        s.scroll_by(50, now, 1000);
        run_to_end(&mut s, now);
        assert!((s.pos() - 50.0).abs() < 0.01);

        // 从 50 行处回底
        now += Duration::from_millis(50);
        s.back_to_bottom(now, false);
        now += Duration::from_millis(20);
        s.tick(now);
        let after_first = s.pos();
        // 又来一次普通回底请求:不应该把动画重置回 50 行
        s.back_to_bottom(now, false);
        s.tick(now);
        assert!(
            s.pos() <= after_first + 1e-3,
            "回底途中被重置回升,{after_first} -> {}",
            s.pos()
        );
    }

    /// 贴底时滚轮向下:没有任何动画
    #[test]
    fn scroll_at_bottom_is_noop() {
        let mut s = ScrollAnim::new(&cfg());
        let t0 = Instant::now();
        s.scroll_by(-5, t0, 1000);
        assert!(!s.needs_frames());
        assert_eq!(s.pos(), 0.0);
        s.back_to_bottom(t0, true);
        assert!(!s.needs_frames());
    }

    /// anim = 0 → 关闭平滑滚动,直接跳到目标
    #[test]
    fn zero_duration_disables_animation() {
        let mut c = cfg();
        c.anim_ms = 0.0;
        let mut s = ScrollAnim::new(&c);
        let t0 = Instant::now();
        s.scroll_by(7, t0, 100);
        assert_eq!(s.pos(), 7.0);
        assert!(!s.needs_frames());
        s.back_to_bottom(t0, false);
        assert_eq!(s.pos(), 0.0);
    }
}
