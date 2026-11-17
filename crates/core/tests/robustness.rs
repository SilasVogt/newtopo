//! Save/load robustness (review round 3): phase overrides survive a round
//! trip, failed saves keep the previous file, hostile archives are refused
//! and malformed scenes are rejected with an error instead of a panic.
mod common;
use common::*;
use newtopo_core::ops::PatchSel;
use newtopo_core::{Op, Scene};
use newtopo_kernel::mock::{MockBody, MockKernel};
use newtopo_kernel::{BodyAttributes, CadKernel, HealReport};
use std::sync::Arc;

fn connect(f: u32, a: u32) -> Op {
    Op::Connect {
        patches: PatchSel {
            faces: vec![f],
            ..Default::default()
        },
        a,
        b: 0,
        start_on_seam: false,
    }
}

/// Finding 1: Link on periodic faces fills `phase_overrides`, which must
/// still save and load.
#[test]
fn phase_overrides_round_trip() {
    let (mut s, b) = scene_with(MockBody::plate_with_holes(2, 2));
    s.apply(Op::SetLink { on: true }).unwrap();
    s.apply(connect(6, 8)).unwrap();
    assert!(
        !s.doc().phase_overrides.is_empty(),
        "linking hole walls maps phase references"
    );
    let before = s.output_of(b).unwrap().clone();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("linked.ntopo");
    s.save(&path).unwrap();
    let mut s2 = Scene::load(Arc::new(MockKernel::new()), &path).unwrap();
    assert_eq!(s2.doc().phase_overrides, s.doc().phase_overrides);
    assert_eq!(s2.doc(), s.doc());
    let b2 = s2.body_ids()[0];
    assert_eq!(s2.output_of(b2).unwrap().stats(), before.stats());
}

/// Finding 2: a save that fails part-way leaves the previous file intact
/// and no temporary files behind.
#[test]
fn failed_save_keeps_previous_file() {
    let k = Arc::new(MockKernel::new());
    let mut s = Scene::new(k.clone());
    let b = s
        .add_body(
            &MockBody::cylinder(2.0, 5.0).to_brep_bytes(),
            BodyAttributes::default(),
            HealReport::default(),
        )
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keep.ntopo");
    s.save(&path).unwrap();
    let good = std::fs::read(&path).unwrap();
    // Saving again overwrites an existing file.
    s.apply(connect(0, 6)).unwrap();
    s.save(&path).unwrap();
    let good2 = std::fs::read(&path).unwrap();
    assert_ne!(good, good2);
    // Make serialisation fail: the kernel no longer knows the body.
    s.apply(connect(0, 4)).unwrap();
    k.unload(b);
    assert!(s.save(&path).is_err());
    assert!(s.save_copy(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), good2, "previous file kept");
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "no temporary files left: {names:?}");
    assert!(Scene::load(Arc::new(MockKernel::new()), &path).is_ok());
}
