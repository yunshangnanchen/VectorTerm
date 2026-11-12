//! 深色主题:与 vectorterm 默认配色对齐,纯黑太硬,整体压暗一档。

use eframe::egui::{self, Color32, FontFamily, FontId, TextStyle};

pub const BG: Color32 = Color32::from_rgb(0x15, 0x17, 0x1c);
pub const PANEL: Color32 = Color32::from_rgb(0x1a, 0x1d, 0x23);
pub const PANEL2: Color32 = Color32::from_rgb(0x21, 0x25, 0x30);
pub const PANEL3: Color32 = Color32::from_rgb(0x27, 0x2c, 0x37);
pub const BORDER: Color32 = Color32::from_rgb(0x2b, 0x30, 0x3a);
pub const TEXT: Color32 = Color32::from_rgb(0xd5, 0xd9, 0xe0);
pub const MUTED: Color32 = Color32::from_rgb(0x8a, 0x92, 0xa1);
pub const DIM: Color32 = Color32::from_rgb(0x66, 0x6e, 0x7c);
pub const DESC: Color32 = Color32::from_rgb(0x79, 0x82, 0x8f);
pub const ACCENT: Color32 = Color32::from_rgb(0x7a, 0xa2, 0xf7);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(0x2c, 0x3a, 0x5c);
pub const OK: Color32 = Color32::from_rgb(0x9e, 0xce, 0x6a);
pub const WARN: Color32 = Color32::from_rgb(0xe0, 0xaf, 0x68);
pub const ERR: Color32 = Color32::from_rgb(0xf7, 0x76, 0x8e);
pub const SEL: Color32 = Color32::from_rgb(0x2f, 0x3a, 0x52);

/// 应用全局字体与深色视觉。
pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = dark_visuals();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);

    // 加载系统中文字体(微软雅黑),否则中文会显示成方块。找不到就退回默认。
    let mut fonts = egui::FontDefinitions::default();
    for path in [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msyh.ttf",
        "C:/Windows/Fonts/simhei.ttf",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            let name = format!("cjk{}", fonts.font_data.len());
            fonts.font_data.insert(
                name.clone(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            // 加入 proportional(正文)家族最前
            if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
                list.insert(0, name.clone());
            }
            // 加入 monospace(代码预览)家族最后:等宽字体没有中文字形时回退到雅黑
            if let Some(list) = fonts.families.get_mut(&FontFamily::Monospace) {
                list.push(name);
            }
            break;
        }
    }
    ctx.set_fonts(fonts);

    let mut text_styles: std::collections::BTreeMap<TextStyle, FontId> = Default::default();
    text_styles.insert(
        TextStyle::Heading,
        FontId::new(17.0, FontFamily::Proportional),
    );
    text_styles.insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
    text_styles.insert(
        TextStyle::Button,
        FontId::new(13.0, FontFamily::Proportional),
    );
    text_styles.insert(
        TextStyle::Small,
        FontId::new(11.0, FontFamily::Proportional),
    );
    text_styles.insert(
        TextStyle::Monospace,
        FontId::new(13.0, FontFamily::Monospace),
    );
    style.text_styles = text_styles;

    ctx.set_style(style);
}

fn dark_visuals() -> egui::Visuals {
    egui::Visuals {
        dark_mode: true,
        override_text_color: Some(TEXT),
        panel_fill: PANEL,
        window_fill: PANEL,
        extreme_bg_color: BG,
        faint_bg_color: PANEL,
        window_stroke: egui::Stroke::new(1.0, BORDER),
        widgets: egui::style::Widgets {
            noninteractive: widget(PANEL, PANEL, BORDER, TEXT),
            inactive: widget(PANEL2, PANEL2, BORDER, TEXT),
            hovered: widget(PANEL3, PANEL3, ACCENT, TEXT),
            active: widget(ACCENT_SOFT, ACCENT_SOFT, ACCENT, TEXT),
            open: widget(PANEL3, PANEL3, ACCENT, TEXT),
        },
        selection: egui::style::Selection {
            bg_fill: SEL,
            stroke: egui::Stroke::new(1.0, ACCENT),
        },
        hyperlink_color: ACCENT,
        ..egui::Visuals::dark()
    }
}

fn widget(bg: Color32, weak: Color32, stroke: Color32, fg: Color32) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: weak,
        bg_stroke: egui::Stroke::new(1.0, stroke),
        fg_stroke: egui::Stroke::new(1.0, fg),
        corner_radius: 4.0.into(),
        expansion: 0.0,
    }
}
