//! VectorTerm 配置管理器。
//!
//! 一个与 `vectorterm` 终端本体**完全独立**的图形化配置工具。它读取、编辑并写回
//! VectorTerm 的 Lua 配置(默认 `config/config.lua`),不依赖终端本体任何代码,甚至
//! 可以在没有 vectorterm 的机器上单独分发运行。

// 发行版是 GUI 子系统:双击不弹黑窗口(与 vectorterm 同样的处理)。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod lua;
mod schema;
mod store;
mod theme;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 820.0])
            .with_min_inner_size([960.0, 600.0])
            .with_title("VectorTerm 配置管理器"),
        ..Default::default()
    };
    eframe::run_native(
        "vectorterm-settings",
        options,
        Box::new(|cc| Ok(Box::new(app::SettingsApp::new(cc)))),
    )
}
