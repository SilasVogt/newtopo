//! Application state and the per-frame orchestration: top bar, dock,
//! status bar, overlays, input routing and command dispatch.

use crate::commands::Cmd;
use crate::docstate::{ApplyError, DocState};
use crate::icons::{self, Icon};
use crate::keymap::{Binding, Dispatcher, Keymap};
use crate::prefs::{Paths, Prefs};
use crate::selection::{Mode, Selection};
use crate::strings as S;
use crate::theme::{self, Palette, Theme};
use crate::toasts::{ToastKind, Toasts};
use crate::tools::{Tool, ToolState};
use crate::viewdata::{DisplayParams, ViewData};
use egui::{Color32, Key, Modifiers, RichText, Ui};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use newtopo_core::doc::LayerId;
use newtopo_core::ops::Op;
use newtopo_kernel::BodyId;
use newtopo_render::synth::SynthBody;
use newtopo_render::{OrbitCamera, OverlayColors, RenderScene};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    Outliner,
    Viewport,
    Tool,
    Stats,
    History,
    Rename,
    Bridge,
    Uv,
}

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Outliner => S::TAB_OUTLINER,
            Tab::Viewport => S::TAB_VIEWPORT,
            Tab::Tool => S::TAB_TOOL,
            Tab::Stats => S::TAB_STATS,
            Tab::History => S::TAB_HISTORY,
            Tab::Rename => S::TAB_RENAME,
            Tab::Bridge => S::TAB_BRIDGE,
            Tab::Uv => S::TAB_UV,
        }
    }
}

pub fn default_layout() -> DockState<Tab> {
    let mut d = DockState::new(vec![Tab::Viewport]);
    let s = d.main_surface_mut();
    let [center, _left] = s.split_left(NodeIndex::root(), 0.17, vec![Tab::Outliner]);
    let [_center, right] = s.split_right(center, 0.75, vec![Tab::Tool]);
    s.split_below(right, 0.5, vec![Tab::Stats, Tab::History]);
    d
}

/// The synthetic sample model shown when there is no scene.
pub struct Demo {
    pub bodies: Vec<SynthBody>,
    pub layers: Vec<(String, LayerId)>,
}

impl Demo {
    pub fn new() -> Self {
        let bodies = newtopo_render::synth::demo_bodies();
        let mut layers: Vec<(String, LayerId)> = Vec::new();
        for b in &bodies {
            if !layers.iter().any(|(n, _)| *n == b.layer) {
                let id = LayerId(layers.len() as u32);
                layers.push((b.layer.clone(), id));
            }
        }
        Demo { bodies, layers }
    }
}

#[derive(Clone, Debug)]
pub struct Isolate {
    pub original: Vec<BodyId>,
    /// None = all of `original`; Some(i) = only `original[i]`.
    pub step: Option<usize>,
}

/// Viewport-related state.
pub struct ViewState {
    pub camera: OrbitCamera,
    pub preview: bool,
    pub wireframe: bool,
    pub flat: bool,
    pub hidden: BTreeSet<BodyId>,
    pub isolate: Option<Isolate>,
    pub visible_only: bool,
    pub needs_frame: bool,
    pub rect: egui::Rect,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            camera: OrbitCamera::default(),
            preview: false,
            wireframe: true,
            flat: false,
            hidden: BTreeSet::new(),
            isolate: None,
            visible_only: false,
            needs_frame: true,
            rect: egui::Rect::NOTHING,
        }
    }
}

/// App-side layer/lock state the facade has no ops for yet.
#[derive(Default)]
pub struct LocalOverrides {
    pub layer_hidden: BTreeSet<LayerId>,
    pub layer_locked: BTreeSet<LayerId>,
    pub body_locked: BTreeSet<BodyId>,
    pub body_hidden_demo: BTreeSet<BodyId>,
}

#[derive(Clone, Debug)]
pub enum DockAction {
    Toggle(Tab),
    Focus(Tab),
    ResetLayout,
    HidePanels(bool),
}

/// A pending action that needs the scene saved or discarded first.
#[derive(Clone, Debug)]
pub enum PendingAfterPrompt {
    New,
    Open(Option<PathBuf>),
    Quit,
}

#[derive(Clone, Debug)]
pub struct Confirm {
    pub title: String,
    pub body: String,
    pub op: Op,
}

#[derive(Default)]
pub struct UiState {
    pub palette: crate::palette::PaletteState,
    pub radial: crate::palette::RadialState,
    pub key_capture: Option<crate::palette::KeyCapture>,
    pub prefs_open: bool,
    pub prefs_tab: crate::prefs_ui::PrefsTab,
    pub keymap_filter: String,
    pub confirm: Option<Confirm>,
    pub unsaved_prompt: Option<PendingAfterPrompt>,
    pub recovery: Vec<PathBuf>,
    pub about_open: bool,
    pub dock_actions: Vec<DockAction>,
    pub panels_hidden: bool,
    pub focus_outliner_search: bool,
    pub outliner: crate::panels::OutlinerState,
    pub rename: crate::studio::RenameState,
    pub uv: crate::studio::UvOptions,
    pub bridge: crate::studio::BridgeUi,
    pub export: Option<crate::jobs::ExportDialog>,
    pub import_list: Option<crate::jobs::ImportList>,
    pub import_report: Option<crate::jobs::ImportReport>,
    pub warned_engine: bool,
}

pub struct AppState {
    pub paths: Paths,
    pub prefs: Prefs,
    pub keymap: Keymap,
    pub themes: Vec<Theme>,
    pub theme: Theme,
    pub pal: Palette,
    pub theme_applied: Option<(String, u32)>,
    pub doc: DocState,
    pub demo: Option<Demo>,
    pub view: ViewState,
    pub sel: Selection,
    pub mode: Mode,
    pub tool: ToolState,
    pub ui: UiState,
    pub toasts: Toasts,
    pub dispatcher: Dispatcher,
    pub local: LocalOverrides,
    pub vd: ViewData,
    vd_key: Option<(u64, DisplayParams, bool)>,
    pub vd_rev: u64,
    pub overlay: Arc<RenderScene>,
    pub overlay_sig: u64,
    pub overlay_rev: u64,
    pub hover: Option<crate::viewport::Hover>,
    pub ui_pointer: Option<egui::Pos2>,
    /// Selection before the last key command (double-press support).
    pub sel_before_key: Option<Selection>,
    pub drag: Option<crate::viewport::DragState>,
    pub jobs: crate::jobs::Jobs,
    pub time: f64,
    pub next_autosave: f64,
    pub last_title: String,
    pub dirty_settings: bool,
    pub quit: bool,
    pub screenshot_mode: bool,
    pub gpu_ok: bool,
}

pub struct App {
    pub st: AppState,
    pub dock: DockState<Tab>,
    pub dock_saved: Option<DockState<Tab>>,
}

impl App {
    pub fn new(ctx: &egui::Context, paths: Paths, gpu_ok: bool, screenshot_mode: bool) -> Self {
        theme::install_fonts(ctx);
        let prefs = if screenshot_mode {
            Prefs::default()
        } else {
            Prefs::load(&paths)
        };
        let keymap = if screenshot_mode {
            Keymap::default()
        } else {
            crate::prefs::load_keymap(&paths)
        };
        let themes = Theme::available(paths.themes_dir().as_deref());
        let theme = themes
            .iter()
            .find(|t| t.name == prefs.theme)
            .cloned()
            .unwrap_or_else(Theme::builtin_dark);
        let pal = theme.palette();
        let dock = if screenshot_mode {
            default_layout()
        } else {
            crate::prefs::load_json::<Option<DockState<Tab>>>(paths.file("layout.json").as_deref())
                .filter(|d| d.find_tab(&Tab::Viewport).is_some())
                .unwrap_or_else(default_layout)
        };
        let recovery = if screenshot_mode {
            vec![]
        } else {
            crate::jobs::find_recoverable(&paths, &prefs)
        };
        let mut st = AppState {
            paths,
            prefs,
            keymap,
            themes,
            theme,
            pal,
            theme_applied: None,
            doc: DocState::default(),
            demo: None,
            view: ViewState::default(),
            sel: Selection::default(),
            mode: Mode::Object,
            tool: ToolState::default(),
            ui: UiState::default(),
            toasts: Toasts::default(),
            dispatcher: Dispatcher::default(),
            local: LocalOverrides::default(),
            vd: ViewData::default(),
            vd_key: None,
            vd_rev: newtopo_render::egui_viewport::next_revision(),
            overlay: Arc::new(RenderScene::default()),
            overlay_sig: 0,
            overlay_rev: newtopo_render::egui_viewport::next_revision(),
            hover: None,
            ui_pointer: None,
            sel_before_key: None,
            drag: None,
            jobs: Default::default(),
            time: 0.0,
            next_autosave: 0.0,
            last_title: String::new(),
            dirty_settings: false,
            quit: false,
            screenshot_mode,
            gpu_ok,
        };
        st.ui.recovery = recovery;
        st.view.camera.up = st.prefs.up;
        App {
            st,
            dock,
            dock_saved: None,
        }
    }

    /// One frame. `ui` is the root ui of the window.
    pub fn frame(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let st = &mut self.st;
        st.time = ctx.input(|i| i.time);
        // Closing the window with unsaved work asks first.
        if ctx.input(|i| i.viewport().close_requested())
            && st.doc.is_dirty()
            && !st.quit
            && !st.screenshot_mode
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if st.ui.unsaved_prompt.is_none() {
                st.request(PendingAfterPrompt::Quit);
            }
        }
        st.ensure_theme(&ctx);
        st.poll_jobs(&ctx);
        st.handle_dropped_files(&ctx);
        st.handle_input(&ctx);

        st.top_bar(ui);
        st.status_bar(ui);

        let dstyle = theme::dock_style(ui.style(), &st.pal);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(st.pal.bg)
                    .inner_margin(egui::Margin {
                        left: 4,
                        right: 4,
                        top: 2,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| {
                DockArea::new(&mut self.dock)
                    .style(dstyle)
                    .show_add_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_inside(ui, &mut Viewer { st: &mut self.st });
            });

        let st = &mut self.st;
        st.overlays(&ctx);
        st.toasts.show(&ctx, &st.pal, st.time);

        // Dock actions queued during the frame.
        let actions = std::mem::take(&mut st.ui.dock_actions);
        for a in actions {
            match a {
                DockAction::Toggle(tab) => match self.dock.find_tab(&tab) {
                    Some(path) => {
                        self.dock.remove_tab(path);
                    }
                    None => open_tab(&mut self.dock, tab),
                },
                DockAction::Focus(tab) => match self.dock.find_tab(&tab) {
                    Some(path) => {
                        let _ = self.dock.set_active_tab(path);
                    }
                    None => open_tab(&mut self.dock, tab),
                },
                DockAction::ResetLayout => {
                    self.dock = default_layout();
                    self.dock_saved = None;
                    self.st.ui.panels_hidden = false;
                }
                DockAction::HidePanels(hide) => {
                    if hide && self.dock_saved.is_none() {
                        self.dock_saved = Some(self.dock.clone());
                        self.dock
                            .main_surface_mut()
                            .retain_tabs(|t| *t == Tab::Viewport);
                    } else if !hide && let Some(saved) = self.dock_saved.take() {
                        self.dock = saved;
                    }
                    self.st.ui.panels_hidden = hide;
                }
            }
            self.st.dirty_settings = true;
        }

        let st = &mut self.st;
        st.autosave_tick();
        // A wheel gesture ends after 400 ms without ticks (SPEC §4.7).
        if let Some(t) = st.tool.wheel_last {
            if st.time - t > crate::tools::COALESCE_SECS {
                st.doc.end_gesture();
                st.tool.wheel_last = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
        let title = format!(
            "{}{} — {}",
            st.doc.title(),
            if st.doc.is_dirty() || (st.doc.has_scene() && st.doc.path.is_none()) {
                " •"
            } else {
                ""
            },
            S::APP_NAME
        );
        if title != st.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            st.last_title = title;
        }
        if st.dirty_settings && !st.screenshot_mode && !ctx.input(|i| i.pointer.any_down()) {
            st.prefs.save(&st.paths);
            crate::prefs::save_keymap(&st.paths, &st.keymap);
            let layout = self.dock_saved.as_ref().unwrap_or(&self.dock);
            crate::prefs::save_json(
                st.paths.file("layout.json").as_deref(),
                &Some(layout.clone()),
            );
            st.dirty_settings = false;
        }
        if st.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    pub fn on_exit(&mut self) {
        let st = &mut self.st;
        if st.screenshot_mode {
            return;
        }
        st.prefs.save(&st.paths);
        crate::prefs::save_keymap(&st.paths, &st.keymap);
        let layout = self.dock_saved.as_ref().unwrap_or(&self.dock);
        crate::prefs::save_json(
            st.paths.file("layout.json").as_deref(),
            &Some(layout.clone()),
        );
        // Keep the autosave if work is unsaved (the window can close
        // without the prompt when the OS kills it).
        if !st.doc.is_dirty() {
            crate::jobs::remove_autosaves(st);
        }
    }
}

fn open_tab(dock: &mut DockState<Tab>, tab: Tab) {
    // Open studio panels next to the tool panel when it exists.
    if let Some((surface, node, _)) = dock
        .find_tab(&Tab::Tool)
        .map(|p| (p.surface, p.node, p.tab))
        && let Ok(leaf) = dock.leaf_mut(egui_dock::NodePath { surface, node })
    {
        leaf.append_tab(tab);
        let n = leaf.tabs.len();
        let _ = leaf.set_active_tab(egui_dock::TabIndex(n - 1));
        return;
    }
    dock.push_to_focused_leaf(tab);
}

struct Viewer<'a> {
    st: &'a mut AppState,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("tab", *tab))
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        let t = match tab {
            Tab::Tool => self.st.tool.active.title().to_string(),
            t => t.title().to_string(),
        };
        t.into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        match tab {
            Tab::Viewport => self.st.viewport_ui(ui),
            Tab::Outliner => self.st.outliner_ui(ui),
            Tab::Tool => self.st.tool_panel_ui(ui),
            Tab::Stats => self.st.stats_ui(ui),
            Tab::History => self.st.history_ui(ui),
            Tab::Rename => self.st.rename_ui(ui),
            Tab::Bridge => self.st.bridge_ui(ui),
            Tab::Uv => self.st.uv_ui(ui),
        }
    }

    fn tab_style_override(
        &self,
        tab: &Tab,
        global: &egui_dock::TabStyle,
    ) -> Option<egui_dock::TabStyle> {
        (*tab == Tab::Viewport).then(|| {
            let mut s = global.clone();
            s.tab_body.inner_margin = egui::Margin::ZERO;
            s
        })
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        !matches!(tab, Tab::Viewport)
    }

    fn clear_background(&self, tab: &Tab) -> bool {
        !matches!(tab, Tab::Viewport)
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        match tab {
            Tab::Viewport | Tab::Outliner => [false, false],
            _ => [false, true],
        }
    }
}

impl AppState {
    pub fn colors(&self) -> OverlayColors {
        self.prefs
            .overlay
            .clone()
            .unwrap_or_else(|| self.theme.overlay.clone())
    }

    fn ensure_theme(&mut self, ctx: &egui::Context) {
        let key = (
            self.theme.name.clone(),
            (self.prefs.ui_scale * 100.0) as u32,
        );
        if self.theme_applied.as_ref() != Some(&key) {
            theme::apply(ctx, &self.theme, self.prefs.ui_scale);
            self.pal = self.theme.palette();
            self.theme_applied = Some(key);
            self.invalidate_view();
        }
    }

    pub fn set_theme(&mut self, name: &str) {
        if let Some(t) = self.themes.iter().find(|t| t.name == name).cloned() {
            self.theme = t;
            self.prefs.theme = name.to_string();
            self.dirty_settings = true;
        }
    }

    pub fn invalidate_view(&mut self) {
        self.vd_key = None;
    }

    pub fn has_content(&self) -> bool {
        self.doc.has_scene() || self.demo.is_some()
    }

    pub fn is_demo(&self) -> bool {
        !self.doc.has_scene() && self.demo.is_some()
    }

    pub fn toast(&mut self, kind: ToastKind, msg: impl Into<String>) {
        self.toasts.push(kind, msg, self.time);
    }

    /// Bodies hidden by the eye toggles, layers, or isolate.
    pub fn effective_hidden(&self) -> BTreeSet<BodyId> {
        let mut h = self.view.hidden.clone();
        h.extend(self.local.body_hidden_demo.iter().copied());
        for b in self.body_rows() {
            if self.local.layer_hidden.contains(&b.layer) {
                h.insert(b.id);
            }
        }
        if let Some(iso) = &self.view.isolate {
            let keep: BTreeSet<BodyId> = match iso.step {
                None => iso.original.iter().copied().collect(),
                Some(i) => iso.original.get(i).copied().into_iter().collect(),
            };
            for b in self.body_rows() {
                if !keep.contains(&b.id) {
                    h.insert(b.id);
                }
            }
        }
        h
    }

    pub fn is_locked(&self, body: BodyId) -> bool {
        self.local.body_locked.contains(&body)
            || self
                .body_rows()
                .iter()
                .any(|b| b.id == body && self.local.layer_locked.contains(&b.layer))
    }

    /// Rebuild view data if the document or display settings changed.
    pub fn ensure_view_data(&mut self) {
        let params = DisplayParams {
            preview: self.view.preview,
            wireframe: self.view.wireframe,
            show_verts: self.mode == Mode::Vertex && !self.view.preview,
            show_grid: self.prefs.show_grid,
            up: self.prefs.up,
            hidden: self.effective_hidden(),
            colors: self.colors(),
        };
        let key = (self.doc.revision, params.clone(), self.demo.is_some());
        if self.vd_key.as_ref() == Some(&key) {
            return;
        }
        self.vd = if let Some(scene) = self.doc.scene.as_mut() {
            crate::viewdata::from_scene(scene, &params)
        } else if let Some(demo) = &self.demo {
            crate::viewdata::from_demo(&demo.bodies, &params)
        } else {
            ViewData::default()
        };
        if let Some(e) = self.vd.error.take()
            && !self.ui.warned_engine
        {
            self.ui.warned_engine = true;
            self.toast(ToastKind::Warn, e);
        }
        self.vd_key = Some(key);
        self.vd_rev = newtopo_render::egui_viewport::next_revision();
        // Hover and weld-drag hold indices into the old tables.
        self.hover = None;
        if matches!(self.drag, Some(crate::viewport::DragState::Weld { .. })) {
            self.drag = None;
        }
        self.overlay_sig = 0;
    }

    // ------------------------------------------------------------ ops

    /// Apply an op; refusals become toasts or a confirmation dialog.
    pub fn apply(&mut self, op: Op) -> Option<newtopo_core::ops::OpOutcome> {
        self.apply_ex(op, false)
    }

    /// Apply as part of the open wheel gesture (one undo step per gesture).
    pub fn apply_coalesced(&mut self, op: Op) -> Option<newtopo_core::ops::OpOutcome> {
        self.apply_ex(op, true)
    }

    /// Apply several ops as one undo step. Returns how many were accepted.
    pub fn apply_group(&mut self, ops: Vec<Op>) -> (usize, u32) {
        if self.is_demo() {
            self.toast(ToastKind::Info, S::TOAST_DEMO_READONLY);
            return (0, 0);
        }
        self.doc.begin_group();
        let mut n = 0;
        let mut lines = 0;
        for op in ops {
            if let Some(o) = self.apply(op) {
                n += 1;
                lines += o.lines_added;
            }
        }
        self.doc.end_gesture();
        (n, lines)
    }

    fn apply_ex(&mut self, op: Op, coalesced: bool) -> Option<newtopo_core::ops::OpOutcome> {
        if self.is_demo() {
            self.toast(ToastKind::Info, S::TOAST_DEMO_READONLY);
            return None;
        }
        let r = if coalesced {
            self.doc.apply_coalesced(op.clone())
        } else {
            self.doc.apply(op.clone())
        };
        match r {
            Ok(o) => {
                if let Some(m) = &o.message {
                    self.toast(ToastKind::Info, m.clone());
                }
                Some(o)
            }
            Err(ApplyError::Refused(r)) if r.needs_confirmation => {
                self.ui.confirm = Some(Confirm {
                    title: S::DLG_CONFIRM.into(),
                    body: r.reason.clone(),
                    op,
                });
                None
            }
            Err(e) => {
                let kind = if matches!(e, ApplyError::Engine(_)) {
                    ToastKind::Error
                } else {
                    ToastKind::Warn
                };
                self.toast(kind, e.message());
                None
            }
        }
    }

    pub fn undo(&mut self) {
        match self.doc.undo() {
            Ok(_) => {}
            Err(e) => self.toast(ToastKind::Error, e),
        }
    }

    pub fn redo(&mut self) {
        match self.doc.redo() {
            Ok(_) => {}
            Err(e) => self.toast(ToastKind::Error, e),
        }
    }

    pub fn set_mode(&mut self, m: Mode) {
        if self.mode != m {
            self.mode = m;
            self.hover = None;
        }
    }

    // ------------------------------------------------------------ commands

    pub fn run(&mut self, cmd: Cmd) {
        use Cmd::*;
        match cmd {
            ModeVertex => self.set_mode(Mode::Vertex),
            ModeEdge => self.set_mode(Mode::Edge),
            ModeFace => self.set_mode(Mode::Face),
            ModeObject => self.set_mode(Mode::Object),
            Palette => self.ui.palette.open_fresh(),
            Radial => {
                let c = self.view.rect.center();
                self.ui.radial.open_at(c, self.time, true);
            }
            Preferences => self.ui.prefs_open = !self.ui.prefs_open,
            KeymapEditor => {
                self.ui.prefs_open = true;
                self.ui.prefs_tab = crate::prefs_ui::PrefsTab::Keymap;
            }
            Cmd::Preview => self.view.preview = !self.view.preview,
            Wireframe => {
                if !self.view.preview {
                    self.view.preview = true;
                    self.view.wireframe = true;
                } else {
                    self.view.wireframe = !self.view.wireframe;
                }
            }
            FlatShading => self.view.flat = !self.view.flat,
            AutoFill => self.toggle_setting(SettingToggle::AutoFill),
            Link => self.toggle_setting(SettingToggle::Link),
            Symmetry => self.toggle_setting(SettingToggle::Symmetry),
            MirrorX => self.toggle_mirror(newtopo_core::doc::Axis::X),
            MirrorY => self.toggle_mirror(newtopo_core::doc::Axis::Y),
            MirrorZ => self.toggle_mirror(newtopo_core::doc::Axis::Z),
            SymmetryPlanes => {
                self.prefs.show_symmetry_planes = !self.prefs.show_symmetry_planes;
                self.dirty_settings = true;
            }
            Connect => self.activate_tool(Tool::Connect),
            Cmd::Grid => self.activate_tool(Tool::Grid),
            Slice => self.activate_tool(Tool::Slice),
            Iso => self.activate_tool(Tool::Iso),
            Cut => self.activate_tool(Tool::Cut),
            Weld => self.activate_tool(Tool::Weld),
            Collapse => self.collapse_selected(),
            Remove => self.remove_selected(),
            Snap => {
                self.tool.snap = !self.tool.snap;
                let msg = if self.tool.snap {
                    "Snap on"
                } else {
                    "Snap off"
                };
                self.toast(ToastKind::Info, msg);
            }
            Loop => self.select_loop(false, false),
            LoopAlternate => self.select_loop(false, true),
            Ring => self.select_loop(true, false),
            RingAlternate => self.select_loop(true, true),
            SelectAll => self.select_all(),
            SelectNone => self.sel.clear_mode(self.mode),
            Invert => self.invert_selection(),
            VisibleOnly => {
                self.view.visible_only = !self.view.visible_only;
                let msg = if self.view.visible_only {
                    "Visible-only selection on"
                } else {
                    "Visible-only selection off"
                };
                self.toast(ToastKind::Info, msg);
            }
            Frame => self.frame_selection(),
            Ortho => self.view.camera.ortho = !self.view.camera.ortho,
            Hide => self.hide_selected(),
            UnhideAll => {
                self.view.hidden.clear();
                self.local.body_hidden_demo.clear();
                self.local.layer_hidden.clear();
            }
            Isolate => self.isolate_step(),
            ExitIsolate => self.view.isolate = None,
            ViewFront => self.view.camera.set_view(newtopo_render::NamedView::Front),
            ViewBack => self.view.camera.set_view(newtopo_render::NamedView::Back),
            ViewLeft => self.view.camera.set_view(newtopo_render::NamedView::Left),
            ViewRight => self.view.camera.set_view(newtopo_render::NamedView::Right),
            ViewTop => self.view.camera.set_view(newtopo_render::NamedView::Top),
            ViewBottom => self.view.camera.set_view(newtopo_render::NamedView::Bottom),
            ViewIso => self.view.camera.set_view(newtopo_render::NamedView::Iso),
            HidePanels => self
                .ui
                .dock_actions
                .push(DockAction::HidePanels(!self.ui.panels_hidden)),
            ResetLayout => self.ui.dock_actions.push(DockAction::ResetLayout),
            OutlinerSearch => {
                self.ui.focus_outliner_search = true;
                self.ui.dock_actions.push(DockAction::Focus(Tab::Outliner));
            }
            CopyRules => self.copy_rules(),
            PasteRules => self.paste_rules(),
            Undo => self.undo(),
            Redo => self.redo(),
            CheckMesh => self.check_mesh(),
            PurgeDormant => {
                self.apply(Op::PurgeDormant);
            }
            RenamePanel => self.ui.dock_actions.push(DockAction::Toggle(Tab::Rename)),
            BridgePanel => self.ui.dock_actions.push(DockAction::Toggle(Tab::Bridge)),
            UvPanel => self.ui.dock_actions.push(DockAction::Toggle(Tab::Uv)),
            New => self.request(PendingAfterPrompt::New),
            Open => self.request(PendingAfterPrompt::Open(None)),
            Save => self.save(false),
            SaveAs => self.save(true),
            Import => self.import_dialog(),
            Export => self.open_export_dialog(),
            LoadSample => self.load_sample(),
            Quit => self.request(PendingAfterPrompt::Quit),
            About => self.ui.about_open = true,
        }
    }

    pub fn load_sample(&mut self) {
        if self.doc.has_scene() {
            self.toast(
                ToastKind::Info,
                "Close the current scene to view the sample.",
            );
            return;
        }
        self.demo = Some(Demo::new());
        self.sel = Selection::default();
        self.view.needs_frame = true;
        self.invalidate_view();
    }

    fn toggle_setting(&mut self, which: SettingToggle) {
        let Some(settings) = self
            .doc
            .scene
            .as_ref()
            .and_then(|s| crate::docstate::guard(|| s.doc().settings.clone()).ok())
        else {
            if self.is_demo() {
                self.toast(ToastKind::Info, S::TOAST_DEMO_READONLY);
            } else {
                self.toast(ToastKind::Info, S::TOAST_NO_SCENE);
            }
            return;
        };
        let op = match which {
            SettingToggle::AutoFill => Op::SetAutofill {
                on: !settings.autofill,
            },
            SettingToggle::Link => Op::SetLink { on: !settings.link },
            SettingToggle::Symmetry => Op::SetSymmetry {
                on: !settings.symmetry,
                axes: if settings.symmetry_axes.is_empty() {
                    vec![newtopo_core::doc::Axis::X]
                } else {
                    settings.symmetry_axes.clone()
                },
            },
        };
        self.apply(op);
    }

    fn toggle_mirror(&mut self, axis: newtopo_core::doc::Axis) {
        let Some(settings) = self
            .doc
            .scene
            .as_ref()
            .and_then(|s| crate::docstate::guard(|| s.doc().settings.clone()).ok())
        else {
            self.toast(ToastKind::Info, S::TOAST_NO_SCENE);
            return;
        };
        let mut axes = settings.symmetry_axes.clone();
        if let Some(i) = axes.iter().position(|a| *a == axis) {
            axes.remove(i);
        } else {
            axes.push(axis);
        }
        self.apply(Op::SetSymmetry {
            on: settings.symmetry || !axes.is_empty(),
            axes,
        });
    }

    /// Current scene toggles (auto-fill, link, symmetry, axes) for the top bar.
    pub fn scene_toggles(&self) -> Option<newtopo_core::doc::SceneSettings> {
        self.doc
            .scene
            .as_ref()
            .and_then(|s| crate::docstate::guard(|| s.doc().settings.clone()).ok())
    }

    /// Validate every body's output (SPEC §4.6 step 8) and report.
    fn check_mesh(&mut self) {
        let Some(scene) = self.doc.scene.as_mut() else {
            self.toast(ToastKind::Info, S::TOAST_NO_SCENE);
            return;
        };
        match crate::docstate::guard(|| scene.validate_all()) {
            Ok(v) if v.is_empty() => self.toast(
                ToastKind::Info,
                "Output mesh checks out: no problems found.",
            ),
            Ok(v) => {
                let mut kinds: Vec<&str> = v
                    .iter()
                    .map(|(_, i)| newtopo_core::scene::issue_reason(i))
                    .collect();
                kinds.dedup();
                self.toast(
                    ToastKind::Warn,
                    format!("{} problem(s): {}", v.len(), kinds.join("; ")),
                );
            }
            Err(e) => self.toast(ToastKind::Error, e),
        }
    }

    fn copy_rules(&mut self) {
        // SPEC §6.13: copy split lines, rules and edits of the selected object.
        // The facade has no copy/paste op yet (see docs/notes/app.md).
        if self.sel.objects.len() != 1 {
            self.toast(
                ToastKind::Info,
                "Select exactly one object to copy its rules.",
            );
            return;
        }
        self.toast(ToastKind::Warn, format!("{}: copy rules", S::TOAST_NOT_YET));
    }

    fn paste_rules(&mut self) {
        self.toast(
            ToastKind::Warn,
            format!("{}: paste rules", S::TOAST_NOT_YET),
        );
    }

    pub fn request(&mut self, what: PendingAfterPrompt) {
        if self.doc.is_dirty() {
            self.ui.unsaved_prompt = Some(what);
        } else {
            self.do_pending(what);
        }
    }

    pub fn do_pending(&mut self, what: PendingAfterPrompt) {
        match what {
            PendingAfterPrompt::New => {
                self.doc.close();
                self.demo = None;
                self.sel = Selection::default();
                self.tool = ToolState::default();
                self.view.isolate = None;
                self.view.hidden.clear();
            }
            PendingAfterPrompt::Open(path) => self.open_scene(path),
            PendingAfterPrompt::Quit => self.quit = true,
        }
    }

    // ------------------------------------------------------------ input

    fn handle_input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let typing = ctx.egui_wants_keyboard_input();
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        for ev in events {
            // Key capture for the keymap editor swallows everything.
            if self.ui.key_capture.is_some() {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } = ev
                {
                    self.capture_key(key, modifiers);
                }
                continue;
            }
            let (key, pressed, repeat, mods) = match ev {
                egui::Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } => (key, pressed, repeat, modifiers),
                egui::Event::Copy if !typing => (Key::C, true, false, Modifiers::COMMAND),
                egui::Event::Paste(_) if !typing => (Key::V, true, false, Modifiers::COMMAND),
                _ => continue,
            };
            // Radial menu: hold to show, release over a slot to run.
            let radial_binds = self.keymap.bindings(Cmd::Radial);
            let is_radial_key = radial_binds
                .iter()
                .any(|b| !b.double && b.same_combo(&Binding::new(key, mods)));
            if !pressed {
                if self.ui.radial.open
                    && self.ui.radial.held
                    && radial_binds.iter().any(|b| b.key == key)
                {
                    self.radial_release();
                }
                continue;
            }
            if repeat {
                continue;
            }
            if key == Key::Escape {
                self.escape();
                continue;
            }
            if typing || self.ui.palette.open {
                continue;
            }
            if is_radial_key && !self.ui.radial.open {
                let at = pointer
                    .filter(|p| self.view.rect.contains(*p))
                    .unwrap_or(self.view.rect.center());
                self.ui.radial.open_at(at, self.time, false);
                continue;
            }
            if self.tool_key(key, mods) {
                continue;
            }
            let b = Binding::new(key, mods);
            if let Some((cmd, double)) = self.dispatcher.press_ex(&self.keymap, b, self.time) {
                if double {
                    // The first press already ran the single command: undo
                    // its selection change so the double command starts fresh.
                    if let Some(prev) = self.sel_before_key.take() {
                        self.sel = prev;
                    }
                } else {
                    self.sel_before_key = Some(self.sel.clone());
                }
                self.run(cmd);
            }
        }
    }

    /// Esc precedence (SPEC §5.8): overlays, then the tool's pending
    /// action, then the tool, then isolate.
    pub fn escape(&mut self) {
        if self.ui.palette.open {
            self.ui.palette.open = false;
        } else if self.ui.radial.open {
            self.ui.radial.close();
        } else if self.ui.confirm.is_some() {
            self.ui.confirm = None;
        } else if self.ui.prefs_open {
            self.ui.prefs_open = false;
        } else if self.drag.is_some() {
            self.drag = None;
        } else if self.tool.cancel_pending() {
        } else if self.tool.active != Tool::Select {
            self.tool.active = Tool::Select;
        } else if self.view.isolate.is_some() {
            self.view.isolate = None;
        }
    }

    // ------------------------------------------------------------ chrome

    fn top_bar(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        egui::Panel::top("topbar")
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 5,
                bottom: 5,
            }))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Wordmark.
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                    paint_logo(ui.painter(), r, pal.accent);
                    ui.add_space(2.0);
                    ui.scope(|ui| {
                        let w = &mut ui.style_mut().visuals.widgets;
                        w.inactive.weak_bg_fill = Color32::TRANSPARENT;
                        w.inactive.bg_fill = Color32::TRANSPARENT;
                        self.menus(ui);
                    });
                    ui.add_space(10.0);
                    sep(ui, &pal);
                    // Component modes.
                    for (m, icon, label, cmd) in [
                        (Mode::Vertex, Icon::Vertex, S::MODE_VERTEX, Cmd::ModeVertex),
                        (Mode::Edge, Icon::Edge, S::MODE_EDGE, Cmd::ModeEdge),
                        (Mode::Face, Icon::Face, S::MODE_FACE, Cmd::ModeFace),
                        (Mode::Object, Icon::Object, S::MODE_OBJECT, Cmd::ModeObject),
                    ] {
                        let r = icons::icon_button(ui, icon, self.mode == m, 26.0);
                        if r.on_hover_text(format!("{label} mode  ({})", self.keymap.label(cmd)))
                            .clicked()
                        {
                            self.set_mode(m);
                        }
                    }
                    sep(ui, &pal);
                    let s = self.scene_toggles();
                    let r = icons::icon_text_button(
                        ui,
                        Icon::Preview,
                        S::TOGGLE_PREVIEW,
                        self.view.preview,
                    );
                    if r.on_hover_text(format!(
                        "{}  ({})",
                        S::TOGGLE_PREVIEW,
                        self.keymap.label(Cmd::Preview)
                    ))
                    .clicked()
                    {
                        self.run(Cmd::Preview);
                    }
                    for (icon, label, on, cmd) in [
                        (
                            Icon::AutoFill,
                            S::TOGGLE_AUTOFILL,
                            s.as_ref().is_some_and(|s| s.autofill),
                            Cmd::AutoFill,
                        ),
                        (
                            Icon::Link,
                            S::TOGGLE_LINK,
                            s.as_ref().is_some_and(|s| s.link),
                            Cmd::Link,
                        ),
                        (
                            Icon::Symmetry,
                            S::TOGGLE_SYMMETRY,
                            s.as_ref().is_some_and(|s| s.symmetry),
                            Cmd::Symmetry,
                        ),
                    ] {
                        let r = ui
                            .add_enabled_ui(s.is_some(), |ui| {
                                icons::icon_text_button(ui, icon, label, on)
                            })
                            .inner;
                        if r.on_hover_text(format!("{label}  ({})", self.keymap.label(cmd)))
                            .clicked()
                        {
                            self.run(cmd);
                        }
                    }
                    if let Some(s) = &s
                        && s.symmetry
                    {
                        for (axis, cmd, l) in [
                            (newtopo_core::doc::Axis::X, Cmd::MirrorX, "X"),
                            (newtopo_core::doc::Axis::Y, Cmd::MirrorY, "Y"),
                            (newtopo_core::doc::Axis::Z, Cmd::MirrorZ, "Z"),
                        ] {
                            if ui
                                .selectable_label(s.symmetry_axes.contains(&axis), l)
                                .clicked()
                            {
                                self.run(cmd);
                            }
                        }
                    }

                    // Right side: title, palette button.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::icon_button(ui, Icon::Gear, self.ui.prefs_open, 26.0)
                            .on_hover_text(format!(
                                "{}  ({})",
                                S::PREFS_TITLE,
                                self.keymap.label(Cmd::Preferences)
                            ))
                            .clicked()
                        {
                            self.ui.prefs_open = !self.ui.prefs_open;
                        }
                        let pr = icons::icon_text_button(
                            ui,
                            Icon::Search,
                            &format!("Commands  {}", self.keymap.label(Cmd::Palette)),
                            self.ui.palette.open,
                        );
                        if pr.clicked() {
                            self.ui.palette.open_fresh();
                        }
                        ui.add_space(12.0);
                        self.title_label(ui);
                    });
                });
            });
    }

    fn title_label(&self, ui: &mut Ui) {
        let pal = self.pal;
        if self.doc.has_scene() {
            let dirty = self.doc.is_dirty();
            let unsaved = self.doc.path.is_none();
            if unsaved || dirty {
                let tag = if unsaved { S::UNSAVED } else { "modified" };
                egui::Frame::new()
                    .fill(pal.warn.gamma_multiply(0.16))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(7, 1))
                    .show(ui, |ui| {
                        ui.label(RichText::new(tag).small().color(pal.warn))
                    });
            }
            ui.label(
                RichText::new(self.doc.title())
                    .font(theme::medium(14.0))
                    .color(pal.text_strong),
            );
        } else if self.demo.is_some() {
            ui.label(RichText::new(S::DEMO_BADGE).color(pal.text_dim));
        } else {
            ui.label(RichText::new(S::NO_SCENE).color(pal.text_dim));
        }
    }

    fn menus(&mut self, ui: &mut Ui) {
        let mut run: Option<Cmd> = None;
        let item = |ui: &mut Ui, km: &Keymap, cmd: Cmd, run: &mut Option<Cmd>| {
            let sc = km.label(cmd);
            let b = egui::Button::new(cmd.label()).shortcut_text(sc);
            if ui.add(b).clicked() {
                *run = Some(cmd);
                ui.close();
            }
        };
        let km = self.keymap.clone();
        let mut open_recent: Option<PathBuf> = None;
        let mut clear_recent = false;
        {
            ui.menu_button(S::MENU_FILE, |ui| {
                for c in [Cmd::New, Cmd::Open] {
                    item(ui, &km, c, &mut run);
                }
                ui.menu_button(S::MENU_RECENT, |ui| {
                    if self.prefs.recent.is_empty() {
                        ui.label(RichText::new(S::MENU_NO_RECENT).weak());
                    }
                    for p in &self.prefs.recent {
                        let name = p
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if ui
                            .button(name)
                            .on_hover_text(p.display().to_string())
                            .clicked()
                        {
                            open_recent = Some(p.clone());
                            ui.close();
                        }
                    }
                    if !self.prefs.recent.is_empty() {
                        ui.separator();
                        if ui.button(S::MENU_CLEAR_RECENT).clicked() {
                            clear_recent = true;
                            ui.close();
                        }
                    }
                });
                ui.separator();
                for c in [Cmd::Save, Cmd::SaveAs] {
                    item(ui, &km, c, &mut run);
                }
                ui.separator();
                for c in [Cmd::Import, Cmd::Export, Cmd::LoadSample] {
                    item(ui, &km, c, &mut run);
                }
                ui.separator();
                item(ui, &km, Cmd::Quit, &mut run);
            });
            ui.menu_button(S::MENU_EDIT, |ui| {
                for c in [Cmd::Undo, Cmd::Redo] {
                    item(ui, &km, c, &mut run);
                }
                ui.separator();
                for c in [
                    Cmd::CopyRules,
                    Cmd::PasteRules,
                    Cmd::PurgeDormant,
                    Cmd::CheckMesh,
                ] {
                    item(ui, &km, c, &mut run);
                }
                ui.separator();
                for c in [
                    Cmd::SelectAll,
                    Cmd::SelectNone,
                    Cmd::Invert,
                    Cmd::Loop,
                    Cmd::LoopAlternate,
                    Cmd::Ring,
                    Cmd::RingAlternate,
                    Cmd::VisibleOnly,
                ] {
                    item(ui, &km, c, &mut run);
                }
                ui.separator();
                item(ui, &km, Cmd::Preferences, &mut run);
            });
            ui.menu_button(S::MENU_VIEW, |ui| {
                for c in [
                    Cmd::Preview,
                    Cmd::Wireframe,
                    Cmd::FlatShading,
                    Cmd::SymmetryPlanes,
                ] {
                    item(ui, &km, c, &mut run);
                }
                ui.separator();
                for c in [
                    Cmd::Frame,
                    Cmd::Ortho,
                    Cmd::Hide,
                    Cmd::UnhideAll,
                    Cmd::Isolate,
                    Cmd::ExitIsolate,
                ] {
                    item(ui, &km, c, &mut run);
                }
                ui.menu_button(S::MENU_CAMERA, |ui| {
                    for c in [
                        Cmd::ViewFront,
                        Cmd::ViewBack,
                        Cmd::ViewLeft,
                        Cmd::ViewRight,
                        Cmd::ViewTop,
                        Cmd::ViewBottom,
                        Cmd::ViewIso,
                    ] {
                        item(ui, &km, c, &mut run);
                    }
                });
                ui.separator();
                ui.menu_button(S::MENU_PANELS, |ui| {
                    for c in [Cmd::RenamePanel, Cmd::BridgePanel, Cmd::UvPanel] {
                        item(ui, &km, c, &mut run);
                    }
                    ui.separator();
                    for (tab, label) in [
                        (Tab::Outliner, S::TAB_OUTLINER),
                        (Tab::Tool, S::TAB_TOOL),
                        (Tab::Stats, S::TAB_STATS),
                        (Tab::History, S::TAB_HISTORY),
                    ] {
                        if ui.button(label).clicked() {
                            self.ui.dock_actions.push(DockAction::Focus(tab));
                            ui.close();
                        }
                    }
                });
                for c in [Cmd::HidePanels, Cmd::ResetLayout] {
                    item(ui, &km, c, &mut run);
                }
            });
            ui.menu_button(S::MENU_TOOLS, |ui| {
                for c in [
                    Cmd::Connect,
                    Cmd::Grid,
                    Cmd::Slice,
                    Cmd::Iso,
                    Cmd::Cut,
                    Cmd::Weld,
                ] {
                    item(ui, &km, c, &mut run);
                }
                ui.separator();
                for c in [Cmd::Collapse, Cmd::Remove, Cmd::Snap] {
                    item(ui, &km, c, &mut run);
                }
                ui.separator();
                for c in [
                    Cmd::AutoFill,
                    Cmd::Link,
                    Cmd::Symmetry,
                    Cmd::MirrorX,
                    Cmd::MirrorY,
                    Cmd::MirrorZ,
                ] {
                    item(ui, &km, c, &mut run);
                }
            });
            ui.menu_button(S::MENU_HELP, |ui| {
                for c in [Cmd::Palette, Cmd::KeymapEditor, Cmd::About] {
                    item(ui, &km, c, &mut run);
                }
            });
        }
        if clear_recent {
            self.prefs.recent.clear();
            self.dirty_settings = true;
        }
        if let Some(p) = open_recent {
            self.request(PendingAfterPrompt::Open(Some(p)));
        }
        if let Some(c) = run {
            self.run(c);
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        egui::Panel::bottom("statusbar")
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin {
                left: 10,
                right: 10,
                top: 3,
                bottom: 4,
            }))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let hint = if self.is_demo() && self.tool.active == Tool::Select {
                        S::HINT_DEMO
                    } else {
                        self.tool.active.hint()
                    };
                    ui.label(RichText::new(hint).small().color(pal.text_dim));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mode = match self.mode {
                            Mode::Vertex => S::MODE_VERTEX,
                            Mode::Edge => S::MODE_EDGE,
                            Mode::Face => S::MODE_FACE,
                            Mode::Object => S::MODE_OBJECT,
                        };
                        let n = self.sel.count(self.mode);
                        ui.label(
                            RichText::new(format!("{mode} · {n} selected"))
                                .small()
                                .color(pal.text_dim),
                        );
                        if self.view.visible_only {
                            ui.label(RichText::new(S::VP_VISIBLE_ONLY).small().color(pal.accent));
                        }
                        if self.tool.snap && matches!(self.tool.active, Tool::Iso | Tool::Cut) {
                            ui.label(RichText::new("Snap").small().color(pal.accent));
                        }
                        self.jobs_status(ui);
                    });
                });
            });
    }

    fn overlays(&mut self, ctx: &egui::Context) {
        self.palette_ui(ctx);
        self.radial_ui(ctx);
        self.key_capture_ui(ctx);
        self.prefs_window(ctx);
        self.dialogs_ui(ctx);
    }
}

#[derive(Clone, Copy)]
enum SettingToggle {
    AutoFill,
    Link,
    Symmetry,
}

fn sep(ui: &mut Ui, pal: &Palette) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(egui::vec2(1.0, 18.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 0.0, pal.border);
    ui.add_space(4.0);
}

/// The newtopo mark: a quad split by a curved line, drawn in the accent.
pub fn paint_logo(p: &egui::Painter, r: egui::Rect, accent: Color32) {
    let r = r.shrink(1.0);
    p.rect_filled(r, 5.0, accent);
    let dark = Color32::from_rgb(10, 22, 18);
    let s = egui::Stroke::new(1.6, dark);
    let a = r.left_top() + egui::vec2(r.width() * 0.32, 3.0);
    let b = r.left_bottom() + egui::vec2(r.width() * 0.32, -3.0);
    let c = r.right_top() + egui::vec2(-r.width() * 0.3, 3.0);
    let d = r.right_bottom() + egui::vec2(-r.width() * 0.3, -3.0);
    p.line_segment([a, b], s);
    p.add(egui::Shape::QuadraticBezier(
        egui::epaint::QuadraticBezierShape::from_points_stroke(
            [c, egui::pos2(r.center().x + 1.0, r.center().y), d],
            false,
            Color32::TRANSPARENT,
            s,
        ),
    ));
    p.line_segment(
        [
            egui::pos2(r.left() + 3.0, r.center().y),
            egui::pos2(r.right() - 3.0, r.center().y),
        ],
        s,
    );
}
