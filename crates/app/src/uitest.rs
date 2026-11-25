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

// ------------------------------------------------------------ problem reports (SPEC §5.11)

fn report_origin(h: &Harness) -> Option<crate::report::ui::Origin> {
    h.app.st.ui.report.as_ref().map(|d| d.origin)
}

#[test]
fn report_opens_once_after_a_caught_panic() {
    use crate::jobs::Modal;
    use crate::report::ui::Origin;
    fn boom() {
        panic!("engine blew up")
    }
    let mut h = Harness::new(false);
    let r: Result<(), String> = crate::docstate::guard(boom);
    assert!(r.is_err());
    h.frames(2);
    assert_eq!(report_origin(&h), Some(Origin::Caught));
    assert_eq!(h.app.st.active_modal(), Some(Modal::Report));
    let d = h.app.st.ui.report.as_ref().unwrap();
    assert_eq!(d.failure().unwrap().message, "engine blew up");
    // A second failure while it is open doesn't replace or stack it.
    let _ = crate::docstate::guard(|| panic!("another one"));
    h.frames(2);
    let d = h.app.st.ui.report.as_ref().unwrap();
    assert_eq!(d.failure().unwrap().message, "engine blew up");
    // Esc closes it; the same failure again doesn't reopen it.
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.app.st.ui.report.is_none());
    let _ = crate::docstate::guard(boom);
    h.frames(2);
    assert!(h.app.st.ui.report.is_none(), "once per failure");
    // A new failure does.
    let _ = crate::docstate::guard(|| panic!("a different failure"));
    h.frames(2);
    assert_eq!(report_origin(&h), Some(Origin::Caught));
}

#[test]
fn refusals_do_not_open_the_report() {
    use newtopo_core::Op;
    let mut h = Harness::new(false);
    h.app.st.doc.set_scene(
        newtopo_core::scene::Scene::new(newtopo_kernel::default_kernel()),
        None,
    );
    h.frames(1);
    let r = h.app.st.apply(Op::DeleteRule {
        rule: newtopo_core::keys::RuleId(4242),
    });
    assert!(r.is_none(), "refused");
    h.frames(3);
    assert!(h.app.st.ui.report.is_none());
    assert!(!h.app.st.toasts.items.is_empty(), "the refusal is a toast");
    // It is in the session's steps, as refused.
    let steps = h.app.st.session.steps_json();
    assert!(steps.contains("\"refused\""), "{steps}");
    assert_eq!(h.app.st.session.op_count(), 0);
}

#[test]
fn report_from_help_has_no_error_and_never_stacks() {
    use crate::report::ui::Origin;
    let mut h = Harness::new(true);
    h.app.st.run(Cmd::ReportProblem);
    h.frames(2);
    assert_eq!(report_origin(&h), Some(Origin::Manual));
    assert!(h.app.st.ui.report.as_ref().unwrap().failure().is_none());
    h.app.st.ui.report.as_mut().unwrap().choices.comment = "typed".into();
    // Opening again (palette, menu) keeps the open one.
    h.app.st.run(Cmd::ReportProblem);
    assert_eq!(
        h.app.st.ui.report.as_ref().unwrap().choices.comment,
        "typed"
    );
    // The palette lists it.
    assert!(crate::commands::ALL.contains(&Cmd::ReportProblem));
}

#[test]
fn saving_writes_the_listed_files_and_issue_links_them() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(false);
    let _ = crate::docstate::guard(|| panic!("save test failure"));
    h.frames(2);
    let path = dir.path().join("report.zip");
    {
        let d = h.app.st.ui.report.as_mut().expect("open");
        d.save_to = Some(path.clone());
        d.choices.pc = false;
    }
    // Open GitHub issue saves first, then opens the page once.
    h.app.st.open_issue();
    assert!(path.exists(), "saved before opening the issue");
    assert_eq!(h.app.st.opened_urls.len(), 1);
    let url = &h.app.st.opened_urls[0];
    assert!(url.starts_with(crate::report::ISSUE_URL));
    assert!(url.len() <= crate::report::MAX_ISSUE_URL);
    assert!(url.contains("report.zip"));
    let d = h.app.st.ui.report.as_ref().unwrap();
    let listed: Vec<String> = d
        .rows()
        .into_iter()
        .filter(|r| r.included)
        .map(|r| r.name)
        .collect();
    let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    let inside: Vec<String> = (0..z.len())
        .map(|i| z.by_index(i).unwrap().name().unwrap().to_string())
        .collect();
    assert_eq!(inside, listed);
    assert!(!inside.iter().any(|n| n == "system.txt"), "PC details off");
}

#[test]
fn previous_crash_is_offered_at_start() {
    use crate::report::crash;
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config: None,
        data: Some(dir.path().to_path_buf()),
    };
    // A session that crashed: marker left, crash file written.
    assert!(crash::begin_session(dir.path()).is_none());
    crash::write_crash_file(dir.path(), &crash::Failure::from_message("hard crash"));
    let ctx = egui::Context::default();
    let app = App::new(&ctx, paths.clone(), false, false);
    let d = app.st.ui.report.as_ref().expect("offered");
    assert_eq!(d.origin, crate::report::ui::Origin::PreviousCrash);
    assert_eq!(d.failure().unwrap().message, "hard crash");
    // This session is running now; a clean exit means no offer next time.
    crash::end_session(dir.path());
    let app = App::new(&ctx, paths, false, false);
    assert!(app.st.ui.report.is_none());
}
