//! Save/load robustness (review round 3): phase overrides survive a round
//! trip, failed saves keep the previous file, hostile archives are refused
//! and malformed scenes are rejected with an error instead of a panic.
mod common;
use common::*;
use glam::{DVec2, DVec3};
use newtopo_core::doc::{
    CutPoint, Document, Edit, EditEntry, IsoDir, PatchId, PhaseTarget, Rule, RuleEntry, Spacing,
    SplitKind, SplitLine,
};
use newtopo_core::keys::{RuleId, SplitId, VertexKey};
use newtopo_core::ops::PatchSel;
use newtopo_core::scene::LoadLimits;
use newtopo_core::{Op, Scene};
use newtopo_kernel::mock::{MockBody, MockKernel};
use newtopo_kernel::{BodyAttributes, BodyId, CadKernel, EdgeId, FaceId, HealReport, VertexId};
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

/// Write a `.ntopo`-shaped zip from raw entries.
fn write_zip(path: &std::path::Path, entries: &[(&str, &[u8])]) {
    use std::io::Write;
    let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        z.start_file(*name, opts).unwrap();
        z.write_all(bytes).unwrap();
    }
    z.finish().unwrap();
}

fn small_limits() -> LoadLimits {
    LoadLimits {
        max_entries: 16,
        max_scene_json: 1 << 20,
        max_brep: 1 << 20,
        max_total: 1 << 21,
    }
}

/// Finding 3: oversized entries, too many entries and an oversized total
/// are refused with errors, without decompressing everything.
#[test]
fn zip_bomb_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let k = || -> Arc<dyn CadKernel> { Arc::new(MockKernel::new()) };
    // scene.json that inflates to 4 MiB (a few KiB compressed).
    let bomb = dir.path().join("bomb.ntopo");
    write_zip(&bomb, &[("scene.json", &vec![b' '; 4 << 20])]);
    let e = Scene::load_with_limits(k(), &bomb, &small_limits())
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("scene.json is larger"), "{e:#}");
    // Too many entries.
    let many = dir.path().join("many.ntopo");
    let names: Vec<String> = (0..20).map(|i| format!("junk/{i}")).collect();
    let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
    write_zip(&many, &entries);
    let e = Scene::load_with_limits(k(), &many, &small_limits())
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("20 entries"), "{e:#}");
    // A real scene whose bodies together exceed the total budget.
    let mut s = Scene::new(k());
    for _ in 0..3 {
        s.add_body(
            &MockBody::plate_with_holes(4, 4).to_brep_bytes(),
            BodyAttributes::default(),
            HealReport::default(),
        )
        .unwrap();
    }
    let real = dir.path().join("real.ntopo");
    s.save(&real).unwrap();
    assert!(Scene::load(k(), &real).is_ok());
    let brep = MockBody::plate_with_holes(4, 4).to_brep_bytes().len() as u64;
    let tight = LoadLimits {
        max_total: 2 * brep + brep / 2,
        ..LoadLimits::default()
    };
    let e = Scene::load_with_limits(k(), &real, &tight).err().unwrap();
    assert!(format!("{e:#}").contains("decompresses to more"), "{e:#}");
    let e = Scene::load_with_limits(
        k(),
        &real,
        &LoadLimits {
            max_brep: brep - 1,
            ..LoadLimits::default()
        },
    )
    .err()
    .unwrap();
    assert!(format!("{e:#}").contains(".bbrep is larger"), "{e:#}");
}

/// Finding 4: a loadable scene with out-of-range CAD ids is refused with a
/// clear error instead of panicking during evaluation.
#[test]
fn malformed_scene_is_refused() {
    let (s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
    let brep = MockBody::cylinder(2.0, 5.0).to_brep_bytes();
    let dir = tempfile::tempdir().unwrap();
    let bad = |doc: &Document| -> String {
        let path = dir.path().join("bad.ntopo");
        let json = serde_json::to_vec(doc).unwrap();
        let name = format!("cad/{}.bbrep", b.0);
        write_zip(&path, &[("scene.json", &json), (&name, &brep)]);
        let e = Scene::load(Arc::new(MockKernel::new()), &path)
            .err()
            .unwrap();
        format!("{e:#}")
    };
    let with_rule = |rule: Rule| -> Document {
        let mut d = s.doc().clone();
        d.rules.push(RuleEntry {
            id: RuleId(900),
            ordinal: 900,
            rule,
            link_group: None,
        });
        d
    };
    let msg = bad(&with_rule(Rule::Cut {
        points: vec![
            CutPoint::CadEdge {
                body: b,
                edge: EdgeId(u32::MAX),
                t: 0.5,
            },
            CutPoint::Face {
                body: b,
                face: FaceId(0),
                uv: DVec2::new(0.5, 0.5),
            },
        ],
        closed: false,
    }));
    assert!(msg.contains("CAD edge 4294967295"), "{msg}");
    let msg = bad(&with_rule(Rule::Grid {
        patch: PatchId::whole(b, FaceId(77)),
        count_u: 2,
        count_v: 2,
        start_on_seam: false,
        spacing: Spacing::ArcLength,
    }));
    assert!(msg.contains("face 77"), "{msg}");
    let msg = bad(&with_rule(Rule::Grid {
        patch: PatchId::whole(b, FaceId(0)),
        count_u: u32::MAX,
        count_v: 2,
        start_on_seam: false,
        spacing: Spacing::ArcLength,
    }));
    assert!(msg.contains("lines"), "{msg}");
    let msg = bad(&with_rule(Rule::Join {
        body: BodyId(4242),
        a: VertexKey::Cad(VertexId(0)),
        b: VertexKey::Cad(VertexId(1)),
    }));
    assert!(msg.contains("body 4242"), "{msg}");
    let mut d = s.doc().clone();
    d.edits.push(EditEntry {
        id: 901,
        edit: Edit::Weld {
            body: b,
            from: VertexKey::Cad(VertexId(0)),
            to: VertexKey::Cad(VertexId(999)),
        },
    });
    assert!(bad(&d).contains("CAD vertex 999"));
    // Settings that size allocations are bounded too.
    let mut d = s.doc().clone();
    d.settings.min_closed_segments = u32::MAX;
    assert!(bad(&d).contains("min_closed_segments"));
    let mut d = s.doc().clone();
    d.settings.sharp_angle_deg = 1e9;
    assert!(bad(&d).contains("sharp angle"));
    let mut d = s.doc().clone();
    d.settings.units = "furlong".into();
    assert!(bad(&d).contains("unit"));
    let mut d = s.doc().clone();
    d.split_lines.push(SplitLine {
        id: SplitId(902),
        group: 0,
        body: b,
        face: FaceId(55),
        kind: SplitKind::Iso {
            dir: IsoDir::U,
            value: 0.5,
            seed: None,
        },
    });
    assert!(bad(&d).contains("face 55"));
    let mut d = s.doc().clone();
    d.phase_overrides
        .insert(PhaseTarget::Edge(b, EdgeId(500)), DVec3::X);
    assert!(bad(&d).contains("CAD edge 500"));
    // The same check guards try_set_document.
    let bad_doc = with_rule(Rule::Cut {
        points: vec![CutPoint::CadEdge {
            body: b,
            edge: EdgeId(u32::MAX),
            t: 0.5,
        }],
        closed: false,
    });
    let mut s = s;
    assert!(s.try_set_document(bad_doc).is_err());
    let ok = s.doc().clone();
    s.try_set_document(ok).unwrap();
}

/// Sol round 2, fix 3: a dropped scene unloads the kernel bodies it owns
/// (and only those), and failed loads leave nothing loaded.
#[test]
fn scene_drop_unloads_bodies() {
    let k = Arc::new(MockKernel::new());
    let add = |s: &mut Scene, body: MockBody| {
        s.add_body(
            &body.to_brep_bytes(),
            BodyAttributes::default(),
            HealReport::default(),
        )
        .unwrap()
    };
    let mut s1 = Scene::new(k.clone());
    let a = add(&mut s1, MockBody::cylinder(2.0, 5.0));
    add(&mut s1, MockBody::make_box(DVec3::ONE));
    let mut s2 = Scene::new(k.clone());
    let c = add(&mut s2, MockBody::cylinder(1.0, 2.0));
    assert_eq!(k.body_count(), 3);
    // Replacing keeps the old body for undo; both belong to s1.
    s1.replace_body(
        a,
        &MockBody::cylinder(2.0, 5.0).to_brep_bytes(),
        BodyAttributes::default(),
        HealReport::default(),
    )
    .unwrap();
    assert_eq!(k.body_count(), 4);
    // A failed add leaves nothing behind and never unloads the scene's own
    // body (the duplicate id is s2's).
    assert!(
        s1.add_body(
            b"not a brep",
            BodyAttributes::default(),
            HealReport::default()
        )
        .is_err()
    );
    assert!(
        s2.add_kernel_body(c, BodyAttributes::default(), HealReport::default(), None)
            .is_err()
    );
    assert_eq!(k.body_count(), 4);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s2.ntopo");
    s2.save(&path).unwrap();
    drop(s1);
    assert_eq!(k.body_count(), 1, "only s1's bodies are unloaded");
    assert!(s2.output_of(c).is_some(), "s2 still works");
    drop(s2);
    assert_eq!(k.body_count(), 0);

    // Loads that fail after loading bodies unload them again.
    let good = Scene::load(k.clone(), &path).unwrap();
    assert_eq!(k.body_count(), 1);
    let mut doc = good.doc().clone();
    drop(good);
    assert_eq!(k.body_count(), 0);
    let b = doc.bodies[0].id;
    let brep = MockBody::cylinder(1.0, 2.0).to_brep_bytes();
    let name = format!("cad/{}.bbrep", b.0);
    doc.phase_overrides
        .insert(PhaseTarget::Edge(b, EdgeId(999)), DVec3::X);
    let bad = dir.path().join("bad.ntopo");
    write_zip(
        &bad,
        &[
            ("scene.json", &serde_json::to_vec(&doc).unwrap()),
            (&name, &brep),
        ],
    );
    assert!(Scene::load(k.clone(), &bad).is_err());
    assert_eq!(k.body_count(), 0, "refused scene unloaded its bodies");
    // Second body's CAD data missing: the first one is unloaded again.
    let mut two = doc.clone();
    two.phase_overrides.clear();
    let mut extra = two.bodies[0].clone();
    extra.id = BodyId(b.0 + 100);
    two.bodies.push(extra);
    write_zip(
        &bad,
        &[
            ("scene.json", &serde_json::to_vec(&two).unwrap()),
            (&name, &brep),
        ],
    );
    let e = Scene::load(k.clone(), &bad).err().unwrap();
    assert!(format!("{e:#}").contains("missing"), "{e:#}");
    assert_eq!(k.body_count(), 0);
}

/// Saved scenes get normal file permissions (not the temp file's 0600), and
/// re-saving keeps whatever mode the user set.
#[cfg(unix)]
#[test]
fn saved_scene_has_normal_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let k = Arc::new(MockKernel::new());
    let mut s = Scene::new(k);
    s.add_body(
        &MockBody::cylinder(2.0, 5.0).to_brep_bytes(),
        BodyAttributes::default(),
        HealReport::default(),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("perm.ntopo");
    s.save(&path).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_ne!(mode, 0o600, "a new scene must not be private-only");
    assert_eq!(mode & 0o400, 0o400);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    s.save(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

/// Geometry is stored in the scene's unit, so it can't change under bodies.
#[test]
fn units_are_fixed_once_the_scene_has_bodies() {
    let k = Arc::new(MockKernel::new());
    let mut s = Scene::new(k);
    s.apply(Op::SetUnits { units: "cm".into() }).unwrap();
    assert_eq!(s.doc().settings.units, "cm");
    assert!(
        s.apply(Op::SetUnits {
            units: "furlong".into()
        })
        .is_err()
    );
    s.add_body(
        &MockBody::cylinder(2.0, 5.0).to_brep_bytes(),
        BodyAttributes::default(),
        HealReport::default(),
    )
    .unwrap();
    assert!(s.apply(Op::SetUnits { units: "m".into() }).is_err());
    s.apply(Op::SetUnits { units: "cm".into() }).unwrap();
}
