//! Preview transactions: every mesh change previews first and is written
//! only on Apply (Silas, user testing 2026-11-18).
mod common;
use common::*;
use newtopo_core::Op;
use newtopo_core::doc::Spacing;
use newtopo_core::ops::PatchSel;
use newtopo_kernel::mock::MockBody;

fn grid(u: u32) -> Op {
    Op::Grid {
        patches: PatchSel {
            faces: vec![0],
            ..Default::default()
        },
        u,
        v: u,
        start_on_seam: false,
        spacing: Spacing::ArcLength,
    }
}

fn verts(s: &mut newtopo_core::Scene, b: newtopo_kernel::BodyId) -> usize {
    s.output_of(b).unwrap().vertices.len()
}

#[test]
fn cancel_restores_document_and_history() {
    let (mut s, b) = scene_with(MockBody::make_box(glam::DVec3::new(4.0, 3.0, 2.0)));
    s.apply(grid(2)).unwrap();
    let applied = verts(&mut s, b);
    assert!(s.undo());
    let base = verts(&mut s, b);
    assert!(s.can_redo(), "one step to redo");

    s.begin_preview();
    assert!(s.in_preview());
    s.apply(grid(3)).unwrap();
    s.apply(grid(5)).unwrap();
    assert!(verts(&mut s, b) > base, "the preview shows the change");
    assert!(!s.can_redo(), "no redo while previewing");
    s.cancel_preview();
    assert!(!s.in_preview());
    assert_eq!(verts(&mut s, b), base, "back to the document before");
    assert!(
        s.can_redo(),
        "the redo history survives a cancelled preview"
    );
    assert!(s.redo());
    assert_eq!(verts(&mut s, b), applied);
}

#[test]
fn commit_is_one_undo_step() {
    let (mut s, b) = scene_with(MockBody::make_box(glam::DVec3::new(4.0, 3.0, 2.0)));
    let base = verts(&mut s, b);
    s.begin_preview();
    s.apply(grid(2)).unwrap();
    s.apply(grid(4)).unwrap();
    s.apply(Op::SetAutofill { on: true }).unwrap();
    let after = verts(&mut s, b);
    s.commit_preview();
    assert!(!s.in_preview());
    assert_eq!(verts(&mut s, b), after);
    assert!(s.undo(), "one undo step");
    assert_eq!(verts(&mut s, b), base, "undoes the whole preview at once");
    assert!(s.redo());
    assert_eq!(verts(&mut s, b), after, "and redoes it as one step");
    assert!(!s.can_redo());
}

#[test]
fn undo_while_previewing_discards_the_preview() {
    let (mut s, b) = scene_with(MockBody::make_box(glam::DVec3::new(4.0, 3.0, 2.0)));
    let base = verts(&mut s, b);
    s.begin_preview();
    assert!(s.can_undo(), "a preview can always be undone");
    s.apply(grid(3)).unwrap();
    assert!(s.undo());
    assert!(!s.in_preview());
    assert_eq!(verts(&mut s, b), base);
}

#[test]
fn which_ops_preview() {
    assert!(grid(2).changes_mesh());
    assert!(Op::SetAutofill { on: true }.changes_mesh());
    assert!(
        !Op::RenameLayer {
            layer: newtopo_core::doc::LayerId(0),
            name: "x".into()
        }
        .changes_mesh()
    );
}

#[test]
fn preview_base_shows_the_document_before() {
    let (mut s, b) = scene_with(MockBody::make_box(glam::DVec3::new(4.0, 3.0, 2.0)));
    s.apply(grid(3)).unwrap();
    let base = s.stats();
    s.begin_preview();
    s.apply(Op::SetAutofill { on: true }).unwrap();
    let after = s.stats();
    let seen = s.with_preview_base(|s| s.stats()).unwrap();
    assert_eq!(seen, base, "the base view evaluates the old document");
    assert_eq!(s.stats(), after, "and the preview is back afterwards");
    assert!(s.in_preview());
    let _ = b;
}
