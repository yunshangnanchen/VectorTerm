//! 可视化滚动条:几何换算与命中测试。
//!
//! 这里刻意只做**纯逻辑**(矩形进、矩形出),不碰 winit / wezterm / 显示列表,
//! 于是「贴底时滑块在最下面」「回看越多滑块越往上」「最小滑块高度」「拖动
//! 跟着鼠标走」这些都能直接单测,不用起窗口。
//!
//! 口径和 `render::snapshot` 一致:`scroll = 0` 表示贴底(看最新的一行),
//! `scroll` 越大表示往回看得越久;`max = scrollback_rows - physical_rows`。

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geom {
    pub track: Rect,
    pub thumb: Rect,
    /// 可回看的最大行数(浮点,和 ScrollAnim 的 max 同源)
    pub max_scroll: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// 按在滑块上;`grab_dy` = 鼠标 y - 滑块上沿,拖动时保持这个偏移
    Thumb { grab_dy: f64 },
    /// 按在轨道上;`above = true` 表示点在滑块上方 → 往上翻一页
    Track { above: bool },
}

/// 由 pane 矩形算出轨道与滑块。
///
/// - `pane` 是**整窗坐标**下的 pane 矩形(调用方已加过 padding / 标签栏高度);
/// - `width` 是滚动条宽度(px),轨道贴在 pane 右边缘内侧;
/// - `rows` 视口行数、`sb` 历史总行数;
/// - `min_thumb` 是滑块最小高度(px),历史很长时也不会细到抓不住。
pub fn geometry(
    pane: Rect,
    width: f64,
    rows: usize,
    sb: usize,
    scroll: f64,
    min_thumb: f64,
) -> Geom {
    let track = Rect {
        x: pane.x + pane.w - width.max(1.0),
        y: pane.y,
        w: width.max(1.0),
        h: pane.h.max(0.0),
    };
    let max_scroll = sb.saturating_sub(rows) as f64;
    // 滑块长度 = 视口占整个历史的比例;至少 min_thumb,但不超过轨道
    let frac = if sb > 0 {
        (rows as f64 / sb as f64).min(1.0)
    } else {
        1.0
    };
    let thumb_h = (track.h * frac)
        .max(min_thumb.max(1.0).min(track.h))
        .min(track.h.max(1.0));
    let travel = (track.h - thumb_h).max(0.0);
    // scroll=0 贴底 → 滑块落在轨道最下方;scroll=max → 最上方
    let t = if max_scroll > 0.0 {
        (scroll / max_scroll).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let thumb = Rect {
        x: track.x,
        y: track.y + travel * (1.0 - t),
        w: track.w,
        h: thumb_h,
    };
    Geom {
        track,
        thumb,
        max_scroll,
    }
}

/// 拖动滑块:鼠标 y 落在哪里,就把视口摆到对应的历史位置。
/// `grab_dy` 是按下时「鼠标 y - 滑块上沿」,保证拖动过程中不跳一下。
pub fn scroll_at(g: &Geom, mouse_y: f64, grab_dy: f64) -> f64 {
    let travel = (g.track.h - g.thumb.h).max(0.0);
    if travel <= 0.0 || g.max_scroll <= 0.0 {
        return 0.0;
    }
    let top = (mouse_y - grab_dy - g.track.y).clamp(0.0, travel);
    (1.0 - top / travel) * g.max_scroll
}

/// 鼠标命中:只认轨道矩形内。滑块优先。
pub fn hit(g: &Geom, x: f64, y: f64) -> Option<Hit> {
    if !g.track.contains(x, y) {
        return None;
    }
    if g.thumb.contains(x, y) {
        Some(Hit::Thumb {
            grab_dy: y - g.thumb.y,
        })
    } else {
        Some(Hit::Track {
            above: y < g.thumb.y,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Rect {
        Rect {
            x: 10.0,
            y: 20.0,
            w: 800.0,
            h: 400.0,
        }
    }

    #[test]
    fn track_hugs_right_edge() {
        let g = geometry(pane(), 8.0, 40, 4000, 0.0, 24.0);
        assert_eq!(g.track.x, 802.0);
        assert_eq!(g.track.y, 20.0);
        assert_eq!(g.track.w, 8.0);
        assert_eq!(g.track.h, 400.0);
        assert_eq!(g.max_scroll, 3960.0);
    }

    #[test]
    fn bottom_scroll_puts_thumb_at_bottom() {
        let g = geometry(pane(), 8.0, 40, 4000, 0.0, 24.0);
        assert!((g.thumb.y + g.thumb.h - (g.track.y + g.track.h)).abs() < 1e-6);
    }

    #[test]
    fn top_scroll_puts_thumb_at_top() {
        let g = geometry(pane(), 8.0, 40, 4000, 3960.0, 24.0);
        assert!((g.thumb.y - g.track.y).abs() < 1e-6);
    }

    #[test]
    fn thumb_keeps_minimum_height_for_long_history() {
        // 40/4000 = 1% → 4px,太细;被 min_thumb 顶到 24px
        let g = geometry(pane(), 8.0, 40, 4000, 0.0, 24.0);
        assert_eq!(g.thumb.h, 24.0);
        // 历史很短时滑块几乎占满轨道
        let g2 = geometry(pane(), 8.0, 40, 45, 0.0, 24.0);
        assert!(g2.thumb.h > 350.0);
    }

    #[test]
    fn drag_maps_back_to_scroll() {
        let g = geometry(pane(), 8.0, 40, 4000, 0.0, 24.0);
        let mid = g.track.y + (g.track.h - g.thumb.h) * 0.5;
        let pos = scroll_at(&g, mid + 5.0, 5.0);
        assert!((pos - g.max_scroll * 0.5).abs() < 1e-6, "中间 = 一半历史: {pos}");
        assert!((scroll_at(&g, g.track.y + 5.0, 5.0) - g.max_scroll).abs() < 1e-6);
        assert_eq!(scroll_at(&g, g.track.y + g.track.h, 0.0), 0.0);
    }

    #[test]
    fn hit_distinguishes_thumb_and_track() {
        let g = geometry(pane(), 8.0, 40, 4000, 1000.0, 24.0);
        let cx = g.track.x + 4.0;
        assert!(matches!(
            hit(&g, cx, g.thumb.y + 2.0),
            Some(Hit::Thumb { .. })
        ));
        assert_eq!(hit(&g, cx, g.track.y + 2.0), Some(Hit::Track { above: true }));
        assert_eq!(
            hit(&g, cx, g.track.y + g.track.h - 2.0),
            Some(Hit::Track { above: false })
        );
        assert_eq!(hit(&g, 0.0, 0.0), None);
    }

    #[test]
    fn no_history_has_one_full_thumb_and_no_scroll() {
        let g = geometry(pane(), 8.0, 40, 40, 0.0, 24.0);
        assert_eq!(g.max_scroll, 0.0);
        assert_eq!(g.thumb.h, g.track.h);
        assert_eq!(scroll_at(&g, g.track.y, 0.0), 0.0);
    }
}
