//! UI interaction tests: drive the real app UI with synthetic input (no GPU,
//! no window) against the sample model and check the resulting state.

use crate::app::App;
use crate::commands::Cmd;
use crate::prefs::Paths;
use crate::selection::Mode;
use crate::tools::Tool;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use newtopo_kernel::{BodyId, FaceId};
use newtopo_render::pick::FaceRef;

struct Harness {
    ctx: egui::Context,
    app: App,
    t: f64,
    mods: Modifiers,
}

impl Harness {
    fn new(sample: bool) -> Self {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, Paths::default(), false, true);
        if sample {
            app.st.load_sample();
        }
        let mut h = Harness {
            ctx,
            app,
            t: 1.0,
            mods: Modifiers::NONE,
        };
        h.frames(3);
        h
    }

    fn frame(&mut self, mut events: Vec<Event>) {
        self.t += 0.05;
        events.insert(0, Event::ModifiersChanged(self.mods));
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))),
            time: Some(self.t),
            events,
            focused: true,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| app.frame(ui));
        out.textures_delta.clear();
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![]);
        }
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        self.mods = modifiers;
        self.frame(vec![Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }]);
        self.frame(vec![Event::Key {
            key,
            physical_key: Some(key),
            pressed: false,
            repeat: false,
            modifiers,
        }]);
        self.mods = Modifiers::NONE;
    }

    fn click(&mut self, at: Pos2, modifiers: Modifiers) {
        self.mods = modifiers;
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers,
        }]);
        self.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        }]);
        self.mods = Modifiers::NONE;
        self.frames(1);
    }

    fn drag(&mut self, from: Pos2, to: Pos2, button: PointerButton) {
        let m = Modifiers::NONE;
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![Event::PointerButton {
            pos: from,
            button,
            pressed: true,
            modifiers: m,
        }]);
        for k in 1..=6 {
            let p = from + (to - from) * (k as f32 / 6.0);
            self.frame(vec![Event::PointerMoved(p)]);
        }
        self.frame(vec![Event::PointerButton {
            pos: to,
            button,
            pressed: false,
            modifiers: m,
        }]);
        self.frames(1);
    }

    /// Screen position of a world point in the viewport.
    fn screen_of(&self, p: glam::Vec3) -> Pos2 {
        let cam = self.app.st.camera_matrices();
        let (s, _) = cam.project(p.as_dvec3()).expect("in front of camera");
        let r = self.app.st.view.rect;
        pos2(r.left() + s.x as f32, r.top() + s.y as f32)
    }
}

#[test]
fn starts_empty_and_survives_frames() {
    let mut h = Harness::new(false);
    h.frames(5);
    assert!(!h.app.st.has_content());
    assert!(h.app.st.view.rect.width() > 100.0, "viewport laid out");
}

#[test]
fn mode_keys_and_palette() {
    let mut h = Harness::new(true);
    h.key(Key::Num3, Modifiers::NONE);
    assert_eq!(h.app.st.mode, Mode::Face);
    h.key(Key::Num2, Modifiers::NONE);
    assert_eq!(h.app.st.mode, Mode::Edge);
    h.key(Key::K, Modifiers::COMMAND);
    assert!(h.app.st.ui.palette.open);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.ui.palette.open);
}

#[test]
fn click_selects_face_and_shift_adds() {
    let mut h = Harness::new(true);
    h.key(Key::Num3, Modifiers::NONE);
    let boss = h.app.st.vd.face_centers[&FaceRef { body: 0, face: 4 }];
    // The cylinder's area centre lies on its axis; aim at its front instead.
    let front = boss + (h.app.st.view.camera.eye().as_vec3() - boss).normalize() * 2.0;
    let at = h.screen_of(boss.lerp(front, 0.0));
    h.click(at, Modifiers::NONE);
    assert!(
        h.app.st.sel.faces.contains(&(BodyId(0), FaceId(4))),
        "selected {:?}",
        h.app.st.sel.faces
    );
    let ring = h.app.st.vd.face_centers[&FaceRef { body: 1, face: 0 }];
    let ring_at = h.screen_of(ring + glam::Vec3::new(0.0, -14.0, 5.0));
    h.click(ring_at, Modifiers::SHIFT);
    assert_eq!(
        h.app.st.sel.faces.len(),
        2,
        "shift adds: {:?}",
        h.app.st.sel.faces
    );
    // Ctrl removes.
    h.click(at, Modifiers::COMMAND);
    assert!(!h.app.st.sel.faces.contains(&(BodyId(0), FaceId(4))));
    // Clicking empty space clears.
    let empty = h.app.st.view.rect.left_top() + vec2(30.0, 200.0);
    h.click(empty, Modifiers::NONE);
    assert!(h.app.st.sel.faces.is_empty());
}

#[test]
fn box_select_objects() {
    let mut h = Harness::new(true);
    let r = h.app.st.view.rect;
    h.drag(
        r.left_top() + vec2(5.0, 160.0),
        r.right_bottom() - vec2(5.0, 5.0),
        PointerButton::Primary,
    );
    assert_eq!(h.app.st.sel.objects.len(), 2, "box covers both bodies");
    // Hide, then unhide all.
    h.key(Key::H, Modifiers::NONE);
    assert_eq!(h.app.st.view.hidden.len(), 2);
    h.key(Key::H, Modifiers::ALT);
    assert!(h.app.st.view.hidden.is_empty());
}

#[test]
fn lasso_select_with_right_drag() {
    let mut h = Harness::new(true);
    let r = h.app.st.view.rect;
    let m = Modifiers::NONE;
    let pts = [
        r.left_top() + vec2(5.0, 160.0),
        r.right_top() + vec2(-5.0, 160.0),
        r.right_bottom() - vec2(5.0, 5.0),
        r.left_bottom() + vec2(5.0, -5.0),
    ];
    h.frame(vec![Event::PointerMoved(pts[0])]);
    h.frame(vec![Event::PointerButton {
        pos: pts[0],
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: m,
    }]);
    for w in 0..4 {
        for k in 1..=4 {
            let p = pts[w] + (pts[(w + 1) % 4] - pts[w]) * (k as f32 / 4.0);
            h.frame(vec![Event::PointerMoved(p)]);
        }
    }
    h.frame(vec![Event::PointerButton {
        pos: pts[0],
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: m,
    }]);
    h.frames(1);
    assert_eq!(h.app.st.sel.objects.len(), 2);
}

#[test]
fn radial_tap_keeps_open_and_hold_runs() {
    let mut h = Harness::new(true);
    // Tap: press and release quickly → stays open.
    h.key(Key::Q, Modifiers::NONE);
    assert!(h.app.st.ui.radial.open && h.app.st.ui.radial.sticky);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.ui.radial.open);
    // Hold over the slot at 12 o'clock (Connect by default) and release.
    let c = h.app.st.view.rect.center();
    h.frame(vec![Event::PointerMoved(c)]);
    h.frame(vec![Event::Key {
        key: Key::Q,
        physical_key: Some(Key::Q),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    let centre = h.app.st.ui.radial.center;
    for k in 0..8 {
        h.frame(vec![Event::PointerMoved(
            centre + vec2(0.0, -20.0 - 12.0 * k as f32),
        )]);
    }
    h.frame(vec![Event::Key {
        key: Key::Q,
        physical_key: Some(Key::Q),
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    h.frames(1);
    assert!(!h.app.st.ui.radial.open);
    assert_eq!(h.app.st.tool.active, Tool::Connect);
}

#[test]
fn escape_precedence_tool_then_isolate() {
    let mut h = Harness::new(true);
    h.app.st.sel.objects.insert(BodyId(1));
    h.key(Key::Slash, Modifiers::NONE);
    assert!(h.app.st.view.isolate.is_some());
    h.key(Key::C, Modifiers::NONE);
    assert_eq!(h.app.st.tool.active, Tool::Connect);
    h.key(Key::Escape, Modifiers::NONE);
    assert_eq!(
        h.app.st.tool.active,
        Tool::Select,
        "first Esc leaves the tool"
    );
    assert!(h.app.st.view.isolate.is_some());
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.app.st.view.isolate.is_none(), "second Esc exits isolate");
}

#[test]
fn edge_loop_double_press() {
    let mut h = Harness::new(true);
    h.key(Key::Num2, Modifiers::NONE);
    // A horizontal edge half way up the boss.
    let i = h
        .app
        .st
        .vd
        .edges
        .iter()
        .position(|e| {
            e.body == BodyId(0)
                && (e.path[0].z - e.path[1].z).abs() < 1e-3
                && e.path[0].z > 20.0
                && e.path[0].z < 40.0
        })
        .expect("edge");
    let sel = h.app.st.vd.edges[i].sel.clone().unwrap();
    h.app.st.sel.edges.insert(sel);
    h.key(Key::L, Modifiers::NONE);
    assert_eq!(h.app.st.sel.edges.len(), 16, "loop around the boss");
    h.frames(10); // let the double-press window pass
    h.app.st.sel.edges.clear();
    h.app
        .st
        .sel
        .edges
        .insert(h.app.st.vd.edges[i].sel.clone().unwrap());
    h.key(Key::L, Modifiers::NONE);
    h.key(Key::L, Modifiers::NONE);
    assert_eq!(h.app.st.sel.edges.len(), 8, "double L = every other");
}

#[test]
fn commands_all_run_without_scene() {
    // Every command must degrade gracefully with no scene and no sample.
    let mut h = Harness::new(false);
    for c in crate::commands::ALL {
        if matches!(
            c,
            Cmd::Quit | Cmd::Open | Cmd::Import | Cmd::SaveAs | Cmd::New
        ) {
            continue; // file dialogs / quitting
        }
        h.app.st.run(*c);
        h.frames(1);
    }
    // And with the sample loaded.
    let mut h = Harness::new(true);
    h.app.st.sel.objects.insert(BodyId(0));
    for c in crate::commands::ALL {
        if matches!(
            c,
            Cmd::Quit | Cmd::Open | Cmd::Import | Cmd::SaveAs | Cmd::New
        ) {
            continue;
        }
        h.app.st.run(*c);
        h.frames(1);
    }
}

#[test]
fn viewport_orbit_with_middle_drag() {
    let mut h = Harness::new(true);
    let yaw = h.app.st.view.camera.yaw;
    let c = h.app.st.view.rect.center();
    h.drag(c, c + vec2(120.0, 0.0), PointerButton::Middle);
    assert!(
        (h.app.st.view.camera.yaw - yaw).abs() > 0.1,
        "middle drag orbits (Blender preset)"
    );
}

#[test]
fn locked_objects_are_left_out_of_select_all_and_edits() {
    let mut h = Harness::new(true);
    h.app.st.local.body_locked.insert(BodyId(0));
    h.key(Key::Num2, Modifiers::NONE);
    h.app.st.run(Cmd::SelectAll);
    assert!(!h.app.st.sel.edges.is_empty());
    assert!(
        h.app.st.sel.edges.iter().all(|e| e.body() != BodyId(0)),
        "edges of the locked body"
    );
    h.key(Key::Num1, Modifiers::NONE);
    h.app.st.run(Cmd::SelectAll);
    assert!(
        h.app.st.sel.verts.iter().all(|v| v.body() != BodyId(0)),
        "vertices of the locked body"
    );
    // Defence in depth: a selection that already holds locked edges.
    h.key(Key::Num2, Modifiers::NONE);
    let locked_edge = h
        .app
        .st
        .vd
        .edges
        .iter()
        .find(|e| e.body == BodyId(0))
        .and_then(|e| e.sel.clone())
        .unwrap();
    h.app.st.sel.edges.insert(locked_edge.clone());
    h.app.st.remove_selected();
    assert!(
        !h.app.st.sel.edges.contains(&locked_edge),
        "remove skips locked bodies"
    );
    h.app.st.sel.edges.insert(locked_edge.clone());
    h.app.st.collapse_selected();
    assert!(
        !h.app.st.sel.edges.contains(&locked_edge),
        "collapse skips locked bodies"
    );
    let i = h
        .app
        .st
        .vd
        .verts
        .iter()
        .position(|v| v.body == BodyId(0))
        .unwrap();
    let j = h
        .app
        .st
        .vd
        .verts
        .iter()
        .rposition(|v| v.body == BodyId(0))
        .unwrap();
    h.app.st.weld(i, j);
    assert!(
        h.app
            .st
            .toasts
            .items
            .last()
            .is_some_and(|t| t.text == crate::strings::TOAST_LOCKED_SKIPPED)
    );
}

#[test]
fn spread_tools_build_one_op_per_body() {
    let mut h = Harness::new(true);
    h.app.st.mode = Mode::Face;
    for f in [3u32, 4] {
        h.app.st.sel.faces.insert((BodyId(0), FaceId(f)));
    }
    h.app.st.sel.faces.insert((BodyId(1), FaceId(0)));
    h.app.st.tool.active = Tool::Connect;
    let ops = h.app.st.segment_ops();
    assert_eq!(ops.len(), 2, "one Connect per body");
    for op in &ops {
        let newtopo_core::ops::Op::Connect { patches, .. } = op else {
            panic!("{op:?}")
        };
        let b = patches.body.expect("body set");
        assert!(
            patches.patches.iter().all(|p| p.body == b),
            "patches of one body only"
        );
    }
    h.app.st.tool.seg.a = 0;
    h.app.st.tool.seg.b = 0;
    assert_eq!(
        h.app.st.segment_ops().len(),
        2,
        "count 0 clears per body too"
    );
    h.app.st.tool.active = Tool::Grid;
    assert_eq!(h.app.st.segment_ops().len(), 2);
}

#[test]
fn escape_closes_dialog_before_leaving_the_tool() {
    let mut h = Harness::new(true);
    h.key(Key::C, Modifiers::NONE);
    assert_eq!(h.app.st.tool.active, Tool::Connect);
    h.app.st.ui.about_open = true;
    h.frames(1);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.ui.about_open, "Esc closes the dialog");
    assert_eq!(
        h.app.st.tool.active,
        Tool::Connect,
        "the same Esc doesn't also leave the tool"
    );
}

#[test]
fn one_modal_at_a_time_and_esc_closes_the_visible_one() {
    use crate::jobs::{ExportDialog, Modal};
    let mut h = Harness::new(true);
    h.app.st.ui.about_open = true;
    h.app.st.ui.export = Some(ExportDialog {
        format: h.app.st.prefs.export_format,
        opts: h.app.st.prefs.export.clone(),
        units: h.app.st.prefs.units.clone(),
    });
    h.frames(1);
    assert_eq!(h.app.st.active_modal(), Some(Modal::Export));
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.app.st.ui.export.is_none(), "Esc closes the shown dialog");
    assert!(h.app.st.ui.about_open, "the waiting dialog is untouched");
    assert_eq!(h.app.st.active_modal(), Some(Modal::About));
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.ui.about_open);
    assert_eq!(h.app.st.active_modal(), None);

fn update_results_follow_manual_and_skip_rules() {
    use crate::update::{Available, Check, Outcome, Version};
    let newer = || Ok(Outcome::Newer(Available::for_version(Version(9, 0, 0))));
    let mut h = Harness::new(true);

    // Automatic check, skipped version: nothing shows.
    h.app.st.prefs.update_skipped = Some("9.0.0".into());
    h.app.st.updates.check = Some(Check::finished(newer(), false));
    h.frames(1);
    assert!(h.app.st.updates.check.is_none());
    assert!(h.app.st.updates.available.is_none());
    assert!(!h.app.st.updates.dialog_open);

    // Manual check shows it even when skipped, and opens the dialog.
    h.app.st.updates.check = Some(Check::finished(newer(), true));
    h.frames(1);
    assert_eq!(
        h.app.st.updates.available.as_ref().map(|a| a.version),
        Some(Version(9, 0, 0))
    );
    assert!(h.app.st.updates.dialog_open);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(
        !h.app.st.updates.dialog_open,
        "Esc closes the update dialog"
    );

    // Automatic check, new version not skipped: chip, no dialog.
    h.app.st.prefs.update_skipped = None;
    h.app.st.updates.available = None;
    h.app.st.updates.check = Some(Check::finished(newer(), false));
    h.frames(1);
    assert!(h.app.st.updates.available.is_some());
    assert!(!h.app.st.updates.dialog_open);

    // Up to date clears a stale notice; errors from automatic checks stay quiet.
    h.app.st.updates.check = Some(Check::finished(
        Ok(Outcome::UpToDate(Version(9, 0, 0))),
        false,
    ));
    h.frames(1);
    assert!(h.app.st.updates.available.is_none());
    let toasts = h.app.st.toasts.items.len();
    h.app.st.updates.check = Some(Check::finished(Err("offline".into()), false));
    h.frames(1);
    assert_eq!(
        h.app.st.toasts.items.len(),
        toasts,
        "automatic errors don't toast"
    );
}

#[test]
fn update_question_is_remembered_and_never_shown_headless() {
    let mut h = Harness::new(false);
    // Screenshot/test mode never asks or checks on its own.
    assert!(!h.app.st.updates.ask_open);
    assert!(h.app.st.updates.check.is_none());
    h.app.st.updates.ask_open = true;
    h.frames(1);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.updates.ask_open);
    assert_eq!(
        h.app.st.prefs.update_check,
        Some(false),
        "Esc on the question means don't check"
    );
}

#[test]
fn esc_spares_a_dialog_that_arrived_unseen() {
    use crate::jobs::{ExportDialog, ImportReport, Modal};
    let mut h = Harness::new(true);
    h.app.st.ui.export = Some(ExportDialog {
        format: h.app.st.prefs.export_format,
        opts: h.app.st.prefs.export.clone(),
        units: h.app.st.prefs.units.clone(),
    });
    h.frames(1);
    assert_eq!(h.app.st.ui.shown_modal, Some(Modal::Export));
    // A job finishes (poll_jobs runs before input) in the same frame as Esc.
    h.app.st.ui.import_report = Some(ImportReport {
        file: "part.step".into(),
        bodies: 1,
        faces: 6,
        fixes: vec![],
        failed_faces: 0,
        warnings: vec![],
        errors: vec![],
    });
    h.key(Key::Escape, Modifiers::NONE);
    assert!(
        h.app.st.ui.export.is_none(),
        "Esc closes the visible Export"
    );
    assert!(
        h.app.st.ui.import_report.is_some(),
        "the unseen report survives and shows next"
    );
    h.frames(1);
    assert_eq!(h.app.st.ui.shown_modal, Some(Modal::ImportReport));
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.app.st.ui.import_report.is_none());
}

#[test]
fn update_dialog_waits_for_other_dialogs() {
    use crate::update::{Available, Version};
    let mut h = Harness::new(true);
    h.app.st.ui.about_open = true;
    h.app.st.updates.available = Some(Available::for_version(Version(9, 0, 0)));
    h.app.st.updates.dialog_open = true;
    h.frames(1);
    // About is the visible dialog, so Esc closes About and the update
    // dialog is still pending.
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.ui.about_open);
    assert!(h.app.st.updates.dialog_open);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.updates.dialog_open);
}
