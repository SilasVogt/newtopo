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
fn named_view_is_orthographic_until_the_next_orbit() {
    let mut h = Harness::new(true);
    let c = h.app.st.view.rect.center();
    assert!(!h.app.st.view.camera.ortho);
    h.app.st.run(Cmd::ViewTop);
    assert!(h.app.st.view.camera.ortho, "a named view is orthographic");
    h.drag(c, c + vec2(0.0, 80.0), PointerButton::Middle);
    assert!(
        !h.app.st.view.camera.ortho,
        "orbiting returns to perspective"
    );

    // An orthographic camera the user chose stays orthographic.
    h.app.st.run(Cmd::Ortho);
    h.app.st.run(Cmd::ViewFront);
    h.drag(c, c + vec2(120.0, 0.0), PointerButton::Middle);
    assert!(h.app.st.view.camera.ortho, "the user's choice is kept");

    // Switching by hand during a named view keeps that choice too.
    h.app.st.run(Cmd::Ortho);
    h.app.st.run(Cmd::ViewRight);
    h.app.st.run(Cmd::Ortho);
    h.app.st.run(Cmd::Ortho);
    h.drag(c, c + vec2(120.0, 0.0), PointerButton::Middle);
    assert!(h.app.st.view.camera.ortho);
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
}

#[test]
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
    // Screenshot/test mode never asks or checks: not at startup, and not
    // when the command runs (no request leaves a test run).
    h.app.st.startup_update_check();
    assert!(!h.app.st.updates.ask_open);
    assert!(h.app.st.updates.check.is_none());
    h.app.st.run(Cmd::CheckUpdates);
    assert!(h.app.st.updates.check.is_none(), "no network in tests");
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

#[test]
fn update_dialog_waits_for_shortcut_capture() {
    use crate::update::{Available, Version};
    let mut h = Harness::new(true);
    h.app.st.ui.key_capture = Some(crate::palette::KeyCapture {
        cmd: crate::commands::Cmd::Connect,
        captured: None,
        double: false,
    });
    h.app.st.updates.available = Some(Available::for_version(Version(9, 0, 0)));
    h.app.st.updates.dialog_open = true;
    h.frames(2);
    assert!(h.app.st.ui.key_capture.is_some(), "capture keeps the focus");
    assert!(
        h.app.st.updates.dialog_open,
        "update dialog is still pending"
    );
}

#[test]
fn esc_spares_an_update_dialog_that_arrived_unseen() {
    use crate::update::{Available, Check, Outcome, Version};
    let mut h = Harness::new(true);
    h.app.st.ui.prefs_open = true;
    h.frames(1);
    // A manual check finishes in the same frame as an Esc meant for
    // Preferences: Preferences closes, the new notice survives.
    h.app.st.updates.check = Some(Check::finished(
        Ok(Outcome::Newer(Available::for_version(Version(9, 0, 0)))),
        true,
    ));
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.ui.prefs_open);
    assert!(h.app.st.updates.dialog_open);
    h.frames(1);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.updates.dialog_open, "seen, so Esc closes it");
}

#[test]
fn only_one_update_dialog_at_a_time() {
    use crate::update::{Available, UpdateDialog, Version};
    let mut h = Harness::new(true);
    h.app.st.updates.ask_open = true;
    h.app.st.updates.available = Some(Available::for_version(Version(9, 0, 0)));
    h.app.st.updates.dialog_open = true;
    h.frames(1);
    assert_eq!(h.app.st.updates.shown, Some(UpdateDialog::Available));
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.updates.dialog_open);
    assert!(h.app.st.updates.ask_open, "the question waits its turn");
    h.frames(1);
    assert_eq!(h.app.st.updates.shown, Some(UpdateDialog::Ask));
}

#[test]
fn rename_field_can_be_left_and_cancelled() {
    use crate::panels::RenameTarget;
    let mut h = Harness::new(false);
    h.app.st.doc.set_scene(
        newtopo_core::scene::Scene::new(newtopo_kernel::default_kernel()),
        None,
    );
    h.frames(1);
    let id = h.app.st.layer_rows()[0].id;
    let name = |h: &Harness| {
        h.app
            .st
            .layer_rows()
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.name.clone())
    };
    h.app.st.ui.outliner.renaming = Some((RenameTarget::Layer(id), "Renamed".into()));
    h.app.st.ui.outliner.focus_rename = true;
    h.frames(3);
    assert!(h.app.st.ui.outliner.renaming.is_some(), "still editing");
    assert!(
        !h.app.st.ui.outliner.focus_rename,
        "focus is asked for once"
    );
    // Clicking elsewhere leaves the field and keeps the typed name.
    h.click(pos2(800.0, 500.0), Modifiers::NONE);
    assert!(h.app.st.ui.outliner.renaming.is_none(), "the field let go");
    assert_eq!(name(&h).as_deref(), Some("Renamed"));
    // Esc cancels without renaming (and doesn't reach tools or dialogs).
    h.app.st.ui.outliner.renaming = Some((RenameTarget::Layer(id), "Other".into()));
    h.app.st.ui.outliner.focus_rename = true;
    h.frames(2);
    // With a mesh preview open, that Esc cancels the rename only.
    h.app.st.apply(newtopo_core::Op::SetAutofill { on: true });
    h.frames(2);
    assert!(h.app.st.doc.in_preview());
    assert!(h.app.st.ui.outliner.renaming.is_some());
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.app.st.ui.outliner.renaming.is_none());
    assert_eq!(name(&h).as_deref(), Some("Renamed"));
    assert!(h.app.st.doc.in_preview(), "the preview survives");
    h.frames(1);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.doc.in_preview(), "the next Esc discards it");
}

#[test]
fn mesh_changes_preview_until_applied() {
    use newtopo_core::Op;
    let mut h = Harness::new(false);
    h.app.st.doc.set_scene(
        newtopo_core::scene::Scene::new(newtopo_kernel::default_kernel()),
        None,
    );
    h.frames(1);
    let autofill = |h: &Harness| h.app.st.scene_toggles().is_some_and(|s| s.autofill);
    assert!(!autofill(&h), "new scenes start with auto-fill off");

    // A mesh change opens a preview and isn't written yet.
    h.app.st.apply(Op::SetAutofill { on: true });
    h.frames(1);
    assert!(h.app.st.doc.in_preview());
    assert!(autofill(&h), "the preview shows the change");
    let tmp = tempfile::tempdir().unwrap();
    assert!(
        h.app.st.doc.save(&tmp.path().join("x.ntopo")).is_err(),
        "nothing is saved while previewing"
    );
    // Enter applies.
    h.key(Key::Enter, Modifiers::NONE);
    assert!(!h.app.st.doc.in_preview());
    assert!(autofill(&h));
    assert!(h.app.st.doc.save(&tmp.path().join("x.ntopo")).is_ok());

    // Esc discards.
    h.app.st.apply(Op::SetAutofill { on: false });
    h.frames(1);
    assert!(h.app.st.doc.in_preview());
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.doc.in_preview());
    assert!(autofill(&h), "discarded: still on");
    assert!(
        !h.app.st.doc.is_dirty(),
        "a discarded preview leaves a saved document saved"
    );

    // Undo discards too, and the applied change undoes in one step.
    h.app.st.apply(Op::SetAutofill { on: false });
    h.app.st.run(Cmd::Undo);
    assert!(!h.app.st.doc.in_preview());
    assert!(autofill(&h));
    assert!(!h.app.st.doc.is_dirty(), "undo of a preview too");
    h.app.st.run(Cmd::Undo);
    assert!(!autofill(&h), "the applied preview was one undo step");
}

#[test]
fn autofill_compare_shows_the_mesh_even_when_compare_was_on() {
    use newtopo_core::Op;
    let mut h = Harness::new(false);
    h.app.st.doc.set_scene(
        newtopo_core::scene::Scene::new(newtopo_kernel::default_kernel()),
        None,
    );
    h.frames(1);
    // A preview in CAD display, with Compare already switched on.
    h.app.st.apply(Op::SetLink { on: true });
    h.app.st.apply(Op::SetAutofill { on: true });
    h.frames(1);
    assert!(h.app.st.doc.in_preview());
    h.app.st.view.preview = false;
    h.app.st.view.compare = true;
    h.app.st.run(Cmd::AutoFill);
    assert!(h.app.st.view.compare);
    assert!(h.app.st.view.preview, "both sides show the output mesh");
    // Discard returns to the display mode from before.
    h.app.st.discard_preview();
    assert!(!h.app.st.view.compare);
    assert!(!h.app.st.view.preview);
}

#[test]
fn refused_op_leaves_no_preview_open() {
    use newtopo_core::Op;
    use newtopo_core::ops::PatchSel;
    let mut h = Harness::new(false);
    h.app.st.doc.set_scene(
        newtopo_core::scene::Scene::new(newtopo_kernel::default_kernel()),
        None,
    );
    h.frames(1);
    // No such body/face: the op is refused, so nothing is pending.
    let r = h.app.st.apply(Op::Connect {
        patches: PatchSel {
            faces: vec![9999],
            ..Default::default()
        },
        a: 4,
        b: 0,
        start_on_seam: false,
    });
    assert!(r.is_none(), "refused");
    assert!(!h.app.st.doc.in_preview(), "no empty preview blocks saving");
}

// ------------------------------------------------------------- mesh mode

/// A real scene (mock kernel): a box with a 4 × 4 grid on every face.
fn mesh_harness(warning: bool) -> (Harness, BodyId) {
    use newtopo_core::Op;
    use newtopo_core::ops::PatchSel;
    let mut h = Harness::new(false);
    h.app.st.prefs.mesh_warning = warning;
    let k = std::sync::Arc::new(newtopo_kernel::mock::MockKernel::new());
    let mut s = newtopo_core::scene::Scene::new(k);
    let body = newtopo_kernel::mock::MockBody::make_box(glam::DVec3::new(40.0, 30.0, 20.0));
    let b = s
        .add_body(
            &body.to_brep_bytes(),
            newtopo_kernel::BodyAttributes {
                name: "Box".into(),
                transform: glam::DMat4::IDENTITY,
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap();
    s.apply(Op::SetAutofill { on: true }).unwrap();
    for f in 0..6 {
        s.apply(Op::Grid {
            patches: PatchSel {
                faces: vec![f],
                ..Default::default()
            },
            u: 4,
            v: 4,
            start_on_seam: false,
            spacing: newtopo_core::doc::Spacing::ArcLength,
        })
        .unwrap();
    }
    h.app.st.doc.set_scene(s, None);
    h.frames(3);
    (h, b)
}

fn enter_mesh(h: &mut Harness) {
    h.app.st.prefs.mesh_warning = false;
    h.key(Key::M, Modifiers::NONE);
    h.frames(2);
    assert!(h.app.st.in_mesh_mode());
}

fn has_edits(h: &Harness, b: BodyId) -> bool {
    h.app.st.doc.scene.as_ref().unwrap().has_mesh_edits(b)
}

/// A visible interior (non-CAD) edge of the editable mesh: the one nearest
/// the camera.
fn near_edge(h: &Harness) -> usize {
    let eye = h.app.st.view.camera.eye().as_vec3();
    let vd = &h.app.st.vd;
    (0..vd.edges.len())
        .filter(|&i| vd.edges[i].kind != crate::viewdata::EdgeKind::Cad)
        .min_by(|&a, &b| {
            let m = |i: usize| {
                vd.edges[i].path[0]
                    .lerp(vd.edges[i].path[1], 0.5)
                    .distance(eye)
            };
            m(a).total_cmp(&m(b))
        })
        .unwrap()
}

fn edge_mid(h: &Harness, i: usize) -> Pos2 {
    let e = &h.app.st.vd.edges[i];
    h.screen_of(e.path[0].lerp(e.path[1], 0.5))
}

fn edge_pair(h: &Harness, i: usize) -> [u32; 2] {
    let crate::selection::EdgeSel::Index(b, k) = h.app.st.vd.edges[i].sel.clone().unwrap() else {
        panic!("mesh edges are indexed");
    };
    let (x, y) = h.app.st.mm.edges[&b][k as usize];
    [x, y]
}

#[test]
fn mesh_mode_warns_once_and_the_tick_box_turns_it_off() {
    let (mut h, _) = mesh_harness(true);
    let cad_accent = h.app.st.pal.accent;
    h.key(Key::M, Modifiers::NONE);
    assert!(h.app.st.mm.dialog, "the first switch asks");
    assert!(
        !h.app.st.in_mesh_mode(),
        "nothing switches before the answer"
    );
    // Cancel (Esc) stays in CAD mode.
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.st.mm.dialog);
    assert!(!h.app.st.in_mesh_mode());
    // Asked again; confirm without the tick: no more questions this session.
    h.key(Key::M, Modifiers::NONE);
    assert!(h.app.st.mm.dialog);
    h.app.st.confirm_mesh_warning();
    h.frames(2);
    assert!(h.app.st.in_mesh_mode());
    assert!(
        h.app.st.prefs.mesh_warning,
        "no tick: still on for next time"
    );
    assert_eq!(
        h.app.st.pal.accent, h.app.st.pal.mesh_accent,
        "amber accent"
    );
    assert_ne!(h.app.st.pal.accent, cad_accent);
    h.key(Key::M, Modifiers::NONE);
    assert!(!h.app.st.in_mesh_mode());
    assert_eq!(h.app.st.pal.accent, cad_accent, "the CAD accent is back");
    h.key(Key::M, Modifiers::NONE);
    assert!(!h.app.st.mm.dialog, "once per session");
    assert!(h.app.st.in_mesh_mode());

    // A new session asks again; the tick box turns the warning off.
    let (mut h, _) = mesh_harness(true);
    h.key(Key::M, Modifiers::NONE);
    assert!(h.app.st.mm.dialog);
    h.app.st.mm.dont_show = true;
    h.app.st.confirm_mesh_warning();
    assert!(h.app.st.in_mesh_mode());
    assert!(!h.app.st.prefs.mesh_warning, "stored in preferences");
    assert!(h.app.st.dirty_settings);
    // The Preferences row turns it back on.
    h.app.st.prefs.mesh_warning = true;
    let (mut h2, _) = mesh_harness(true);
    h2.key(Key::M, Modifiers::NONE);
    assert!(h2.app.st.mm.dialog);
}

#[test]
fn mesh_and_cad_tools_stay_in_their_mode() {
    let (mut h, _) = mesh_harness(false);
    // A mesh tool in CAD mode: refused with a toast.
    let n = h.app.st.toasts.items.len();
    h.key(Key::R, Modifiers::SHIFT);
    assert_eq!(h.app.st.mm.tool, crate::meshmode::MeshTool::Select);
    assert!(h.app.st.toasts.items.len() > n);
    enter_mesh(&mut h);
    assert_eq!(h.app.st.mode, Mode::Edge, "mesh mode starts in edge mode");
    // CAD tools are not available in mesh mode.
    h.key(Key::C, Modifiers::NONE);
    assert_eq!(h.app.st.tool.active, Tool::Select);
    assert!(
        h.app
            .st
            .toasts
            .items
            .last()
            .unwrap()
            .text
            .contains("CAD tool"),
        "{:?}",
        h.app.st.toasts.items.last()
    );
    // Object mode belongs to CAD editing.
    h.key(Key::Num4, Modifiers::NONE);
    assert_ne!(h.app.st.mode, Mode::Object);
    // The CAD selection comes back on return, the mesh one stays apart.
    h.key(Key::Num1, Modifiers::NONE);
    let first = h.app.st.vd.verts[0].sel.clone();
    h.app.st.sel.verts.insert(first);
    h.key(Key::M, Modifiers::NONE);
    assert!(!h.app.st.in_mesh_mode());
    assert_eq!(h.app.st.mode, Mode::Object);
    assert!(h.app.st.sel.verts.is_empty());
    h.key(Key::M, Modifiers::NONE);
    assert_eq!(h.app.st.mode, Mode::Vertex);
    assert_eq!(h.app.st.sel.verts.len(), 1);
}

#[test]
fn alt_click_selects_an_edge_loop_and_shift_alt_adds() {
    let (mut h, b) = mesh_harness(false);
    enter_mesh(&mut h);
    let i = near_edge(&h);
    let at = edge_mid(&h, i);
    let e = edge_pair(&h, i);
    let m = h.app.st.mm.meshes[&b].clone();
    let expect = newtopo_core::meshedit::edge_loop(&m, e[0], e[1]).unwrap();
    assert!(expect.len() > 1);
    h.click(at, Modifiers::ALT);
    assert_eq!(
        h.app.st.sel.edges.len(),
        expect.len(),
        "{:?}",
        h.app.st.sel.edges
    );
    for x in &expect {
        let k = h.app.st.mm.edges[&b].binary_search(&(x[0], x[1])).unwrap();
        assert!(
            h.app
                .st
                .sel
                .edges
                .contains(&crate::selection::EdgeSel::Index(b, k as u32))
        );
    }
    // A plain click replaces it with one edge (wait out the double-click
    // time: a double-click selects the loop too).
    h.frames(10);
    h.click(at, Modifiers::NONE);
    assert_eq!(h.app.st.sel.edges.len(), 1);
    // Shift+Alt on a crossing edge adds its loop.
    let cross = (0..h.app.st.vd.edges.len())
        .filter(|&j| {
            let p = edge_pair(&h, j);
            j != i
                && (p.contains(&e[0]) || p.contains(&e[1]))
                && !expect.contains(&p)
                && h.app.st.vd.edges[j].kind != crate::viewdata::EdgeKind::Cad
        })
        .min_by(|&a, &c| {
            let eye = h.app.st.view.camera.eye().as_vec3();
            let d = |j: usize| {
                h.app.st.vd.edges[j].path[0]
                    .lerp(h.app.st.vd.edges[j].path[1], 0.5)
                    .distance(eye)
            };
            d(a).total_cmp(&d(c))
        })
        .unwrap();
    h.frames(10);
    h.click(at, Modifiers::ALT);
    let one = h.app.st.sel.edges.len();
    h.frames(10);
    let mid = edge_mid(&h, cross);
    h.click(mid, Modifiers::ALT | Modifiers::SHIFT);
    assert!(
        h.app.st.sel.edges.len() > one,
        "Shift+Alt adds the second loop"
    );
}

#[test]
fn ctrl_click_selects_the_shortest_path() {
    let (mut h, b) = mesh_harness(false);
    enter_mesh(&mut h);
    h.key(Key::Num1, Modifiers::NONE);
    assert_eq!(h.app.st.mode, Mode::Vertex);
    let eye = h.app.st.view.camera.eye().as_vec3();
    let cam = h.app.st.camera_matrices();
    let m = h.app.st.mm.meshes[&b].clone();
    // Two visible vertices a few edges apart.
    let mut order: Vec<usize> = (0..h.app.st.vd.verts.len()).collect();
    order.sort_by(|&a, &c| {
        let d = |i: usize| h.app.st.vd.verts[i].pos.distance(eye);
        d(a).total_cmp(&d(c))
    });
    let vid = |i: usize| match h.app.st.vd.verts[i].sel {
        crate::selection::VertSel::Index(_, v) => v,
        _ => unreachable!(),
    };
    let a = order[0];
    let far = order
        .iter()
        .take(40)
        .copied()
        .filter(|&i| {
            h.app
                .st
                .vd
                .pick
                .is_visible(&cam, h.app.st.vd.verts[i].pos.as_dvec3(), 0.1)
        })
        .max_by_key(|&i| {
            newtopo_core::meshedit::shortest_path(&m, vid(a), vid(i))
                .unwrap()
                .len()
        })
        .unwrap();
    let path = newtopo_core::meshedit::shortest_path(&m, vid(a), vid(far)).unwrap();
    assert!(path.len() >= 3, "{path:?}");
    h.click(h.screen_of(h.app.st.vd.verts[a].pos), Modifiers::NONE);
    assert_eq!(h.app.st.sel.verts.len(), 1);
    h.click(h.screen_of(h.app.st.vd.verts[far].pos), Modifiers::COMMAND);
    let got: std::collections::BTreeSet<u32> = h
        .app
        .st
        .sel
        .verts
        .iter()
        .map(|v| match v {
            crate::selection::VertSel::Index(_, i) => *i,
            _ => unreachable!(),
        })
        .collect();
    let want: std::collections::BTreeSet<u32> = path.iter().copied().collect();
    assert_eq!(got, want);
}

/// Select the edge loop through the nearest interior edge.
fn select_loop(h: &mut Harness) {
    let i = near_edge(h);
    let at = edge_mid(h, i);
    h.click(at, Modifiers::ALT);
    assert!(h.app.st.sel.edges.len() > 1);
}

#[test]
fn mesh_edits_lock_cad_editing_until_discarded() {
    use newtopo_core::Op;
    use newtopo_core::ops::PatchSel;
    let (mut h, b) = mesh_harness(false);
    enter_mesh(&mut h);
    select_loop(&mut h);
    assert!(!has_edits(&h, b), "entering bakes but writes nothing");
    h.app.st.run(Cmd::MeshSpace);
    h.app.st.mm.params.relax_iterations = 3;
    h.app.st.run(Cmd::MeshRelax);
    h.frames(1);
    assert!(has_edits(&h, b), "{:?}", h.app.st.toasts.items.last());
    assert!(!h.app.st.doc.in_preview(), "no preview in mesh mode");
    h.key(Key::M, Modifiers::NONE);
    let grid = Op::Grid {
        patches: PatchSel {
            faces: vec![0],
            ..Default::default()
        },
        u: 7,
        v: 7,
        start_on_seam: false,
        spacing: newtopo_core::doc::Spacing::ArcLength,
    };
    assert!(h.app.st.apply(grid.clone()).is_none(), "refused");
    assert!(
        h.app
            .st
            .toasts
            .items
            .last()
            .unwrap()
            .text
            .contains("locked"),
        "{:?}",
        h.app.st.toasts.items.last()
    );
    assert!(!h.app.st.doc.in_preview());
    // Discard (no prompt, undoable) unlocks it.
    h.app.st.sel.objects.insert(b);
    h.app.st.run(Cmd::MeshDiscard);
    assert!(!has_edits(&h, b));
    assert!(h.app.st.apply(grid).is_some(), "CAD-editable again");
    h.app.st.discard_preview();
    h.app.st.run(Cmd::Undo);
    assert!(has_edits(&h, b), "discard is one undo step");
}

#[test]
fn undo_and_redo_work_across_modes() {
    let (mut h, b) = mesh_harness(false);
    enter_mesh(&mut h);
    select_loop(&mut h);
    h.app.st.mm.params.relax_iterations = 3;
    h.app.st.run(Cmd::MeshRelax);
    assert!(has_edits(&h, b));
    let edited = h.app.st.doc.scene.as_ref().unwrap().mesh_edit(b).unwrap();
    // Live values: redo the edit with more iterations, still one step.
    h.app.st.mm.params.relax_iterations = 25;
    h.app.st.mesh_params_changed();
    let more = h.app.st.doc.scene.as_ref().unwrap().mesh_edit(b).unwrap();
    assert_ne!(*more, *edited, "the new value took effect");
    // Back to CAD, undo there: the mesh edit (one step) is gone.
    h.key(Key::M, Modifiers::NONE);
    h.key(Key::Z, Modifiers::COMMAND);
    assert!(!has_edits(&h, b));
    h.key(Key::Y, Modifiers::COMMAND);
    assert!(has_edits(&h, b));
    // In mesh mode the view shows the redone mesh; undo there too.
    h.key(Key::M, Modifiers::NONE);
    h.frames(1);
    assert!(std::sync::Arc::ptr_eq(
        &h.app.st.mm.meshes[&b],
        &h.app.st.doc.scene.as_ref().unwrap().mesh_edit(b).unwrap()
    ));
    h.key(Key::Z, Modifiers::COMMAND);
    h.frames(1);
    assert!(!has_edits(&h, b));
    assert!(h.app.st.sel.edges.is_empty(), "stale selection is dropped");
}

#[test]
fn a_mesh_edit_survives_save_and_open() {
    let (mut h, b) = mesh_harness(false);
    enter_mesh(&mut h);
    select_loop(&mut h);
    h.app.st.run(Cmd::MeshRelax);
    assert!(has_edits(&h, b));
    let before = h.app.st.doc.scene.as_ref().unwrap().mesh_edit(b).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("edited.ntopo");
    h.app.st.doc.save(&p).unwrap();
    let mut h2 = Harness::new(false);
    let k = std::sync::Arc::new(newtopo_kernel::mock::MockKernel::new());
    h2.app.st.doc.set_scene(
        newtopo_core::scene::Scene::load(k, &p).unwrap(),
        Some(p.clone()),
    );
    h2.frames(2);
    let s2 = h2.app.st.doc.scene.as_ref().unwrap();
    let b2 = s2.body_ids()[0];
    assert!(s2.has_mesh_edits(b2));
    let after = s2.mesh_edit(b2).unwrap();
    assert_eq!(after.positions, before.positions);
    assert_eq!(after.polygons, before.polygons);
}

/// Export and the Blender bridge read `ViewData::outputs`; in mesh mode it
/// must hold the edited mesh, for hidden objects too.
#[test]
fn export_in_mesh_mode_sends_the_edited_mesh() {
    let (mut h, b) = mesh_harness(false);
    enter_mesh(&mut h);
    select_loop(&mut h);
    h.app.st.run(Cmd::MeshRelax);
    assert!(has_edits(&h, b));
    let want = h
        .app
        .st
        .doc
        .scene
        .as_mut()
        .unwrap()
        .effective_output_of(b)
        .unwrap();
    h.app.st.view.hidden.insert(b);
    h.app.st.invalidate_view();
    h.frames(1);
    assert!(h.app.st.in_mesh_mode());
    let items = h.app.st.export_items().unwrap();
    assert_eq!(items.len(), 1, "the hidden, edited object is exported");
    let got = &items[0].4;
    assert_eq!(got.body, b);
    let pos = |o: &newtopo_core::mesh::OutputMesh| -> Vec<glam::DVec3> {
        o.vertices.iter().map(|v| v.position).collect()
    };
    assert_eq!(pos(got), pos(&want));
    assert_eq!(got.polygons.len(), want.polygons.len());
}

/// Merge by Distance redone with "Selected only" switched: the scope
/// follows the box, from the vertices selected when it ran.
#[test]
fn merge_by_distance_redo_follows_selected_only() {
    use crate::selection::VertSel;
    let (mut h, b) = mesh_harness(false);
    enter_mesh(&mut h);
    let m = (*h.app.st.mm.meshes[&b]).clone();
    // Two far-apart edges between free vertices; squeeze each to a near
    // duplicate pair.
    let flagged: std::collections::BTreeSet<u32> =
        m.flagged.iter().flat_map(|e| [e.a, e.b]).collect();
    let free = |v: u32| !m.pinned[v as usize] && !flagged.contains(&v);
    let ring = |v: u32| -> std::collections::BTreeSet<u32> {
        m.polygons
            .iter()
            .filter(|p| p.verts.contains(&v))
            .flat_map(|p| p.verts.iter().copied())
            .collect()
    };
    let edges: Vec<(u32, u32)> = m
        .edges()
        .into_iter()
        .filter(|&(a, c)| free(a) && free(c))
        .collect();
    let e1 = edges[0];
    let near1: std::collections::BTreeSet<u32> = ring(e1.0).union(&ring(e1.1)).copied().collect();
    let e2 = *edges
        .iter()
        .find(|&&(a, c)| !near1.contains(&a) && !near1.contains(&c))
        .unwrap();
    let mut squeezed = m.clone();
    for (a, c) in [e1, e2] {
        let (pa, pc) = (m.positions[a as usize], m.positions[c as usize]);
        squeezed.positions[a as usize] = pc + (pa - pc) * 0.001;
    }
    h.app
        .st
        .doc
        .set_meshes(vec![(b, Some(squeezed))], false)
        .unwrap();
    h.app.st.invalidate_view();
    h.frames(1);
    let used = |h: &Harness| {
        let e = h.app.st.doc.scene.as_ref().unwrap().mesh_edit(b).unwrap();
        let mut u = std::collections::BTreeSet::new();
        e.polygons
            .iter()
            .for_each(|p| u.extend(p.verts.iter().copied()));
        u.len()
    };
    let n0 = used(&h);
    // Only the first pair is selected.
    h.app.st.mode = Mode::Vertex;
    h.app.st.sel = Default::default();
    h.app.st.sel.verts.insert(VertSel::Index(b, e1.0));
    h.app.st.sel.verts.insert(VertSel::Index(b, e1.1));
    h.app.st.mm.params.merge_selected = false;
    h.app.st.mm.params.merge_pct = 0.05;
    h.app.st.run(Cmd::MeshMergeByDistance);
    assert_eq!(used(&h), n0 - 2, "whole mesh: both pairs merge");
    // Tick "Selected only": the redo merges the selected pair only.
    h.app.st.mm.params.merge_selected = true;
    h.app.st.mesh_params_changed();
    assert_eq!(used(&h), n0 - 1, "selected only: one pair");
    // And back.
    h.app.st.mm.params.merge_selected = false;
    h.app.st.mesh_params_changed();
    assert_eq!(used(&h), n0 - 2);
}
