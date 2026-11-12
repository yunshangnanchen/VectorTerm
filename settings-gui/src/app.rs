//! 应用主窗口:三栏布局。
//!
//! 左栏 = 配置文件列表(可添加/新建/删除,支持程序目录与用户目录多个 lua);
//! 中栏 = 全部配置项分组控件 + 搜索过滤;
//! 右栏 = Lua 实时预览,可手动编辑并同步回控件。
//!
//! 颜色选项走专门的拾色器(色板网格 + HEX 输入 + 预设色),这是相对旧版的体验重点。

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText};

use crate::lua::{self, Value};
use crate::schema::{self, Field, Group, Kind, Reload};
use crate::store::{self, ConfigFile, FieldStatus, FileItem};
use crate::theme;

/// 搜索框的稳定 Id,Ctrl+F 靠它把焦点送过去。
fn search_id() -> egui::Id {
    egui::Id::new("vectorterm_settings_search_box")
}

/// 界面里显示的字段状态(额外区分"已修改待保存")。
#[derive(Clone, Copy, PartialEq)]
enum UiState {
    Default,
    Set,
    Expr,
    Modified,
}

pub struct SettingsApp {
    // 配置文件
    files: Vec<FileItem>,
    current: Option<ConfigFile>,
    current_idx: Option<usize>,
    // 搜索
    search: String,
    // 右侧预览
    preview: String,
    preview_editing: bool,
    // 颜色拾取器
    color_picker: Option<ColorPickerState>,
    // 状态提示
    toast: Option<(String, f64)>,
    // 项目目录(用于发现配置文件)
    project_dir: PathBuf,
    // 新文件对话框
    new_file_name: String,
    show_new_file: bool,
    // 当前生效配置:~/.config/VectorTerm/current/ 里唯一那份 lua(None = 用程序默认)
    active_path: Option<PathBuf>,
    // current/ 里有多份 lua 时的错误说明(终端会退回程序默认配置)
    active_error: Option<String>,
    // 键位文件清单(~/.config/VectorTerm/keys/ 里的 + 命名像 keys*.lua 的)
    keymaps: Vec<PathBuf>,
    // 当前主配置里 keys_file 引用的真实文件路径(空 = 未引用任何键位文件)
    current_keymap: String,
    // 当前被引用的键位文件(在内存里编辑;None = 主配置未引用,或文件还不存在)
    keymap: Option<ConfigFile>,
    // 新建键位文件对话框
    new_keymap_name: String,
    show_new_keymap: bool,
    // 键位面板:新增一条键位的临时输入
    new_key_combo: String,
    new_key_action: String,
}

struct ColorPickerState {
    path: Vec<String>,
    label: String,
    sub_key: Option<String>, // pair 里的子键
    current: Color32,
    hex: String,
    alpha: bool,
    /// 是否是"none/Auto"这类特殊值
    special: Option<String>,
}

/// 分割线实时预览用的「当前生效值」。字段与 config::SplitDividerConfig 一一对应,
/// 但这里只用颜色与几何,不引入终端本体的依赖(配置管理器是独立 crate)。
struct DividerPreviewSpec {
    enabled: bool,
    thickness: Option<f32>,
    /// 两侧文字之间的缝宽。None = 自动(按字号)
    gap: Option<f32>,
    /// 文字到线边缘的距离。设置后覆盖 gap
    text_gap: Option<f32>,
    inset_start: f32,
    inset_end: f32,
    solid: Color32,
    stops: Vec<Color32>,
    orientation: String,
    opacity: f32,
    /// 只有聚焦 pane 显示输入光标
    hide_inactive_caret: bool,
    /// 输入光标(飞行光标)颜色
    caret_color: Color32,
}

impl Default for DividerPreviewSpec {
    fn default() -> Self {
        Self {
            enabled: true,
            thickness: None,
            gap: None,
            text_gap: None,
            inset_start: 0.0,
            inset_end: 0.0,
            solid: Color32::from_rgb(0x32, 0x32, 0x36),
            stops: Vec::new(),
            orientation: "Auto".to_owned(),
            opacity: 1.0,
            hide_inactive_caret: true,
            caret_color: Color32::from_rgb(0xd0, 0xd4, 0xe0),
        }
    }
}

impl SettingsApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        // 项目目录:可执行文件同级向上找 config 目录,或当前工作目录。
        let project_dir = Self::detect_project_dir();
        let files = store::discover(Some(&project_dir));
        // 当前生效配置:current/ 里唯一那份 lua;目录为空就用程序自带默认配置。
        let active = store::active_config();
        let active_path = active.path.clone();
        let active_error = active.error.clone();
        // 键位引用的基准:有当前配置就用它,否则用程序默认 config.lua
        let main_config_path = active_path
            .clone()
            .unwrap_or_else(|| project_dir.join("config.lua"));
        // 当前配置里的 keys_file 引用(空 = 没引用任何键位文件)
        let base_text = active_path
            .as_deref()
            .map(store::read_text)
            .unwrap_or_else(|| store::read_text(&project_dir.join("config.lua")));
        let current_keymap = store::read_keys_file(&base_text).unwrap_or_default();
        let referenced_keymap = if current_keymap.is_empty() {
            None
        } else {
            store::resolve_keys_ref_str(&main_config_path, &current_keymap)
        };
        let keymap = referenced_keymap
            .clone()
            .filter(|p| p.exists())
            .map(ConfigFile::open);
        let mut app = SettingsApp {
            files,
            current: None,
            current_idx: None,
            // 自动化截图 / 排障用:无障碍环境下注入不了键盘,用环境变量预填搜索词
            search: std::env::var("VECTORTERM_SETTINGS_FILTER").unwrap_or_default(),
            preview: String::new(),
            preview_editing: false,
            color_picker: None,
            toast: None,
            project_dir,
            new_file_name: String::new(),
            show_new_file: false,
            active_path,
            active_error,
            keymaps: Vec::new(),
            current_keymap,
            keymap,
            new_keymap_name: String::new(),
            show_new_keymap: false,
            new_key_combo: String::new(),
            new_key_action: "reload".to_owned(),
        };
        app.refresh_files();
        app.open_first();
        app
    }

    fn detect_project_dir() -> PathBuf {
        // 优先:当前工作目录下若有 config 目录
        let cwd = std::env::current_dir().unwrap_or_default();
        let cand = cwd.join("config");
        if cand.exists() {
            return cand;
        }
        // 其次:可执行文件同级
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let c = dir.join("config");
                if c.exists() {
                    return c;
                }
            }
        }
        cwd.join("config")
    }

    /// 键位引用的基准配置路径:当前生效配置优先,没有就用程序默认 `config.lua`。
    fn main_config(&self) -> PathBuf {
        self.active_path
            .clone()
            .unwrap_or_else(|| self.project_dir.join("config.lua"))
    }

    /// 某个文件是否属于「键位文件」:在 `~/.config/VectorTerm/keys/` 里,命名像
    /// `keys.lua` / `keys1.lua`,或者它正是配置 `keys_file` 引用的那份。
    fn is_keymap_path(&self, path: &std::path::Path) -> bool {
        if store::is_keymap_path(path) {
            return true;
        }
        if self.current_keymap.is_empty() {
            return false;
        }
        store::resolve_keys_ref_str(&self.main_config(), &self.current_keymap)
            .map(|p| p == path)
            .unwrap_or(false)
    }

    /// 打开当前生效的配置;还没设置过就保持空(中栏会给出引导)。
    fn open_first(&mut self) {
        let Some(p) = self.active_path.clone() else {
            self.current = None;
            self.current_idx = None;
            self.preview.clear();
            return;
        };
        if let Some(idx) = self.files.iter().position(|f| f.path == p) {
            self.open(idx);
        }
    }

    fn open(&mut self, idx: usize) {
        // 若当前有未保存改动,先提示?简单起见直接切换(预览区保留了文本)。
        let path = self.files[idx].path.clone();
        let is_keymap = self.is_keymap_path(&path);
        self.current = Some(ConfigFile::open(path.clone()));
        self.current_idx = Some(idx);
        // 打开的是"当前配置"时,键位面板跟着它自己的 keys_file 走;
        // 打开别的文件只做浏览,键位面板仍对应终端真正加载的那份。
        if !is_keymap && self.active_path.as_deref() == Some(path.as_path()) {
            let text = store::read_text(&path);
            self.current_keymap = store::read_keys_file(&text).unwrap_or_default();
            self.keymap = if self.current_keymap.is_empty() {
                None
            } else {
                store::resolve_keys_ref_str(&path, &self.current_keymap)
                    .filter(|p| p.exists())
                    .map(ConfigFile::open)
            };
        }
        self.refresh_preview();
    }

    fn refresh_preview(&mut self) {
        self.preview = match &mut self.current {
            Some(c) => c.preview(),
            None => String::new(),
        };
    }

    fn field_status(&self, f: &Field) -> UiState {
        let Some(cur) = &self.current else {
            return UiState::Default;
        };
        let parts: Vec<&str> = f.parts();
        let (val, status) = cur.get(&parts);
        // 有改动待保存?
        let key: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
        if cur.dirty() && cur.has_change(&key) {
            return UiState::Modified;
        }
        match status {
            FieldStatus::Default => UiState::Default,
            FieldStatus::Expr => UiState::Expr,
            FieldStatus::Set => {
                if val.is_some() {
                    UiState::Set
                } else {
                    UiState::Default
                }
            }
        }
    }

    fn set_field(&mut self, f: &Field, value: Option<Value>) {
        if let Some(cur) = &mut self.current {
            let parts: Vec<&str> = f.parts();
            cur.set(&parts, value);
            self.refresh_preview();
        }
    }

    fn show_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((
            msg.into(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
        ));
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ctx);

        egui::TopBottomPanel::top("toolbar")
            .exact_height(40.0)
            .frame(egui::Frame::new().fill(theme::PANEL))
            .show(ctx, |ui| self.toolbar(ui));

        egui::TopBottomPanel::bottom("status")
            .exact_height(26.0)
            .frame(egui::Frame::new().fill(theme::PANEL2))
            .show(ctx, |ui| self.statusbar(ui));

        egui::SidePanel::left("files")
            .resizable(true)
            .default_width(200.0)
            .min_width(150.0)
            .max_width(320.0)
            .frame(egui::Frame::new().fill(theme::PANEL))
            .show(ctx, |ui| self.file_panel(ui));

        egui::SidePanel::right("preview")
            .resizable(true)
            .default_width(400.0)
            .min_width(280.0)
            .frame(egui::Frame::new().fill(theme::PANEL))
            .show(ctx, |ui| self.preview_panel(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG))
            .show(ctx, |ui| self.fields_panel(ui));

        // 颜色拾取器(模态弹窗)
        if self.color_picker.is_some() {
            self.color_picker_window(ctx);
        }
        if self.show_new_file {
            self.new_file_window(ctx);
        }
        if self.show_new_keymap {
            self.new_keymap_window(ctx);
        }

        // toast 自动消退
        if let Some((_, t)) = self.toast {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            if now - t > 3.0 {
                self.toast = None;
            } else {
                ctx.request_repaint();
            }
        }
    }
}

// ---------------------------------------------------------------------
// 各面板实现
// ---------------------------------------------------------------------

impl SettingsApp {
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        // 用 consume_key 而不是 `modifiers.command && key_pressed`:前者会把
        // 按键事件从队列里摘掉,避免焦点在搜索框 / 右侧代码编辑器时
        // 被 TextEdit 先消费掉导致快捷键失灵。必须在任何 widget 之前调用。
        let save = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S));
        if save {
            self.save_current();
            return;
        }
        let search = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F));
        if search {
            ctx.memory_mut(|m| m.request_focus(search_id()));
        }
        let open = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O));
        if open {
            self.add_existing_file();
        }
    }

    fn save_current(&mut self) {
        if self.current.is_none() {
            self.show_toast("请先在左侧选择一个配置文件");
            return;
        }
        // 1) 保存当前文件
        let mut save_err: Option<String> = None;
        let mut saved_path: Option<PathBuf> = None;
        if let Some(cur) = &mut self.current {
            match cur.save() {
                Ok(_) => saved_path = Some(cur.path.clone()),
                Err(e) => save_err = Some(format!("保存失败:{}", e)),
            }
        }
        if let Some(e) = save_err {
            self.show_toast(e);
            return;
        }
        let Some(saved) = saved_path else { return };

        // 2) 提示这份改动会不会被终端读到:只有 current/ 里的那份才是生效配置。
        let is_active = self.active_path.as_deref() == Some(saved.as_path());
        self.refresh_preview();
        self.refresh_files();
        if is_active {
            self.show_toast("已保存(当前配置,重启终端生效)");
        } else {
            let name = saved
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            self.show_toast(format!(
                "已保存 {name};它还不是当前配置 —— 点「设为当前配置」后重启终端才会生效"
            ));
        }

        // 3) 同时保存被引用的键位文件(若它与当前文件不是同一份)
        if let Some(km) = &mut self.keymap {
            if km.dirty() {
                let km_path = km.path.clone();
                let cur_path = self.current.as_ref().map(|c| c.path.clone());
                if Some(&km_path) != cur_path.as_ref() {
                    match km.save() {
                        Ok(_) => self.show_toast("键位文件已保存(重启终端生效)"),
                        Err(e) => self.show_toast(format!("键位文件保存失败:{}", e)),
                    }
                }
            }
        }
    }

    /// 重新扫描磁盘,并同步「当前配置 / 键位文件」清单与当前生效值。
    fn refresh_files(&mut self) {
        self.files = store::discover(Some(&self.project_dir));
        let active = store::active_config();
        self.active_path = active.path.clone();
        self.active_error = active.error.clone();
        // 当前配置在 current/ 里,不在常规扫描范围内 —— 单独插到最前面,
        // 左栏因此能一眼看到"现在生效的是哪一份"。
        if let Some(p) = &self.active_path {
            if !self.files.iter().any(|f| &f.path == p) {
                self.files.insert(
                    0,
                    FileItem {
                        path: p.clone(),
                        group: "当前配置",
                        exists: true,
                    },
                );
            }
        }
        let base_text = self
            .active_path
            .as_deref()
            .map(store::read_text)
            .unwrap_or_else(|| store::read_text(&self.project_dir.join("config.lua")));
        self.current_keymap = store::read_keys_file(&base_text).unwrap_or_default();

        self.keymaps = self
            .files
            .iter()
            .map(|f| f.path.clone())
            .filter(|p| self.is_keymap_path(p))
            .collect();

        // files 重建后下标会变,按当前打开的文件重新定位,否则左栏高亮会错位。
        self.current_idx = self
            .current
            .as_ref()
            .and_then(|c| self.files.iter().position(|f| f.path == c.path));
    }

    /// 切换当前配置引用的键位文件(`keys_file`)。
    ///
    /// 直接改写**当前生效配置**的 `keys_file` 字段并落盘。没有当前配置时先提示
    /// 用户「设为当前配置」—— 键位引用必须写在终端真正加载的那份文件里。
    fn apply_keymap(&mut self, ref_str: Option<String>) {
        let Some(target) = self.active_path.clone() else {
            self.show_toast("请先把某份配置设为当前配置,再挂键位文件");
            return;
        };
        let base = store::read_text(&target);
        let new_text = store::set_keys_file(&base, ref_str.as_deref());
        match store::write_text(&target, &new_text) {
            Ok(_) => {
                self.current_keymap = ref_str.clone().unwrap_or_default();
                // 重新加载被引用的键位文件(若引用的是还没落盘的新文件,则为 None)
                self.keymap = ref_str
                    .as_ref()
                    .and_then(|r| store::resolve_keys_ref_str(&target, r))
                    .filter(|p| p.exists())
                    .map(ConfigFile::open);
                // 当前打开的若正是这份配置,重新打开以显示新写入的 keys_file
                let is_open = self
                    .current
                    .as_ref()
                    .map(|c| c.path == target)
                    .unwrap_or(false);
                if is_open {
                    self.current = Some(ConfigFile::open(target.clone()));
                    self.current_idx = self.files.iter().position(|f| f.path == target);
                }
                self.refresh_preview();
                self.refresh_files();
                self.show_toast(match &ref_str {
                    Some(r) => format!("已引用键位文件 {r}(重启终端生效)"),
                    None => "已清除键位文件引用".into(),
                });
            }
            Err(e) => self.show_toast(format!("保存失败:{}", e)),
        }
    }

    /// 把某份 lua 复制进 `current/` 并设为当前配置(终端只加载这一份)。
    fn set_as_active(&mut self, path: PathBuf) {
        let label = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        match store::set_active_config(&path) {
            Ok(dest) => {
                self.active_path = Some(dest.clone());
                self.active_error = None;
                self.refresh_files();
                // 之后编辑的是 current/ 里的副本,改动保存即落到生效文件上
                if let Some(i) = self.files.iter().position(|f| f.path == dest) {
                    self.open(i);
                }
                self.show_toast(format!(
                    "已把 {label} 复制到 current/ 并设为当前配置(重启终端生效)"
                ));
            }
            Err(e) => self.show_toast(e),
        }
    }

    fn new_keymap_window(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let mut create = false;
        egui::Window::new("新建键位文件")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("键位方案名(会放在 ~/.config/VectorTerm/keys/ 下,作为 .lua 文件)");
                ui.text_edit_singleline(&mut self.new_keymap_name);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                    if ui
                        .add(egui::Button::new("创建并引用").fill(theme::ACCENT))
                        .clicked()
                    {
                        create = true;
                    }
                });
            });
        if close {
            self.show_new_keymap = false;
            self.new_keymap_name.clear();
        }
        if create {
            let name = self.new_keymap_name.trim().to_owned();
            self.show_new_keymap = false;
            self.new_keymap_name.clear();
            if name.is_empty() {
                self.show_toast("名称不能为空");
                return;
            }
            let name = if name.ends_with(".lua") {
                name
            } else {
                format!("{}.lua", name)
            };
            // 放在独立的键位目录:current/ 里只能有一份 lua,键位文件不能挤进去
            let dir = store::keys_dir();
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join(&name);
            if path.exists() {
                self.show_toast("文件已存在");
                return;
            }
            match store::write_text(&path, &store::keymap_template(&name)) {
                Ok(_) => {
                    self.refresh_files();
                    // 把它接进当前配置的 keys_file(裸文件名,终端会自动到 keys/ 里找)
                    let target = self.main_config();
                    self.apply_keymap(Some(store::keys_ref_for(&target, &path)));
                    // 直接打开新建的键位文件,开始编辑
                    if let Some(i) = self.files.iter().position(|f| f.path == path) {
                        self.open(i);
                    }
                    self.show_toast(format!("已创建 {name} 并引用(重启终端生效)"));
                }
                Err(e) => self.show_toast(format!("创建失败:{}", e)),
            }
        }
    }

    // --- 顶部工具栏 -------------------------------------------------
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                RichText::new("VectorTerm 配置管理器")
                    .color(theme::TEXT)
                    .strong()
                    .size(15.0),
            );

            // 当前配置:current/ 里唯一那份 lua。终端启动时只加载它;
            // 想换一份,在左栏点文件右键「设为当前配置」,或打开后点这里的按钮。
            ui.separator();
            ui.label(RichText::new("当前配置").color(theme::MUTED).size(12.0));
            let active_label = self
                .active_path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "程序默认".to_owned());
            ui.label(
                RichText::new(active_label)
                    .color(if self.active_path.is_some() {
                        theme::ACCENT
                    } else {
                        theme::MUTED
                    })
                    .size(12.0)
                    .strong(),
            )
            .on_hover_text(match &self.active_path {
                Some(p) => format!("终端加载:{}", p.display()),
                None => "current/ 为空,终端加载程序自带默认配置".to_owned(),
            });
            let cur_not_active = self
                .current
                .as_ref()
                .map(|c| Some(&c.path) != self.active_path.as_ref())
                .unwrap_or(false);
            if cur_not_active
                && ui
                    .add(
                        egui::Button::new(RichText::new("设为当前配置").size(11.0))
                            .fill(theme::ACCENT),
                    )
                    .on_hover_text("把当前打开的文件复制到 current/,终端下次启动就加载它")
                    .clicked()
            {
                if let Some(p) = self.current.as_ref().map(|c| c.path.clone()) {
                    self.set_as_active(p);
                }
            }

            // 键位方案:当前配置通过 keys_file 引用的外部键位 Lua 文件。下拉切换即改写
            // 配置的 keys_file 字段并保存,决定终端实际套用哪一份键位预设。
            ui.separator();
            ui.label(RichText::new("键位方案").color(theme::MUTED).size(12.0));
            let mut sel_km = self.current_keymap.clone();
            let mut changed_km = false;
            let km_text = if sel_km.is_empty() {
                "默认（无键位文件）".to_owned()
            } else {
                std::path::Path::new(&sel_km)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| sel_km.clone())
            };
            egui::ComboBox::from_id_salt("keymap_selector")
                .selected_text(km_text)
                .width(150.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(sel_km.is_empty(), "默认（无键位文件）")
                        .on_hover_text("清除 keys_file,回到只认主配置内联 keys / 纯默认")
                        .clicked()
                    {
                        sel_km = String::new();
                        changed_km = true;
                    }
                    for km in &self.keymaps {
                        let name = km
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let ref_str = store::keys_ref_for(&self.main_config(), km);
                        if ui
                            .selectable_label(self.current_keymap == ref_str, name.clone())
                            .on_hover_text(format!("主配置引用 {ref_str}"))
                            .clicked()
                        {
                            sel_km = ref_str;
                            changed_km = true;
                        }
                    }
                });
            if changed_km {
                self.apply_keymap(if sel_km.is_empty() {
                    None
                } else {
                    Some(sel_km.clone())
                });
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("新建键位文件").size(11.0))
                        .fill(theme::PANEL3),
                )
                .on_hover_text("创建一个新的键位 Lua 文件(keys.lua → keys1.lua),并把它接进主配置")
                .clicked()
            {
                self.open_new_keymap_dialog();
            }

            ui.separator();
            let dirty = self.current.as_ref().map(|c| c.dirty()).unwrap_or(false)
                || self.keymap.as_ref().map(|c| c.dirty()).unwrap_or(false);
            ui.label(
                RichText::new(if dirty { "● 有未保存改动" } else { "无改动" })
                    .color(if dirty { theme::WARN } else { theme::DIM })
                    .size(11.0),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        dirty,
                        egui::Button::new(RichText::new("保存").color(theme::BG))
                            .fill(theme::ACCENT),
                    )
                    .on_hover_text("Ctrl+S 保存当前配置文件与键位文件")
                    .clicked()
                {
                    self.save_current();
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("撤销全部").color(theme::MUTED))
                            .fill(theme::PANEL2),
                    )
                    .on_hover_text("放弃所有未保存的改动,回到磁盘上的版本")
                    .clicked()
                {
                    if let Some(cur) = &mut self.current {
                        cur.reload();
                        self.refresh_preview();
                    }
                }
            });
        });
    }

    // --- 底部状态栏 -------------------------------------------------
    fn statusbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let cur = self.current.as_ref();
            let path = cur
                .map(|c| c.path.display().to_string())
                .unwrap_or_default();
            ui.label(RichText::new(path).color(theme::MUTED).size(11.0));
            ui.separator();
            ui.label(
                RichText::new("Ctrl+S 保存 · Ctrl+F 搜索 · Ctrl+O 打开")
                    .color(theme::DIM)
                    .size(10.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((msg, _)) = &self.toast {
                    ui.label(RichText::new(msg).color(theme::OK).size(11.0));
                }
                let n = self.current.as_ref().map(|c| c.change_count()).unwrap_or(0)
                    + self.keymap.as_ref().map(|c| c.change_count()).unwrap_or(0);
                ui.label(
                    RichText::new(format!("{} 项待保存", n))
                        .color(if n > 0 { theme::WARN } else { theme::DIM })
                        .size(11.0),
                );
            });
        });
    }

    // --- 左栏:配置文件列表 -----------------------------------------
    fn file_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        // ---- 上栏标题:配置文件 ----
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("配置文件").strong().color(theme::TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("+").color(theme::TEXT))
                            .fill(theme::PANEL3),
                    )
                    .on_hover_text("新建配置")
                    .clicked()
                {
                    self.show_new_file = true;
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("…").color(theme::TEXT))
                            .fill(theme::PANEL3),
                    )
                    .on_hover_text("添加已有配置")
                    .clicked()
                {
                    self.add_existing_file();
                }
            });
        });
        ui.separator();

        // ---- 当前配置状态:终端只会加载 current/ 里这一份 ----
        if let Some(err) = self.active_error.clone() {
            ui.label(
                RichText::new(format!("⚠ {err}"))
                    .color(egui::Color32::from_rgb(0xE0, 0x6C, 0x75))
                    .size(10.0),
            );
        } else {
            let (label, color) = match &self.active_path {
                Some(p) => (
                    format!(
                        "● 当前配置:{}",
                        p.file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default()
                    ),
                    theme::ACCENT,
                ),
                None => ("当前配置:程序自带默认(尚未选择)".to_owned(), theme::MUTED),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(color).size(10.5));
                if self.active_path.is_some()
                    && ui
                        .add(egui::Button::new(RichText::new("清除").size(10.0)).fill(theme::PANEL3))
                        .on_hover_text("清空 current/ 目录,终端回到程序自带默认配置")
                        .clicked()
                {
                    match store::clear_active_config() {
                        Ok(_) => {
                            self.active_path = None;
                            self.refresh_files();
                            self.open_first();
                            self.show_toast("已清除当前配置(终端回到程序默认)");
                        }
                        Err(e) => self.show_toast(e),
                    }
                }
            });
        }
        ui.label(
            RichText::new("点文件右键「设为当前配置」:复制进 current/,终端只加载它。")
                .color(theme::DIM)
                .size(10.0),
        );

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let active = self.active_path.clone();
                let mut to_open: Option<usize> = None;
                let mut to_delete: Option<PathBuf> = None;
                let mut to_activate: Option<PathBuf> = None;
                let mut key_open: Option<usize> = None;

                // ---- 配置文件列表(排除键位文件) ----
                let mut last_group = "";
                for (idx, item) in self.files.iter().enumerate() {
                    let fname = item
                        .path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if self.is_keymap_path(&item.path) {
                        continue;
                    }
                    if item.group != last_group {
                        ui.add_space(6.0);
                        ui.label(RichText::new(item.group).color(theme::DIM).size(10.0));
                        last_group = item.group;
                    }
                    let is_active = active.as_deref() == Some(item.path.as_path());
                    let name = if is_active {
                        format!("● {fname}")
                    } else {
                        fname
                    };
                    let selected = self.current_idx == Some(idx);
                    let resp = ui.selectable_label(
                        selected,
                        RichText::new(&name)
                            .color(if selected { theme::ACCENT } else { theme::TEXT }),
                    );
                    if resp.clicked() {
                        to_open = Some(idx);
                    }
                    resp.context_menu(|ui| {
                        if ui
                            .button("设为当前配置")
                            .on_hover_text("复制进 ~/.config/VectorTerm/current/,终端下次启动加载它")
                            .clicked()
                        {
                            to_activate = Some(item.path.clone());
                            ui.close_menu();
                        }
                        if item.group != "程序目录"
                            && ui.button("删除").clicked()
                        {
                            to_delete = Some(item.path.clone());
                            ui.close_menu();
                        }
                    });
                }

                // ---- 下栏:键位文件 ----
                ui.add_space(12.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("键位文件").strong().color(theme::TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("+").color(theme::TEXT))
                                    .fill(theme::PANEL3),
                            )
                            .on_hover_text("新建键位文件(keys.lua → keys1.lua → …)")
                            .clicked()
                        {
                            self.open_new_keymap_dialog();
                        }
                    });
                });
                ui.label(
                    RichText::new("存放于 ~/.config/VectorTerm/keys/;点击即挂到当前配置并编辑。")
                        .color(theme::DIM)
                        .size(10.0),
                );
                let cur_km = std::path::Path::new(&self.current_keymap)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                for (idx, item) in self.files.iter().enumerate() {
                    let fname = item
                        .path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if !self.is_keymap_path(&item.path) {
                        continue;
                    }
                    let selected = self.current_idx == Some(idx);
                    // ● 标出当前配置 keys_file 引用的那份
                    let mark = if !cur_km.is_empty() && cur_km == fname {
                        "● "
                    } else {
                        ""
                    };
                    let resp = ui.selectable_label(
                        selected,
                        RichText::new(format!("{mark}{fname}"))
                            .color(if selected { theme::ACCENT } else { theme::TEXT }),
                    );
                    if resp.clicked() {
                        key_open = Some(idx);
                    }
                    resp.context_menu(|ui| {
                        if ui.button("删除").clicked() {
                            to_delete = Some(item.path.clone());
                            ui.close_menu();
                        }
                    });
                }

                if let Some(idx) = to_open {
                    self.open(idx);
                }
                if let Some(p) = to_activate {
                    self.set_as_active(p);
                }
                if let Some(p) = to_delete {
                    self.delete_file(p);
                }
                if let Some(idx) = key_open {
                    // 点击键位文件 = 打开它 + 挂到当前配置
                    let path = self.files[idx].path.clone();
                    let ref_str = store::keys_ref_for(&self.main_config(), &path);
                    self.apply_keymap(Some(ref_str));
                    if let Some(i) = self.files.iter().position(|f| f.path == path) {
                        self.open(i);
                    }
                }
            });
    }

    /// 打开「新建键位文件」弹窗,并按 keys/keys1/keys2 … 规则预填文件名。
    fn open_new_keymap_dialog(&mut self) {
        let dir = store::keys_dir();
        let _ = std::fs::create_dir_all(&dir);
        self.new_keymap_name = store::next_keymap_filename(&dir);
        self.show_new_keymap = true;
    }

    fn add_existing_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Lua 配置", &["lua"])
            .pick_file()
        {
            // 外部文件没法长期显示在左栏(那里只列固定目录),直接设成当前配置:
            // 复制进 current/ 后它就变成终端真正加载的那一份,也能持续编辑。
            self.set_as_active(path);
        }
    }

    fn delete_file(&mut self, path: PathBuf) {
        // 删之前先解除引用:否则当前配置会留下指向已删键位文件的 keys_file。
        self.drop_refs_to(&path);
        let was_active = self.active_path.as_deref() == Some(path.as_path());
        if let Err(e) = std::fs::remove_file(&path) {
            self.show_toast(format!("删除失败:{}", e));
        } else {
            self.show_toast(if was_active {
                "已删除当前配置 —— 终端将退回程序默认配置"
            } else {
                "已删除"
            });
            self.refresh_files();
            self.open_first();
            if self.current.is_none() {
                self.preview.clear();
            }
        }
    }

    /// 解除当前配置对某个文件的引用(目前只有 `keys_file`)。
    fn drop_refs_to(&mut self, path: &std::path::Path) {
        let referenced = if self.current_keymap.is_empty() {
            false
        } else {
            store::resolve_keys_ref_str(&self.main_config(), &self.current_keymap)
                .map(|p| p == path)
                .unwrap_or(false)
        };
        if referenced {
            self.apply_keymap(None);
            self.keymap = None;
        }
    }

    fn new_file_window(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let mut create = false;
        egui::Window::new("新建配置")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("文件名(会放在用户配置目录 ~/.config/VectorTerm/ 下)");
                ui.text_edit_singleline(&mut self.new_file_name);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                    if ui
                        .add(egui::Button::new("创建").fill(theme::ACCENT))
                        .clicked()
                    {
                        create = true;
                    }
                });
            });
        if close {
            self.show_new_file = false;
            self.new_file_name.clear();
        }
        if create {
            let name = self.new_file_name.trim().to_owned();
            self.show_new_file = false;
            self.new_file_name.clear();
            if name.is_empty() {
                self.show_toast("文件名不能为空");
                return;
            }
            let mut path = store::user_config_dir();
            if let Err(e) = std::fs::create_dir_all(&path) {
                self.show_toast(format!("创建目录失败:{}", e));
                return;
            }
            let name = if name.ends_with(".lua") {
                name
            } else {
                format!("{}.lua", name)
            };
            path.push(name);
            if path.exists() {
                self.show_toast("文件已存在");
            } else {
                std::fs::write(&path, store::empty_config()).ok();
                self.refresh_files();
                if let Some(idx) = self.files.iter().position(|f| f.path == path) {
                    self.open(idx);
                }
            }
        }
    }

    // --- 右栏:预览 -------------------------------------------------
    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Lua 预览").strong().color(theme::TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let editing = self.preview_editing;
                if ui
                    .selectable_label(
                        editing,
                        RichText::new("编辑").color(if editing {
                            theme::ACCENT
                        } else {
                            theme::MUTED
                        }),
                    )
                    .on_hover_text("切换为手动编辑模式")
                    .clicked()
                {
                    self.preview_editing = !self.preview_editing;
                }
            });
        });
        ui.separator();

        if self.preview_editing {
            egui::ScrollArea::both().show(ui, |ui| {
                let mut text = self.preview.clone();
                let resp = ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .font(egui::TextStyle::Monospace)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(30),
                );
                if resp.changed() {
                    self.preview = text.clone();
                }
                if resp.lost_focus() {
                    // 停手后同步回控件(解析并写回 current)
                    if let Some(cur) = &mut self.current {
                        cur.set_text(text.clone());
                    }
                }
            });
        } else {
            // 只读预览 + 语法着色(用 egui_extras 的 syntect 或简单着色)
            egui::ScrollArea::both().show(ui, |ui| {
                self.show_code(ui);
            });
        }

        ui.add_space(4.0);
        ui.label(
            RichText::new(if self.preview_editing {
                "手动编辑模式:改动会在失去焦点时解析回左侧"
            } else {
                "只读预览;点右上角「编辑」可直接改文本"
            })
            .color(theme::DIM)
            .size(10.0),
        );
    }

    fn show_code(&self, ui: &mut egui::Ui) {
        // 简单 Lua 语法着色(不做完整高亮,区分注释/字符串/关键字即可)。
        let code = self.preview.clone();
        let mut job = egui::text::LayoutJob::default();
        let mono = egui::TextStyle::Monospace;
        color_lua(&code, &mut job, mono);
        ui.add(egui::Label::new(job).extend());
    }

    // --- 自定义键位面板 -------------------------------------------
    /// 当前编辑目标是不是「键位文件」(当前打开的是键位文件)。
    fn current_is_keymap(&self) -> bool {
        self.current
            .as_ref()
            .map(|c| self.is_keymap_path(&c.path))
            .unwrap_or(false)
    }

    /// 把一条键位改动写进"键位文件"。
    ///
    /// 目标优先级:当前打开的键位文件 → 主配置 `keys_file` 引用的文件。两者都
    /// 没有就先建一个 `keys.lua` 并引用,再写进去。**绝不写进 config.lua /
    /// 配置分片** —— 主配置的职责只是引用,不该出现具体配置项。
    fn write_key(&mut self, norm: &str, value: Option<Value>) {
        if self.current_is_keymap() {
            if let Some(cur) = &mut self.current {
                cur.set(&["keys", norm], value);
                return;
            }
        }
        if self.keymap.is_none() {
            self.focus_keymap();
        }
        if self.current_is_keymap() {
            if let Some(cur) = &mut self.current {
                cur.set(&["keys", norm], value);
                return;
            }
        }
        if let Some(km) = &mut self.keymap {
            km.set(&["keys", norm], value);
        }
    }

    /// 在生效的键位文件上增删一条键位。
    fn set_key(&mut self, combo: &str, value: Option<String>) {
        let norm = combo.trim().to_lowercase();
        if norm.is_empty() {
            return;
        }
        self.write_key(&norm, value.map(Value::Str));
        self.refresh_preview();
    }

    /// 键位映射分组:列出当前键位,支持删除与新增。这是"自定义键位"的主要界面。
    fn keys_group(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("键位映射")
                    .strong()
                    .color(theme::TEXT)
                    .size(14.0),
            );
            ui.label(
                RichText::new("（自定义键位 · 未列出的组合键退回内置默认）")
                    .color(theme::DIM)
                    .size(10.0),
            );
        });
        ui.separator();

        // 与写入口径一致:当前打开的是键位文件就用它(未保存改动也能实时展示),
        // 否则展示主配置 keys_file 引用的那份。
        let target_is_current = self.current_is_keymap();
        let (preview_text, target_name) = if target_is_current {
            let name = self
                .current
                .as_ref()
                .and_then(|c| c.path.file_name())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            (self.current.as_mut().map(|c| c.preview()).unwrap_or_default(), name)
        } else {
            let name = self
                .keymap
                .as_ref()
                .and_then(|c| c.path.file_name())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "（未引用键位文件）".to_owned());
            (
                self.keymap.as_mut().map(|c| c.preview()).unwrap_or_default(),
                name,
            )
        };
        ui.label(
            RichText::new(format!("写入目标:{target_name}"))
                .color(theme::DIM)
                .size(10.0),
        );
        let entries = store::keys_entries(&preview_text);

        egui::ScrollArea::vertical()
            .max_height(180.0)
            .show(ui, |ui| {
                if entries.is_empty() {
                    ui.label(
                        RichText::new("（暂无自定义键位；下方添加后会生成 keys 表）")
                            .color(theme::DIM)
                            .size(10.5),
                    );
                }
                for (combo, action) in &entries {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(combo)
                                .color(theme::ACCENT)
                                .size(11.5),
                        );
                        ui.label(
                            RichText::new(action)
                                .color(theme::TEXT)
                                .size(11.0),
                        );
                        if ui
                            .small_button("×")
                            .on_hover_text("删除此键位")
                            .clicked()
                        {
                            self.set_key(combo, None);
                        }
                    });
                }
            });
        ui.separator();

        // 新增表单:组合键 + 动作(内置动作或 send: 文本)
        ui.horizontal(|ui| {
            ui.label(RichText::new("新增").color(theme::MUTED).size(11.0));
            ui.add(
                egui::TextEdit::singleline(&mut self.new_key_combo)
                    .hint_text("组合键 如 ctrl+shift+k"),
            );
            egui::ComboBox::from_id_salt("new_key_action")
                .selected_text(&self.new_key_action)
                .width(140.0)
                .show_ui(ui, |ui| {
                    for a in [
                        "reload",
                        "quit",
                        "copy",
                        "paste",
                        "new_tab",
                        "close_tab",
                        "next_tab",
                        "prev_tab",
                        "files_gui",
                    ] {
                        if ui
                            .selectable_label(self.new_key_action == a, a)
                            .clicked()
                        {
                            self.new_key_action = a.to_owned();
                        }
                    }
                    if ui
                        .selectable_label(self.new_key_action == "send:", "send:文本")
                        .clicked()
                    {
                        self.new_key_action = "send:".to_owned();
                    }
                });
            // 选了 send: 且还想带文本时,追加一个文本输入框
            if self.new_key_action.starts_with("send:") {
                let mut s = self.new_key_action.clone();
                if ui.text_edit_singleline(&mut s).changed() {
                    self.new_key_action = s;
                }
            }
            if ui.small_button("添加").clicked() {
                let combo = self.new_key_combo.trim().to_lowercase();
                if combo.is_empty() || self.new_key_action.is_empty() {
                    self.show_toast("组合键与动作都要填");
                } else {
                    self.set_key(&combo, Some(self.new_key_action.clone()));
                    self.new_key_combo.clear();
                }
            }
        });
        ui.label(
            RichText::new(
                "组合键小写、修饰键顺序无关；动作可选内置动作或 send:文本(把一段文字发进终端)。\
                 详细动作清单见键位文件模板注释与 docs/keyboard-shortcuts.md。",
            )
            .color(theme::DESC)
            .size(10.0),
        );
        ui.add_space(6.0);
    }

    // --- 中栏:配置项 -----------------------------------------------
    fn fields_panel(&mut self, ui: &mut egui::Ui) {
        // 当前打开文件的角色:主配置 / 键位文件 / 配置分片
        let cur_path = self.current.as_ref().map(|c| c.path.clone());
        let fname = cur_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let is_keymap = cur_path
            .as_deref()
            .map(|p| self.is_keymap_path(p))
            .unwrap_or(false);
        let is_active = cur_path.is_some() && cur_path == self.active_path;

        let title = if self.current.is_none() {
            "还没有当前配置"
        } else if is_keymap {
            "键位方案"
        } else if is_active {
            "配置项 · 当前生效"
        } else {
            "配置项 · 未生效"
        };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(title).strong().color(theme::TEXT));
            if self.current.is_some() && !is_keymap {
                ui.label(
                    RichText::new(fname)
                        .color(if is_active { theme::ACCENT } else { theme::MUTED })
                        .size(11.0),
                );
                if !is_active
                    && ui
                        .add(
                            egui::Button::new(RichText::new("设为当前配置").size(11.0))
                                .fill(theme::ACCENT),
                        )
                        .on_hover_text("复制进 ~/.config/VectorTerm/current/,终端下次启动加载它")
                        .clicked()
                {
                    if let Some(p) = cur_path.clone() {
                        self.set_as_active(p);
                    }
                    return;
                }
                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .id(search_id())
                        .hint_text("搜索配置项…  (Ctrl+F)")
                        .desired_width(220.0),
                );
            }
        });
        ui.separator();

        // 还没有当前配置:引导用户先把某份 lua 复制进 current/
        if self.current.is_none() {
            self.welcome_panel(ui);
            return;
        }

        // 解析失败横幅:告诉用户为什么这个文件不能保存,避免一头雾水
        if let Some(err) = self.current.as_ref().and_then(|c| c.error.clone()) {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("⚠ {err}"))
                    .color(egui::Color32::from_rgb(0xE0, 0x6C, 0x75))
                    .size(11.0),
            );
            ui.separator();
        }

        // 键位文件:显示键位映射编辑器
        if is_keymap {
            self.keys_group(ui);
            return;
        }

        let q = self.search.trim().to_lowercase();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for group in schema::GROUPS {
                    let fields: Vec<&Field> = group
                        .fields
                        .iter()
                        .filter(|f| {
                            q.is_empty()
                                || f.label.to_lowercase().contains(&q)
                                || f.path.to_lowercase().contains(&q)
                                || f.desc.to_lowercase().contains(&q)
                        })
                        .collect();
                    if fields.is_empty() {
                        continue;
                    }
                    ui.add_space(10.0);
                    self.group_header(ui, group);
                    ui.separator();
                    // 分割线这一组给一条所见即所得的预览:调粗细 / 留白 / 渐变色
                    // 时不用切回终端窗口就能看到效果。
                    if group.key == "split" {
                        self.divider_gap_bar(ui);
                        self.divider_preview(ui);
                        ui.separator();
                    }
                    // 跨窗光标动画:一键把它的速度 / 颜色 / 透明度对齐到普通尾迹
                    if group.key == "pane_focus" {
                        self.pane_focus_sync_bar(ui);
                        ui.separator();
                    }
                    for f in fields {
                        self.field_row(ui, f);
                    }
                }
            });
    }

    /// 还没有当前配置时的引导页。
    ///
    /// 终端启动时只认 `~/.config/VectorTerm/current/` 里唯一那份 lua;目录为空就
    /// 加载程序自带默认配置。这里给两条出路:用程序默认配置开一份,或者在
    /// 左栏挑一个已有 lua「设为当前配置」。
    fn welcome_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(18.0);
        ui.label(
            RichText::new("终端现在用的是程序自带默认配置")
                .strong()
                .color(theme::TEXT)
                .size(15.0),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "配置管理器会把选中的 lua 复制到  ~/.config/VectorTerm/current/ ,\
                 终端启动时只加载那个目录里唯一的一份(文件名随意)。\
                 目录为空 → 用程序默认;目录里有多份 → 报错并退回程序默认。",
            )
            .color(theme::DESC)
            .size(11.5),
        );
        ui.add_space(12.0);
        if ui
            .add(
                egui::Button::new(RichText::new("以程序默认配置创建当前配置").color(theme::BG))
                    .fill(theme::ACCENT),
            )
            .on_hover_text("把 config/config.lua 复制成当前配置,之后改什么都落在这份上")
            .clicked()
        {
            let src = self.project_dir.join("config.lua");
            if src.exists() {
                self.set_as_active(src);
            } else {
                self.show_toast(format!("程序目录里没有 config.lua:{}", src.display()));
            }
        }
        ui.add_space(10.0);
        ui.label(
            RichText::new(
                "也可以在左栏点任意 .lua 打开,再点顶部的「设为当前配置」。\
                 旧版的 ~/.config/VectorTerm/config.lua 已不再参与加载。",
            )
            .color(theme::DIM)
            .size(10.5),
        );
    }

    /// 打开当前配置引用的键位文件;若还没引用,自动在 `keys/` 里新建 `keys.lua`
    /// 并写进配置的 `keys_file`,然后打开它。
    fn focus_keymap(&mut self) {
        let existing = if let Some(km) = &self.keymap {
            Some(km.path.clone())
        } else if !self.current_keymap.is_empty() {
            store::resolve_keys_ref_str(&self.main_config(), &self.current_keymap)
        } else {
            None
        }
        .filter(|p| p.exists());
        if let Some(p) = existing {
            if let Some(i) = self.files.iter().position(|f| f.path == p) {
                self.open(i);
                return;
            }
        }

        let target = self.main_config();
        let dir = store::keys_dir();
        let _ = std::fs::create_dir_all(&dir);
        let name = store::next_keymap_filename(&dir);
        let path = dir.join(&name);
        match store::write_text(&path, &store::keymap_template(&name)) {
            Ok(_) => {
                self.refresh_files();
                let ref_str = store::keys_ref_for(&target, &path);
                self.apply_keymap(Some(ref_str));
                if let Some(i) = self.files.iter().position(|f| f.path == path) {
                    self.open(i);
                }
                self.show_toast(format!("已新建并引用 {name}"));
            }
            Err(e) => self.show_toast(format!("创建失败:{}", e)),
        }
    }

    fn group_header(&self, ui: &mut egui::Ui, group: &Group) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(group.title)
                    .strong()
                    .color(theme::TEXT)
                    .size(14.0),
            );
            ui.label(RichText::new(group.chapter).color(theme::DIM).size(10.0));
        });
        if !group.desc.is_empty() {
            ui.label(RichText::new(group.desc).color(theme::DESC).size(11.0));
        }
    }

    // --- 分屏分割线实时预览 -------------------------------------------
    /// 读取当前配置文件里 split_divider 的生效值(没设的键取默认值)。
    fn divider_preview_spec(&self) -> DividerPreviewSpec {
        let get = |path: &[&str]| -> Option<Value> {
            self.current.as_ref().and_then(|c| c.get(path).0)
        };
        let mut s = DividerPreviewSpec::default();

        if let Some(Value::Bool(b)) = get(&["split_divider", "enabled"]) {
            s.enabled = b;
        }
        if let Some(v) = val_as_f64(&get(&["split_divider", "thickness"])) {
            s.thickness = Some(v as f32);
        }
        if let Some(v) = val_as_f64(&get(&["split_divider", "gap"])) {
            s.gap = Some(v as f32);
        }
        if let Some(v) = val_as_f64(&get(&["split_divider", "text_gap"])) {
            s.text_gap = Some(v as f32);
        }
        if let Some(Value::Bool(b)) = get(&["pane_focus", "hide_inactive_caret"]) {
            s.hide_inactive_caret = b;
        }
        // 预览里那个小光标块的颜色:与终端同一套优先级 —— 同步时跟随
        // cursor.trail_color,关掉同步才看 pane_focus.trail_color。
        let synced = !matches!(
            get(&["pane_focus", "sync_trail"]),
            Some(Value::Bool(false))
        );
        let caret_src = if synced {
            get(&["cursor", "trail_color"])
        } else {
            get(&["pane_focus", "trail_color"]).or_else(|| get(&["cursor", "trail_color"]))
        };
        if let Some(c) = caret_src.and_then(|v| parse_hex_color(&value_to_display(&Some(v)))) {
            s.caret_color = c;
        }
        // inset = N 是两端相同的简写,随后可被单端设置覆盖
        if let Some(v) = val_as_f64(&get(&["split_divider", "inset"])) {
            s.inset_start = v as f32;
            s.inset_end = v as f32;
        }
        if let Some(v) = val_as_f64(&get(&["split_divider", "inset_start"])) {
            s.inset_start = v as f32;
        }
        if let Some(v) = val_as_f64(&get(&["split_divider", "inset_end"])) {
            s.inset_end = v as f32;
        }
        if let Some(c) = get(&["split_divider", "color"])
            .and_then(|v| parse_hex_color(&value_to_display(&Some(v))))
        {
            s.solid = c;
        }
        if let Some(Value::Table(lua::Table::Array(arr))) =
            get(&["split_divider", "gradient", "colors"])
        {
            s.stops = arr
                .iter()
                .filter_map(|v| parse_hex_color(&value_to_display_ref(v)))
                .collect();
        }
        if let Some(v) = get(&["split_divider", "gradient", "orientation"]) {
            s.orientation = value_to_display(&Some(v));
        }
        if let Some(v) = val_as_f64(&get(&["split_divider", "opacity"])) {
            s.opacity = (v as f32).clamp(0.0, 1.0);
        }
        s
    }

    fn divider_preview(&mut self, ui: &mut egui::Ui) {
        let s = self.divider_preview_spec();
        ui.label(
            RichText::new("实时预览 · 左右分屏(竖线)")
                .color(theme::MUTED)
                .size(11.0),
        );
        self.paint_divider_preview(ui, &s, true);
        ui.add_space(4.0);
        ui.label(
            RichText::new("实时预览 · 上下分屏(横线)")
                .color(theme::MUTED)
                .size(11.0),
        );
        self.paint_divider_preview(ui, &s, false);
        ui.add_space(3.0);
        let note = if !s.enabled {
            "分割线已关闭(缝仍在,露出窗口背景)".to_owned()
        } else {
            let kind = if s.stops.len() >= 2 {
                format!("渐变 · {} 个停靠点 · {}", s.stops.len(), s.orientation)
            } else {
                "单色".to_owned()
            };
            let t = s
                .thickness
                .map(|v| format!("{v}px"))
                .unwrap_or_else(|| "自动 3px".to_owned());
            let spacing = if let Some(tg) = s.text_gap {
                let unit_t = s.thickness.unwrap_or(3.0).max(0.0);
                format!("文字距离 {tg}px(缝 {:.0}px)", 2.0 * tg + unit_t)
            } else if let Some(g) = s.gap {
                format!("缝宽 {g}px")
            } else {
                "间距自动(随字号)".to_owned()
            };
            format!(
                "{kind} · 线宽 {t} · {spacing} · 两端留白 {}/{}px",
                s.inset_start, s.inset_end
            )
        };
        ui.label(RichText::new(note).color(theme::ACCENT).size(10.0));
        ui.label(
            RichText::new("留白按 400px 内容区等比缩放示意,实际以终端窗口为准")
                .color(theme::DIM)
                .size(10.0),
        );
        ui.add_space(4.0);
    }

    /// 「文字到分割线的距离」调节条 —— `split_divider.text_gap` 的快捷入口。
    ///
    /// 总缝宽只是个中间量:用户真正看见的是"文字离那根线多远"。这里直接把这个
    /// 距离做成滑杆 + 常用档位,并实时显示它换算出的缝宽,预览图会同步变化。
    /// 自动 = 只留 1px 空气(缝宽 = 线宽 + 2),不随字号 / DPI 变。
    fn divider_gap_bar(&mut self, ui: &mut egui::Ui) {
        let read = |path: &[&str]| -> Option<f64> {
            self.current
                .as_ref()
                .and_then(|c| c.get(path).0)
                .and_then(|v| val_as_f64(&Some(v)))
        };
        let cur = read(&["split_divider", "text_gap"]);
        let line_t = read(&["split_divider", "thickness"])
            .map(|t| t.max(0.0))
            .unwrap_or(3.0);

        let mut want = cur;
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(
                RichText::new("文字到分割线的距离")
                    .size(12.0)
                    .color(theme::TEXT),
            );
            let mut auto = cur.is_none();
            if ui
                .checkbox(&mut auto, "自动")
                .on_hover_text("不写 split_divider.text_gap,只留 1px 空气")
                .changed()
            {
                want = if auto { None } else { Some(4.0) };
            }
            if let Some(v) = want {
                let mut d = v as f32;
                // 滑杆默认 0~24;配置里已经写了更大的值(手改 Lua)就别把它夹没
                let hi = cur.unwrap_or(0.0).max(24.0) as f32;
                let resp = ui.add(
                    egui::Slider::new(&mut d, 0.0..=hi)
                        .step_by(0.5)
                        .suffix(" px"),
                );
                if resp.changed() {
                    want = Some(d as f64);
                }
            }
            for preset in [0.0f64, 2.0, 4.0, 8.0] {
                let on = cur
                    .map(|v| (v - preset).abs() < 1e-6)
                    .unwrap_or(false);
                if ui
                    .selectable_label(on, format!("{}", preset as i64))
                    .on_hover_text(format!("文字到线边缘 {preset}px"))
                    .clicked()
                {
                    want = Some(preset);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let text = match want {
                Some(d) => format!(
                    "缝宽 {:.1}px = 2×{d:.1} + 线宽 {line_t:.1} · 两侧文字各让出 {d:.1}px",
                    2.0 * d + line_t
                ),
                None => format!(
                    "自动:{:.1}px(只留 1px 空气,换字号不用重调)",
                    line_t + 2.0
                ),
            };
            ui.label(RichText::new(text).color(theme::DIM).size(10.0));
        });
        if want != cur {
            if let Some(cfg) = &mut self.current {
                cfg.set(&["split_divider", "text_gap"], want.map(Value::Num));
            }
            self.refresh_preview();
        }
        ui.add_space(2.0);
    }

    /// 跨窗光标动画的「同步」按钮:一键把它的速度曲线、尾迹颜色、不透明度
    /// 对齐到普通光标尾迹(`cursor.trail_*`)。
    ///
    /// 同步是**继承**而不是复制:写回时直接删掉 `pane_focus.sync_trail` /
    /// `pane_focus.trail_color`,终端侧就按默认行为跟随 `cursor.trail_*`,
    /// 以后改普通尾迹的参数,跨窗动画自动一起变。
    fn pane_focus_sync_bar(&mut self, ui: &mut egui::Ui) {
        let sync_val = self
            .current
            .as_ref()
            .and_then(|c| c.get(&["pane_focus", "sync_trail"]).0);
        // 没写这个键 = 默认开启
        let synced = !matches!(sync_val, Some(Value::Bool(false)));

        let get = |path: &[&str]| -> Option<Value> {
            self.current.as_ref().and_then(|c| c.get(path).0)
        };
        let fast = val_as_f64(&get(&["cursor", "trail_decay_fast"])).unwrap_or(0.1);
        let slow = val_as_f64(&get(&["cursor", "trail_decay_slow"])).unwrap_or(0.4);
        let color = get(&["cursor", "trail_color"])
            .map(|v| value_to_display(&Some(v)))
            .filter(|s| {
                !s.is_empty()
                    && !s.eq_ignore_ascii_case("none")
                    && !s.eq_ignore_ascii_case("auto")
            })
            .unwrap_or_else(|| "跟随光标色".to_owned());
        let summary = format!("decay {fast}/{slow} · {color}");

        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let resp = ui
                .add(
                    egui::Button::new(
                        RichText::new("与普通光标尾迹同步").size(12.0),
                    )
                    .min_size(egui::vec2(160.0, 22.0)),
                )
                .on_hover_text(
                    "把跨窗飞行的速度曲线、尾迹颜色、不透明度全部对齐到 cursor.trail_*;\
                     会清掉单独的「飞行光标颜色」",
                );
            if resp.clicked() {
                if let Some(cur) = &mut self.current {
                    cur.set(&["pane_focus", "sync_trail"], None);
                    cur.set(&["pane_focus", "trail_color"], None);
                }
                self.refresh_preview();
                self.show_toast("跨窗光标动画已与普通光标尾迹同步");
            }
            if synced {
                ui.label(RichText::new("已同步").color(theme::OK).size(10.5));
            } else {
                ui.label(
                    RichText::new("当前使用单独设置")
                        .color(theme::WARN)
                        .size(10.5),
                );
            }
            ui.label(
                RichText::new(format!("cursor.trail_* = {summary}"))
                    .color(theme::DIM)
                    .size(10.0),
            );
        });
        ui.add_space(2.0);
    }

    /// 画一张「两个 pane + 中间缝 + 分割线」的缩略图。渐变同样切成 64 段,
    /// 与终端本体按同一套停靠点插值,所见即所得。
    fn paint_divider_preview(&self, ui: &mut egui::Ui, s: &DividerPreviewSpec, vertical: bool) {
        let size = egui::vec2(ui.available_width().min(320.0), 62.0);
        let (card, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let p = ui.painter();
        let backdrop = Color32::from_rgb(0x0c, 0x0c, 0x0c);
        p.rect_filled(card, 4.0, backdrop);
        p.rect_stroke(card, 4.0, egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);

        let inner = card.shrink(6.0);
        // 与终端本体同一套公式:自动缝宽 = 线宽 + 2px(文字几乎贴着线),
        // text_gap 优先,线宽自动 3px,缝至少容纳线宽 + 1px。
        let auto = 5.0f32;
        let unit_t = s.thickness.unwrap_or(3.0).max(0.0);
        let mut gap = s.gap.unwrap_or(auto);
        if let Some(tg) = s.text_gap {
            gap = 2.0 * tg.max(0.0) + unit_t;
        }
        // 与终端同一套下限:线最多贴到文字边(text_gap = 0 就是贴着线)
        let gap = gap.max(unit_t).min(inner.width().min(inner.height()) * 0.30);
        let (a, b, gap_rect) = if vertical {
            let cx = inner.center().x;
            (
                egui::Rect::from_min_max(inner.min, egui::pos2(cx - gap * 0.5, inner.max.y)),
                egui::Rect::from_min_max(egui::pos2(cx + gap * 0.5, inner.min.y), inner.max),
                egui::Rect::from_min_max(
                    egui::pos2(cx - gap * 0.5, inner.min.y),
                    egui::pos2(cx + gap * 0.5, inner.max.y),
                ),
            )
        } else {
            let cy = inner.center().y;
            (
                egui::Rect::from_min_max(inner.min, egui::pos2(inner.max.x, cy - gap * 0.5)),
                egui::Rect::from_min_max(egui::pos2(inner.min.x, cy + gap * 0.5), inner.max),
                egui::Rect::from_min_max(
                    egui::pos2(inner.min.x, cy - gap * 0.5),
                    egui::pos2(inner.max.x, cy + gap * 0.5),
                ),
            )
        };
        p.rect_filled(a, 0.0, Color32::from_rgb(0x13, 0x15, 0x1a));
        p.rect_filled(b, 0.0, Color32::from_rgb(0x1b, 0x1e, 0x25));
        p.rect_filled(gap_rect, 0.0, theme::BG);
        // 焦点反馈:左/上半 pane 视为聚焦,只有它画输入光标。
        // 关掉「非聚焦窗格隐藏光标」后,另一半才会出现一个变暗的 caret。
        let caret = |r: egui::Rect| {
            egui::Rect::from_min_size(
                egui::pos2(r.min.x + 12.0, r.min.y + 14.0),
                egui::vec2(7.0, 14.0),
            )
        };
        p.rect_filled(caret(a), 0.0, s.caret_color);
        if !s.hide_inactive_caret {
            p.rect_filled(caret(b), 0.0, Color32::from_rgb(0x55, 0x58, 0x62));
        }

        if !s.enabled || s.opacity <= 0.0 {
            return;
        }
        let t = s.thickness.unwrap_or(3.0f32.min(gap)).clamp(0.0, 64.0);
        if t <= 0.0 {
            return;
        }
        let len = if vertical { inner.height() } else { inner.width() };
        // 预览里的留白按 400px 内容区等比缩放,并留出至少 10% 的线长
        let scale = len / 400.0;
        let s0 = (s.inset_start * scale).clamp(0.0, len * 0.45);
        let s1 = (s.inset_end * scale).clamp(0.0, len * 0.45);
        let line = if vertical {
            let cx = inner.center().x;
            egui::Rect::from_min_max(
                egui::pos2(cx - t * 0.5, inner.min.y + s0),
                egui::pos2(cx + t * 0.5, inner.max.y - s1),
            )
        } else {
            let cy = inner.center().y;
            egui::Rect::from_min_max(
                egui::pos2(inner.min.x + s0, cy - t * 0.5),
                egui::pos2(inner.max.x - s1, cy + t * 0.5),
            )
        };
        if line.width() <= 0.0 || line.height() <= 0.0 {
            return;
        }

        let stops: Vec<Color32> = if s.stops.len() >= 2 {
            s.stops.clone()
        } else {
            vec![s.solid]
        };
        let along_y = match s.orientation.as_str() {
            "Vertical" => true,
            "Horizontal" => false,
            _ => vertical,
        };
        let (base, span, cross_base, cross_span) = if along_y {
            (line.min.y, line.height(), line.min.x, line.width())
        } else {
            (line.min.x, line.width(), line.min.y, line.height())
        };
        let n = if stops.len() >= 2 { 64 } else { 1 };
        for i in 0..n {
            let raw = preview_gradient_at(&stops, (i as f32 + 0.5) / n as f32);
            let c = preview_mix(theme::BG, raw, s.opacity);
            let b0 = base + span * i as f32 / n as f32;
            let b1 = base + span * (i + 1) as f32 / n as f32;
            let seg = if along_y {
                egui::Rect::from_min_max(
                    egui::pos2(cross_base, b0),
                    egui::pos2(cross_base + cross_span, b1),
                )
            } else {
                egui::Rect::from_min_max(
                    egui::pos2(b0, cross_base),
                    egui::pos2(b1, cross_base + cross_span),
                )
            };
            p.rect_filled(seg, 0.0, c);
        }
    }

    // --- 单行字段控件 -----------------------------------------------
    fn field_row(&mut self, ui: &mut egui::Ui, f: &Field) {
        let state = self.field_status(f);
        let label_color = match state {
            UiState::Set => theme::ACCENT,
            UiState::Modified => theme::WARN,
            UiState::Expr => theme::WARN,
            UiState::Default => theme::TEXT,
        };

        ui.horizontal(|ui| {
            ui.add_space(4.0);
            // 左侧标签(固定宽度)
            ui.add_sized(
                [190.0, 20.0],
                egui::Label::new(RichText::new(f.label).color(label_color).size(12.5)),
            );

            // 中间控件
            self.field_widget(ui, f);

            // 右侧徽标
            let badge = match state {
                UiState::Set => (theme::OK, "已设置"),
                UiState::Modified => (theme::WARN, "已修改"),
                UiState::Expr => (theme::WARN, "表达式"),
                UiState::Default => {
                    if f.reload != Reload::Live {
                        (theme::DIM, f.reload.label())
                    } else {
                        (theme::DIM, "")
                    }
                }
            };
            if !badge.1.is_empty() {
                ui.label(RichText::new(badge.1).color(badge.0).size(10.0));
            }
        });

        // 说明文字
        let mut desc = f.desc.to_owned();
        if !f.chapter.is_empty() {
            desc.push_str(&format!("({})", f.chapter));
        }
        if !desc.is_empty() || !f.hint.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(198.0);
                ui.label(
                    RichText::new(if !f.hint.is_empty() { f.hint } else { &desc })
                        .color(theme::DESC)
                        .size(10.5),
                );
            });
        }
    }

    fn field_widget(&mut self, ui: &mut egui::Ui, f: &Field) {
        let parts: Vec<&str> = f.parts();
        let (val, _status) = match &self.current {
            Some(c) => c.get(&parts),
            None => (None, FieldStatus::Default),
        };
        // 解析成可编辑的中间表示
        let cur_str = value_to_display(&val);

        match f.kind {
            Kind::Bool => {
                let mut checked = val
                    .as_ref()
                    .map(|v| matches!(v, Value::Bool(true)))
                    .unwrap_or(false);
                // 默认值兜底
                if matches!(val, None) {
                    checked = f.default.map(|d| d == "true").unwrap_or(false);
                }
                let mut changed = false;
                if ui.checkbox(&mut checked, "").changed() {
                    changed = true;
                }
                if changed {
                    self.set_field(f, Some(Value::Bool(checked)));
                }
                if f.allow_none {
                    self.none_reset(ui, f);
                }
            }
            Kind::Int => {
                let lo = f.lo.unwrap_or(f64::MIN) as i64;
                let hi = f.hi.unwrap_or(f64::MAX) as i64;
                // 先把显示值夹进范围再交给 DragValue。否则未设置的字段以 0 起步,
                // egui 首帧内部夹到 lo 就会回报 changed(),凭空写出一条待保存改动
                // (例:font.line_height 下限 0.6、initial_cols 下限 2)。
                let mut v = val_as_i64(&val)
                    .unwrap_or_else(|| f.default.and_then(|d| d.parse().ok()).unwrap_or(0))
                    .clamp(lo, hi);
                let mut changed = false;
                let resp = ui.add(
                    egui::DragValue::new(&mut v).range(lo..=hi).speed(1),
                );
                if resp.changed() {
                    changed = true;
                }
                if !f.unit.is_empty() {
                    ui.label(RichText::new(f.unit).color(theme::MUTED).size(11.0));
                }
                if changed {
                    self.set_field(f, Some(Value::Int(v)));
                }
                if f.allow_none {
                    self.none_reset(ui, f);
                }
            }
            Kind::Float => {
                let lo = f.lo.unwrap_or(f64::MIN);
                let hi = f.hi.unwrap_or(f64::MAX);
                let mut v = val_as_f64(&val)
                    .unwrap_or_else(|| f.default.and_then(|d| d.parse().ok()).unwrap_or(0.0))
                    .clamp(lo, hi);
                let mut changed = false;
                let resp = ui.add(
                    egui::DragValue::new(&mut v)
                        .range(lo..=hi)
                        .speed(f.step.unwrap_or(0.1)),
                );
                if resp.changed() {
                    changed = true;
                }
                if !f.unit.is_empty() {
                    ui.label(RichText::new(f.unit).color(theme::MUTED).size(11.0));
                }
                if changed {
                    self.set_field(f, Some(Value::Num(v)));
                }
                if f.allow_none {
                    self.none_reset(ui, f);
                }
            }
            Kind::Str => {
                let mut s = cur_str.clone();
                if ui.text_edit_singleline(&mut s).changed() {
                    self.set_field(f, Some(Value::Str(s)));
                }
                if f.allow_none {
                    self.none_reset(ui, f);
                }
            }
            Kind::Enum => {
                let mut selected = cur_str.clone();
                let mut changed = false;
                egui::ComboBox::from_id_salt(f.path)
                    .selected_text(option_label(f, &selected))
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in f.options {
                            if ui.selectable_label(selected == *val, *label).clicked() {
                                selected = val.to_string();
                                changed = true;
                            }
                        }
                    });
                if changed {
                    self.set_field(f, Some(Value::Str(selected)));
                }
                if f.allow_none {
                    self.none_reset(ui, f);
                }
            }
            Kind::Color => {
                self.color_widget(ui, f, &cur_str, None);
                if f.allow_none {
                    self.none_reset(ui, f);
                }
            }
            Kind::Path => {
                let mut s = cur_str.clone();
                ui.horizontal(|ui| {
                    if ui.text_edit_singleline(&mut s).changed() {
                        self.set_field(f, Some(Value::Str(s.clone())));
                    }
                    if ui.button("浏览…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_file() {
                            self.set_field(f, Some(Value::Str(p.display().to_string())));
                        }
                    }
                });
                if f.allow_none {
                    self.none_reset(ui, f);
                }
            }
            Kind::Pair => {
                self.pair_widget(ui, f);
                if f.allow_none {
                    self.none_reset(ui, f);
                }
            }
            Kind::List => {
                self.list_widget(ui, f);
                if f.allow_none {
                    self.none_reset(ui, f);
                }
            }
            Kind::Table | Kind::Code => {
                // 复杂表 / 代码:在右侧的 Lua 预览里直接改
                ui.label(
                    RichText::new("在右侧 Lua 预览里编辑")
                        .color(theme::DIM)
                        .size(11.0),
                );
                if f.allow_none && val.is_some() {
                    self.none_reset(ui, f);
                }
            }
        }
    }

    /// "恢复默认"小按钮(allow_none 的字段)。
    fn none_reset(&mut self, ui: &mut egui::Ui, f: &Field) {
        let parts: Vec<&str> = f.parts();
        let has = match &self.current {
            Some(c) => c.get(&parts).1 != FieldStatus::Default,
            None => false,
        };
        if has {
            if ui
                .add(egui::Button::new(RichText::new("↺").color(theme::MUTED)).frame(false))
                .on_hover_text(if f.none_label.is_empty() {
                    "恢复默认(未设置)".to_owned()
                } else {
                    format!("恢复默认 · {}", f.none_label)
                })
                .clicked()
            {
                self.set_field(f, None);
            }
        }
    }

    // --- 颜色控件(重点优化) ----------------------------------------
    fn color_widget(&mut self, ui: &mut egui::Ui, f: &Field, cur: &str, sub: Option<String>) {
        let special = match cur {
            "none" | "Auto" | "auto" | "off" => Some(cur.to_owned()),
            _ => None,
        };
        // 没填时直接把默认值当占位色显示 / 作为拾色器起点,
        // 省得先猜一个 HEX 再点开调色。
        let default_hex = f
            .default
            .map(|d| d.trim_matches('\"').trim_matches('\'').trim().to_owned())
            .unwrap_or_default();
        let color = parse_hex_color(cur).or_else(|| parse_hex_color(&default_hex));
        let preview_color = color.unwrap_or(theme::PANEL3);
        let picker_hex = if cur.trim().is_empty() {
            default_hex.clone()
        } else {
            cur.to_owned()
        };

        let mut open_picker = false;
        ui.horizontal(|ui| {
            // 色块
            let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 20.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 3.0, preview_color);
            ui.painter().rect_stroke(
                rect,
                3.0,
                egui::Stroke::new(1.0, theme::BORDER),
                egui::StrokeKind::Inside,
            );
            if ui.rect_contains_pointer(rect) && ui.input(|i| i.pointer.primary_clicked()) {
                open_picker = true;
            }

            // 文本(HEX 或特殊值)
            let mut s = cur.to_owned();
            let resp = ui.add(
                egui::TextEdit::singleline(&mut s)
                    .desired_width(110.0)
                    .hint_text(if default_hex.is_empty() {
                        "#RRGGBB"
                    } else {
                        default_hex.as_str()
                    }),
            );
            if resp.changed() {
                self.set_field(f, Some(Value::Str(s)));
            }
            if resp.clicked() {
                // 点击也打开拾色器
            }

            if ui
                .add(egui::Button::new("◧").frame(false))
                .on_hover_text("打开颜色选择器")
                .clicked()
            {
                open_picker = true;
            }
        });

        if open_picker {
            self.color_picker = Some(ColorPickerState {
                path: f.parts().iter().map(|s| s.to_string()).collect(),
                label: f.label.to_owned(),
                sub_key: sub,
                current: color.unwrap_or(theme::PANEL3),
                hex: picker_hex,
                alpha: false,
                special,
            });
        }
    }

    fn pair_widget(&mut self, ui: &mut egui::Ui, f: &Field) {
        let parts: Vec<&str> = f.parts();
        let (val, _) = match &self.current {
            Some(c) => c.get(&parts),
            None => (None, FieldStatus::Default),
        };
        // 提取子键的值
        let mut sub_vals: Vec<String> = Vec::new();
        if let Some(Value::Table(lua::Table::Map(m))) = &val {
            for k in f.sub {
                sub_vals.push(match m.get(*k) {
                    Some(Value::Str(s)) => s.clone(),
                    Some(v) => value_to_display(&Some(v.clone())),
                    None => String::new(),
                });
            }
        } else {
            for _ in f.sub {
                sub_vals.push(String::new());
            }
        }

        ui.vertical(|ui| {
            for (i, k) in f.sub.iter().enumerate() {
                let label = f.sub_labels.get(i).copied().unwrap_or(*k);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).color(theme::MUTED).size(11.0));
                    // 颜色子键走拾色器
                    let cur = sub_vals[i].clone();
                    let color = parse_hex_color(&cur).unwrap_or(theme::PANEL3);
                    let mut open = false;
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(20.0, 18.0), egui::Sense::click());
                    ui.painter().rect_filled(rect, 3.0, color);
                    ui.painter().rect_stroke(
                        rect,
                        3.0,
                        egui::Stroke::new(1.0, theme::BORDER),
                        egui::StrokeKind::Inside,
                    );
                    if ui.rect_contains_pointer(rect)
                        && ui.input(|inp| inp.pointer.primary_clicked())
                    {
                        open = true;
                    }
                    let mut s = cur.clone();
                    if ui.text_edit_singleline(&mut s).changed() {
                        self.set_pair_sub(f, k, &s);
                    }
                    if ui.add(egui::Button::new("◧").frame(false)).clicked() {
                        open = true;
                    }
                    if open {
                        self.color_picker = Some(ColorPickerState {
                            path: f.parts().iter().map(|s| s.to_string()).collect(),
                            label: format!("{} · {}", f.label, label),
                            sub_key: Some(k.to_string()),
                            current: color,
                            hex: cur,
                            alpha: false,
                            special: None,
                        });
                    }
                });
            }
        });
    }

    fn set_pair_sub(&mut self, f: &Field, sub_key: &str, value: &str) {
        let parts: Vec<&str> = f.parts();
        // 构建/更新 pair 表
        let (val, _) = match &self.current {
            Some(c) => c.get(&parts),
            None => (None, FieldStatus::Default),
        };
        let mut map: HashMap<String, Value> = match val {
            Some(Value::Table(lua::Table::Map(m))) => m,
            _ => {
                // 默认值兜底:从 f.default 解析
                parse_default_pair(f)
            }
        };
        map.insert(sub_key.to_string(), Value::Str(value.to_string()));
        self.set_field(f, Some(Value::Table(lua::Table::Map(map))));
    }

    fn list_widget(&mut self, ui: &mut egui::Ui, f: &Field) {
        let parts: Vec<&str> = f.parts();
        let (val, _) = match &self.current {
            Some(c) => c.get(&parts),
            None => (None, FieldStatus::Default),
        };
        let items: Vec<String> = match &val {
            Some(Value::Table(lua::Table::Array(arr))) => {
                arr.iter().map(value_to_display_ref).collect()
            }
            _ => vec![],
        };

        ui.vertical(|ui| {
            let mut new_items = items.clone();
            let mut changed = false;
            for i in 0..new_items.len() {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{}", i + 1))
                            .color(theme::DIM)
                            .size(11.0),
                    );
                    let mut s = new_items[i].clone();
                    let mut item_changed = false;
                    match f.item {
                        "color" => {
                            // 颜色列表项也走拾色器
                            let color = parse_hex_color(&s).unwrap_or(theme::PANEL3);
                            let mut open = false;
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(20.0, 18.0), egui::Sense::click());
                            ui.painter().rect_filled(rect, 3.0, color);
                            ui.painter().rect_stroke(
                                rect,
                                3.0,
                                egui::Stroke::new(1.0, theme::BORDER),
                                egui::StrokeKind::Inside,
                            );
                            if ui.rect_contains_pointer(rect)
                                && ui.input(|inp| inp.pointer.primary_clicked())
                            {
                                open = true;
                            }
                            if ui.text_edit_singleline(&mut s).changed() {
                                item_changed = true;
                            }
                            if open {
                                self.color_picker = Some(ColorPickerState {
                                    path: f.parts().iter().map(|s| s.to_string()).collect(),
                                    label: format!("{} [{}]", f.label, i + 1),
                                    sub_key: Some(i.to_string()),
                                    current: color,
                                    hex: s.clone(),
                                    alpha: false,
                                    special: None,
                                });
                            }
                        }
                        _ => {
                            if ui.text_edit_singleline(&mut s).changed() {
                                item_changed = true;
                            }
                        }
                    }
                    if item_changed {
                        new_items[i] = s;
                        changed = true;
                    }
                    if ui
                        .add(egui::Button::new("×").frame(false))
                        .on_hover_text("删除此项")
                        .clicked()
                    {
                        new_items.remove(i);
                        changed = true;
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui.small_button("+ 添加").clicked() {
                    if f.count == 0 || new_items.len() < f.count {
                        new_items.push(String::new());
                        changed = true;
                    }
                }
            });
            if changed {
                let arr: Vec<Value> = new_items
                    .into_iter()
                    .map(|s| string_item_value(f, &s))
                    .collect();
                self.set_field(f, Some(Value::Table(lua::Table::Array(arr))));
            }
        });
    }

    // --- 颜色拾色器窗口 -------------------------------------------
    fn color_picker_window(&mut self, ctx: &egui::Context) {
        let Some(picker) = &mut self.color_picker else {
            return;
        };
        let mut close = false;
        let mut commit: Option<Option<Value>> = None;

        egui::Window::new(format!("选择颜色 · {}", picker.label))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                // 预设色板(终端常用色)
                ui.label(RichText::new("预设").color(theme::MUTED).size(11.0));
                let presets: &[(&str, Color32)] = &[
                    ("#0c0c0c", Color32::from_rgb(0x0c, 0x0c, 0x0c)),
                    ("#161616", Color32::from_rgb(0x16, 0x16, 0x16)),
                    ("#262626", Color32::from_rgb(0x26, 0x26, 0x26)),
                    ("#2f2f2f", Color32::from_rgb(0x2f, 0x2f, 0x2f)),
                    ("#3a6ea5", Color32::from_rgb(0x3a, 0x6e, 0xa5)),
                    ("#a0a0a0", Color32::from_rgb(0xa0, 0xa0, 0xa0)),
                    ("#cccccc", Color32::from_rgb(0xcc, 0xcc, 0xcc)),
                    ("#ffffff", Color32::from_rgb(0xff, 0xff, 0xff)),
                    ("#ff5555", Color32::from_rgb(0xff, 0x55, 0x55)),
                    ("#f1fa8c", Color32::from_rgb(0xf1, 0xfa, 0x8c)),
                    ("#50fa7b", Color32::from_rgb(0x50, 0xfa, 0x7b)),
                    ("#8be9fd", Color32::from_rgb(0x8b, 0xe9, 0xfd)),
                    ("#bd93f9", Color32::from_rgb(0xbd, 0x93, 0xf9)),
                    ("#ff79c6", Color32::from_rgb(0xff, 0x79, 0xc6)),
                    ("#ffb86c", Color32::from_rgb(0xff, 0xb8, 0x6c)),
                    ("#6272a4", Color32::from_rgb(0x62, 0x72, 0xa4)),
                ];
                ui.horizontal_wrapped(|ui| {
                    for (hex, c) in presets {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
                        ui.painter().rect_filled(rect, 4.0, *c);
                        ui.painter().rect_stroke(
                            rect,
                            4.0,
                            egui::Stroke::new(1.0, theme::BORDER),
                            egui::StrokeKind::Inside,
                        );
                        if ui.rect_contains_pointer(rect)
                            && ui.input(|i| i.pointer.primary_clicked())
                        {
                            picker.current = *c;
                            picker.hex = hex.to_string();
                        }
                        resp_tooltip(ui, rect, hex);
                    }
                });

                ui.add_space(6.0);
                ui.label(RichText::new("自定义").color(theme::MUTED).size(11.0));
                ui.horizontal(|ui| {
                    // 大色块预览
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(48.0, 32.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 4.0, picker.current);
                    ui.painter().rect_stroke(
                        rect,
                        4.0,
                        egui::Stroke::new(1.0, theme::BORDER),
                        egui::StrokeKind::Inside,
                    );

                    ui.vertical(|ui| {
                        // RGB 滑块
                        let mut r = picker.current.r();
                        let mut g = picker.current.g();
                        let mut b = picker.current.b();
                        let mut changed = false;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("R").color(theme::ERR).size(11.0));
                            changed |= ui.add(egui::Slider::new(&mut r, 0..=255)).changed();
                        });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("G").color(theme::OK).size(11.0));
                            changed |= ui.add(egui::Slider::new(&mut g, 0..=255)).changed();
                        });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("B").color(theme::ACCENT).size(11.0));
                            changed |= ui.add(egui::Slider::new(&mut b, 0..=255)).changed();
                        });
                        if changed {
                            picker.current = Color32::from_rgb(r, g, b);
                            picker.hex = format!("#{:02x}{:02x}{:02x}", r, g, b);
                        }
                    });
                });

                ui.horizontal(|ui| {
                    ui.label(RichText::new("HEX").color(theme::MUTED).size(11.0));
                    let resp =
                        ui.add(egui::TextEdit::singleline(&mut picker.hex).desired_width(110.0));
                    if resp.changed() {
                        if let Some(c) = parse_hex_color(&picker.hex) {
                            picker.current = c;
                        }
                    }
                });

                // 特殊值
                if picker.special.is_some() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("特殊值").color(theme::MUTED).size(11.0));
                        for sp in ["none", "Auto", "off"] {
                            if ui
                                .selectable_label(picker.special.as_deref() == Some(sp), sp)
                                .clicked()
                            {
                                picker.hex = sp.to_owned();
                            }
                        }
                    });
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                    if ui
                        .add(egui::Button::new("应用").fill(theme::ACCENT))
                        .clicked()
                    {
                        commit = Some(Some(Value::Str(picker.hex.clone())));
                    }
                });
            });

        if let Some(v) = commit {
            self.apply_color_picker(v);
            close = true;
        }
        if close {
            self.color_picker = None;
        }
    }

    fn apply_color_picker(&mut self, value: Option<Value>) {
        let Some(picker) = self.color_picker.take() else {
            return;
        };
        let parts: Vec<&str> = picker.path.iter().map(|s| s.as_str()).collect();
        if let Some(sub) = &picker.sub_key {
            // 子键(pair 的 bg_color / fg_color,或 list 的序号)
            let f = schema::all_fields()
                .into_iter()
                .find(|f| f.parts() == parts)
                .cloned();
            if let Some(f) = f {
                if f.kind == Kind::Pair {
                    if let Some(Value::Str(s)) = &value {
                        self.set_pair_sub(&f, sub, s);
                    }
                } else if f.kind == Kind::List {
                    // 更新 list 第 i 项
                    let i: usize = sub.parse().unwrap_or(0);
                    if let Some(Value::Str(s)) = &value {
                        self.set_list_item(&f, i, s);
                    }
                }
            }
        } else if let Some(cur) = &mut self.current {
            cur.set(&parts, value);
            self.refresh_preview();
        }
    }

    fn set_list_item(&mut self, f: &Field, idx: usize, value: &str) {
        let parts: Vec<&str> = f.parts();
        let (val, _) = match &self.current {
            Some(c) => c.get(&parts),
            None => (None, FieldStatus::Default),
        };
        let mut items: Vec<Value> = match val {
            Some(Value::Table(lua::Table::Array(arr))) => arr,
            _ => vec![],
        };
        if idx < items.len() {
            items[idx] = Value::Str(value.to_string());
            self.set_field(f, Some(Value::Table(lua::Table::Array(items))));
        }
    }
}

// ---------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------

fn resp_tooltip(ui: &mut egui::Ui, rect: egui::Rect, text: &str) {
    if ui.rect_contains_pointer(rect) {
        ui.painter().text(
            rect.center() + egui::vec2(0.0, 18.0),
            egui::Align2::CENTER_TOP,
            text,
            egui::FontId::proportional(11.0),
            theme::TEXT,
        );
    }
}

fn value_to_display(v: &Option<Value>) -> String {
    match v {
        None => String::new(),
        Some(Value::Str(s)) => s.clone(),
        Some(Value::Int(i)) => i.to_string(),
        Some(Value::Num(f)) => format!("{}", f),
        Some(Value::Bool(b)) => (if *b { "true" } else { "false" }).to_owned(),
        Some(Value::Nil) => "nil".to_owned(),
        Some(Value::Expr(e)) => e.clone(),
        Some(Value::Table(_)) => "(表)".to_owned(),
    }
}

fn value_to_display_ref(v: &Value) -> String {
    value_to_display(&Some(v.clone()))
}

fn val_as_i64(v: &Option<Value>) -> Option<i64> {
    match v {
        Some(Value::Int(i)) => Some(*i),
        Some(Value::Num(f)) => Some(*f as i64),
        Some(Value::Str(s)) => s.parse().ok(),
        _ => None,
    }
}

fn val_as_f64(v: &Option<Value>) -> Option<f64> {
    match v {
        Some(Value::Int(i)) => Some(*i as f64),
        Some(Value::Num(f)) => Some(*f),
        Some(Value::Str(s)) => s.parse().ok(),
        _ => None,
    }
}

fn option_label(f: &Field, value: &str) -> String {
    f.options
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| value.to_owned())
}

/// 解析 `#rrggbb` / `#rgb` / `rrggbb` 形式的颜色。
fn parse_hex_color(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Color32::from_rgb(r, g, b))
    } else if s.len() == 3 {
        let r = u8::from_str_radix(&s[0..1], 16).ok()? * 17;
        let g = u8::from_str_radix(&s[1..2], 16).ok()? * 17;
        let b = u8::from_str_radix(&s[2..3], 16).ok()? * 17;
        Some(Color32::from_rgb(r, g, b))
    } else {
        None
    }
}

/// 预览用的颜色插值(与终端本体的 8bit 插值一致)。
fn preview_mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| -> u8 {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// 多个停靠点等距插值,和 config/渲染层的 gradient_at 是同一套算法。
fn preview_gradient_at(stops: &[Color32], t: f32) -> Color32 {
    if stops.is_empty() {
        return Color32::from_rgb(0x32, 0x32, 0x36);
    }
    if stops.len() == 1 {
        return stops[0];
    }
    let scaled = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let i = (scaled.floor() as usize).min(stops.len() - 2);
    preview_mix(stops[i], stops[i + 1], scaled - i as f32)
}

/// 从 pair 字段的默认值解析出子键表。
fn parse_default_pair(f: &Field) -> HashMap<String, Value> {
    let mut map = HashMap::new();
    if let Some(default) = f.default {
        // 形如 `{ bg_color = "#2f2f2f", fg_color = "#ffffff" }`
        for sub in f.sub {
            if let Some(pos) = default.find(sub) {
                let after = &default[pos + sub.len()..];
                if let Some(eq) = after.find('=') {
                    let rest = after[eq + 1..].trim_start();
                    if let Some(q) = rest.find('"') {
                        let inner = &rest[q + 1..];
                        if let Some(q2) = inner.find('"') {
                            map.insert(sub.to_string(), Value::Str(inner[..q2].to_owned()));
                        }
                    }
                }
            }
        }
    }
    map
}

fn string_item_value(f: &Field, s: &str) -> Value {
    match f.item {
        "color" | "str" | "path" => Value::Str(s.to_owned()),
        _ => Value::Str(s.to_owned()),
    }
}

/// 简单的 Lua 语法着色(注释/字符串/关键字/数字)。
fn color_lua(code: &str, job: &mut egui::text::LayoutJob, _style: egui::TextStyle) {
    let comment = theme::DIM;
    let string = Color32::from_rgb(0x9e, 0xce, 0x6a);
    let keyword = Color32::from_rgb(0xbb, 0x9a, 0xf7);
    let number = Color32::from_rgb(0xe0, 0xaf, 0x68);
    let text = theme::TEXT;

    let keywords = [
        "return", "local", "true", "false", "nil", "and", "or", "not", "function", "end", "if",
        "then", "else", "elseif", "for", "while", "do", "in", "repeat", "until", "break", "goto",
    ];

    let mut i = 0;
    let chars: Vec<char> = code.chars().collect();
    while i < chars.len() {
        // 注释
        if i + 1 < chars.len() && chars[i] == '-' && chars[i + 1] == '-' {
            let mut j = i;
            while j < chars.len() && chars[j] != '\n' {
                j += 1;
            }
            job.append(
                &chars[i..j].iter().collect::<String>(),
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::new(13.0, egui::FontFamily::Monospace),
                    color: comment,
                    ..Default::default()
                },
            );
            i = j;
            continue;
        }
        // 字符串
        if chars[i] == '"' || chars[i] == '\'' {
            let quote = chars[i];
            let mut j = i + 1;
            while j < chars.len() && chars[j] != quote {
                if chars[j] == '\\' {
                    j += 1;
                }
                j += 1;
            }
            if j < chars.len() {
                j += 1;
            }
            job.append(
                &chars[i..j].iter().collect::<String>(),
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::new(13.0, egui::FontFamily::Monospace),
                    color: string,
                    ..Default::default()
                },
            );
            i = j;
            continue;
        }
        // 数字
        if chars[i].is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                j += 1;
            }
            job.append(
                &chars[i..j].iter().collect::<String>(),
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::new(13.0, egui::FontFamily::Monospace),
                    color: number,
                    ..Default::default()
                },
            );
            i = j;
            continue;
        }
        // 关键字 / 标识符
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let color = if keywords.contains(&word.as_str()) {
                keyword
            } else {
                text
            };
            job.append(
                &word,
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::new(13.0, egui::FontFamily::Monospace),
                    color,
                    ..Default::default()
                },
            );
            i = j;
            continue;
        }
        // 普通字符
        let j = i + 1;
        job.append(
            &chars[i..j].iter().collect::<String>(),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::new(13.0, egui::FontFamily::Monospace),
                color: text,
                ..Default::default()
            },
        );
        i = j;
    }
}
