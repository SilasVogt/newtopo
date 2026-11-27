//! Mesh mode on the scene (SPEC §9.5): edited meshes in the document, the
//! CAD lock, discarding, amending the last edit, undo, save/load (format 2
//! and the format 1 migration) and the effective output that export uses.
mod common;
use common::*;
use newtopo_core::Op;
use newtopo_core::doc::Spacing as GridSpacing;
use newtopo_core::meshedit::*;
use newtopo_core::ops::PatchSel;
use newtopo_kernel::BodyId;
use newtopo_kernel::mock::{MockBody, MockKernel};
use std::io::{Read, Write};
use std::sync::Arc;

fn grid(face: u32, n: u32) -> Op {
    Op::Grid {
        patches: PatchSel {
            faces: vec![face],
            ..Default::default()
        },
        u: n,
        v: n,
        start_on_seam: false,
        spacing: GridSpacing::ArcLength,
    }
}

/// A box with a grid on every face, auto-fill on.
fn gridded_box() -> (newtopo_core::Scene, BodyId) {
    let (mut s, b) = scene_with(MockBody::make_box(glam::DVec3::new(4.0, 3.0, 2.0)));
    set_autofill(&mut s, true);
    for f in 0..6 {
        s.apply(grid(f, 4)).unwrap();
    }
    (s, b)
}

/// A small, valid edit of a baked mesh: move every free vertex a little
/// off its face along the face normal.
fn edited(m: &EditMesh) -> EditMesh {
    let mut out = m.clone();
    let mut push = vec![glam::DVec3::ZERO; m.positions.len()];
    for p in &m.polygons {
        for (k, &v) in p.verts.iter().enumerate() {
            push[v as usize] += p.normals[k];
        }
    }
    for (i, p) in out.positions.iter_mut().enumerate() {
        if !m.pinned[i] {
            *p += push[i].normalize_or_zero() * 0.01;
        }
    }
    out
}

#[test]
fn entering_changes_nothing_and_the_first_edit_is_one_undo_step() {
    let (mut s, b) = gridded_box();
    let before = s.doc().clone();
    let m = s.editable_mesh(b).unwrap();
    assert_eq!(
        *s.doc(),
        before,
        "baking for mesh mode doesn't touch the document"
    );
    assert!(!s.has_mesh_edits(b));
    assert!(
        Arc::ptr_eq(&m, &s.editable_mesh(b).unwrap()),
        "bake is cached"
    );

    s.set_meshes(vec![(b, Some(edited(&m)))]).unwrap();
    assert!(s.has_mesh_edits(b));
    assert_eq!(*s.editable_mesh(b).unwrap(), edited(&m));
    // The effective output (statistics, export) is the edited mesh.
    let out = s.output()[0].clone();
    assert_eq!(out.vertices.len(), m.positions.len());
    assert!(
        out.vertices
            .iter()
            .zip(&edited(&m).positions)
            .all(|(v, p)| v.position == *p)
    );
    assert_eq!(s.effective_output_of(b).unwrap(), out);
    assert!(s.validate(b).is_empty());
    s.undo();
    assert!(!s.has_mesh_edits(b));
    assert_eq!(*s.doc(), before);
    s.redo();
    assert!(s.has_mesh_edits(b));
}

#[test]
fn mesh_edits_lock_cad_editing_until_discarded() {
    let (mut s, b) = gridded_box();
    let m = s.editable_mesh(b).unwrap();
    s.set_meshes(vec![(b, Some(edited(&m)))]).unwrap();
    let r = s.apply(grid(0, 6)).unwrap_err();
    assert!(r.reason.contains("locked"), "{}", r.reason);
    assert!(r.reason.contains("Discard mesh edits"));
    // Scene settings and names still change.
    s.apply(Op::RenameBody {
        body: b,
        name: "Edited".into(),
    })
    .unwrap();
    s.apply(Op::SetAutofill { on: false }).unwrap();
    // Discard: CAD-driven again, editable again, one undo step.
    assert_eq!(s.discard_mesh_edits(&[b]).unwrap(), 1);
    assert!(!s.has_mesh_edits(b));
    s.apply(grid(0, 6)).unwrap();
    s.undo();
    s.undo();
    assert!(s.has_mesh_edits(b), "undo brings the mesh edits back");
    assert_eq!(s.discard_mesh_edits(&[]).unwrap(), 0);
}

/// Import "Replace" gives the body a new id; the edited mesh belongs to
/// the old one. Refused while the body has mesh edits, so no mesh is left
/// keyed by a body the scene no longer has (which would make the saved file
/// unopenable).
#[test]
fn replacing_a_body_with_mesh_edits_is_refused_until_discarded() {
    let (mut s, b) = gridded_box();
    let m = s.editable_mesh(b).unwrap();
    s.set_meshes(vec![(b, Some(edited(&m)))]).unwrap();
    let before = s.doc().clone();
    let brep = MockBody::make_box(glam::DVec3::new(4.0, 3.0, 2.0)).to_brep_bytes();
    let attrs = || newtopo_kernel::BodyAttributes {
        name: "Box v2".into(),
        transform: glam::DMat4::IDENTITY,
        ..Default::default()
    };
    let e = s
        .replace_body_with_report(b, &brep, attrs(), Default::default())
        .unwrap_err();
    assert!(format!("{e:#}").contains("Discard mesh edits"), "{e:#}");
    assert_eq!(*s.doc(), before, "nothing changed");
    // Saving and loading still work.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("r.ntopo");
    s.save(&p).unwrap();
    newtopo_core::Scene::load(Arc::new(MockKernel::new()), &p).unwrap();
    // After discarding, the replacement goes through and leaves no mesh.
    s.discard_mesh_edits(&[b]).unwrap();
    let nb = s
        .replace_body_with_report(b, &brep, attrs(), Default::default())
        .unwrap()
        .new_body
        .unwrap();
    assert!(s.mesh_edited_bodies().is_empty());
    assert!(s.doc().body(nb).is_some());
    s.save(&p).unwrap();
    newtopo_core::Scene::load(Arc::new(MockKernel::new()), &p).unwrap();
}

/// A rule can name patches of two bodies (the document allows it):
/// replacing one of them must not rewrite that rule while the other has
/// mesh edits, because that changes the other's (locked) CAD stage.
#[test]
fn replacing_a_body_never_rewrites_a_rule_of_a_mesh_edited_body() {
    let attrs = |name: &str| newtopo_kernel::BodyAttributes {
        name: name.into(),
        transform: glam::DMat4::IDENTITY,
        ..Default::default()
    };
    let cyl = || MockBody::cylinder(2.0, 5.0);
    let mut s = newtopo_core::Scene::new(Arc::new(MockKernel::new()));
    let a = s
        .add_body(&cyl().to_brep_bytes(), attrs("a"), Default::default())
        .unwrap();
    let moved = glam::DAffine3::from_translation(glam::DVec3::new(10.0, 0.0, 0.0));
    let b = s
        .add_body(
            &cyl().transformed(moved).to_brep_bytes(),
            attrs("b"),
            Default::default(),
        )
        .unwrap();
    s.apply(Op::Connect {
        patches: PatchSel {
            body: Some(a),
            faces: vec![0],
            ..Default::default()
        },
        a: 6,
        b: 0,
        start_on_seam: false,
    })
    .unwrap();
    // Let the Connect span both bodies: add b's copy of the patch.
    let dir = tempfile::tempdir().unwrap();
    let (p, q) = (dir.path().join("one.ntopo"), dir.path().join("two.ntopo"));
    s.save(&p).unwrap();
    rewrite(&p, &q, |name, bytes| {
        if name != "scene.json" {
            return Some(bytes);
        }
        let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let rule = v["rules"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r["rule"].get("Connect").is_some())
            .unwrap();
        let list = rule["rule"]["Connect"]["patches"].as_array_mut().unwrap();
        let mut other = list[0].clone();
        other["body"] = serde_json::json!(b.0);
        list.push(other);
        Some(serde_json::to_vec(&v).unwrap())
    });
    let mut s = newtopo_core::Scene::load(Arc::new(MockKernel::new()), &q).unwrap();
    let m = s.editable_mesh(b).unwrap();
    s.set_meshes(vec![(b, Some(edited(&m)))]).unwrap();
    let before = s.doc().clone();
    let e = s
        .replace_body_with_report(a, &cyl().to_brep_bytes(), attrs("a v2"), Default::default())
        .unwrap_err();
    assert!(format!("{e:#}").contains("\"b\" has mesh edits"), "{e:#}");
    assert_eq!(*s.doc(), before, "nothing changed");
    // Without b's mesh edits the replacement goes through.
    s.discard_mesh_edits(&[b]).unwrap();
    s.replace_body_with_report(a, &cyl().to_brep_bytes(), attrs("a v2"), Default::default())
        .unwrap();
}

#[test]
fn amend_replaces_the_last_edit_only_while_nothing_else_changed() {
    let (mut s, b) = gridded_box();
    let m = s.editable_mesh(b).unwrap();
    let undo_before = s.can_undo();
    s.set_meshes(vec![(b, Some(edited(&m)))]).unwrap();
    assert!(s.can_amend_meshes());
    let twice = edited(&edited(&m));
    s.amend_meshes(vec![(b, Some(twice.clone()))]).unwrap();
    assert_eq!(*s.mesh_edit(b).unwrap(), twice);
    // Still one undo step back to before the edit.
    s.undo();
    assert!(!s.has_mesh_edits(b));
    assert_eq!(s.can_undo(), undo_before);
    // After undo (or any other change) amending is refused.
    assert!(!s.can_amend_meshes());
    assert!(s.amend_meshes(vec![(b, Some(twice))]).is_err());
}

/// The camera is not an edit: storing it (save, autosave) keeps the last
/// mesh edit adjustable, and the amend stays one undo step.
#[test]
fn storing_the_camera_keeps_the_last_edit_amendable() {
    let (mut s, b) = gridded_box();
    let m = s.editable_mesh(b).unwrap();
    s.set_meshes(vec![(b, Some(edited(&m)))]).unwrap();
    let cam = newtopo_core::doc::Camera {
        yaw: 1.2,
        ..Default::default()
    };
    s.set_camera(cam);
    assert!(s.can_amend_meshes());
    let dir = tempfile::tempdir().unwrap();
    s.save(&dir.path().join("c.ntopo")).unwrap();
    assert!(s.can_amend_meshes(), "saving changes nothing either");
    let twice = edited(&edited(&m));
    s.amend_meshes(vec![(b, Some(twice.clone()))]).unwrap();
    assert_eq!(*s.mesh_edit(b).unwrap(), twice);
    assert_eq!(s.doc().camera, cam);
    s.undo();
    assert!(!s.has_mesh_edits(b), "still one step");
}

/// A mesh step that changes nothing makes no undo step, and the step
/// before it can no longer be amended (the latest edit isn't its own).
#[test]
fn an_unchanged_result_is_no_step_and_ends_the_previous_amend() {
    let (mut s, b) = gridded_box();
    let m = s.editable_mesh(b).unwrap();
    let e = edited(&m);
    assert!(s.set_meshes(vec![(b, Some(e.clone()))]).unwrap());
    assert!(s.can_amend_meshes());
    assert!(!s.set_meshes(vec![(b, Some(e.clone()))]).unwrap());
    assert!(!s.can_amend_meshes());
    assert!(s.amend_meshes(vec![(b, Some(edited(&e)))]).is_err());
    assert_eq!(*s.mesh_edit(b).unwrap(), e);
}

/// A body without mesh edits that gets its own bake back stores nothing:
/// no lock, no undo step. Amending such a body back to its bake removes
/// the stored mesh again.
#[test]
fn the_unchanged_bake_is_not_stored() {
    let (mut s, b) = gridded_box();
    let m = s.editable_mesh(b).unwrap();
    let before = s.doc().clone();
    assert!(!s.set_meshes(vec![(b, Some((*m).clone()))]).unwrap());
    assert!(!s.has_mesh_edits(b), "not locked");
    assert_eq!(*s.doc(), before);
    assert!(s.set_meshes(vec![(b, Some(edited(&m)))]).unwrap());
    s.amend_meshes(vec![(b, Some((*m).clone()))]).unwrap();
    assert!(!s.has_mesh_edits(b), "amended back to the bake: unlocked");
    // A body that already has mesh edits keeps an equal mesh as an edit.
    assert!(s.set_meshes(vec![(b, Some(edited(&m)))]).unwrap());
    s.set_meshes(vec![(b, Some((*m).clone()))]).unwrap();
    assert!(s.has_mesh_edits(b));
}

/// Mesh edits only for bodies of the document: storing a mesh for any
/// other id is refused (the loader would refuse the saved file). Undo
/// never takes an imported body out of the document.
#[test]
fn meshes_for_bodies_not_in_the_document_are_refused() {
    let (mut s, b) = gridded_box();
    let m = s.editable_mesh(b).unwrap();
    while s.undo() {}
    assert!(s.doc().body(b).is_some(), "undo keeps the import");
    let r = s.set_meshes(vec![(BodyId(b.0 + 1000), Some(edited(&m)))]);
    assert!(r.unwrap_err().reason.contains("no object"));
    assert!(s.mesh_edited_bodies().is_empty());
}

#[test]
fn invalid_meshes_are_refused() {
    let (mut s, b) = gridded_box();
    let mut bad = (*s.editable_mesh(b).unwrap()).clone();
    bad.polygons[0].verts.reverse();
    assert!(s.set_meshes(vec![(b, Some(bad))]).is_err());
    let mut bad = (*s.editable_mesh(b).unwrap()).clone();
    bad.polygons[0].group = newtopo_kernel::FaceId(99);
    assert!(s.set_meshes(vec![(b, Some(bad))]).is_err());
    assert!(s.set_meshes(vec![(BodyId(77), None)]).is_err());
    assert!(!s.has_mesh_edits(b));
    // No mesh while a CAD preview is open.
    s.begin_preview();
    let m = (*s.editable_mesh(b).unwrap()).clone();
    assert!(s.set_meshes(vec![(b, Some(m))]).is_err());
}

#[test]
fn save_and_load_keep_mesh_edits() {
    let (mut s, b) = gridded_box();
    let m = s.editable_mesh(b).unwrap();
    let r = relax(
        &m,
        &Selection::Edges(edge_loop(&m, m.edges()[40].0, m.edges()[40].1).unwrap()),
        &RelaxParams::default(),
    );
    let e = match r {
        Ok(r) if r.mesh != *m => r.mesh,
        _ => edited(&m),
    };
    s.set_meshes(vec![(b, Some(e.clone()))]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("m.ntopo");
    s.save(&p).unwrap();
    let mut s2 = newtopo_core::Scene::load(Arc::new(MockKernel::new()), &p).unwrap();
    let b2 = s2.body_ids()[0];
    assert!(s2.has_mesh_edits(b2));
    let mut e2 = e.clone();
    e2.body = b2;
    assert_eq!(*s2.mesh_edit(b2).unwrap(), e2);
    assert_eq!(s2.stats(), s.stats());
    assert!(!s2.is_modified());
    // The archive lists the mesh in scene.json and stores it on its own.
    let f = std::fs::File::open(&p).unwrap();
    let mut z = zip::ZipArchive::new(f).unwrap();
    let mut json = String::new();
    z.by_name("scene.json")
        .unwrap()
        .read_to_string(&mut json)
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["format_version"], 2);
    assert_eq!(v["meshes"], serde_json::json!([b.0]));
    assert!(z.by_name(&format!("mesh/{}.json", b.0)).is_ok());
    // Still locked after the reload.
    assert!(s2.apply(grid(0, 6)).is_err());
}

/// Rewrite one archive with `f` applied to its entries.
fn rewrite(
    src: &std::path::Path,
    dst: &std::path::Path,
    f: impl Fn(&str, Vec<u8>) -> Option<Vec<u8>>,
) {
    let mut z = zip::ZipArchive::new(std::fs::File::open(src).unwrap()).unwrap();
    let mut w = zip::ZipWriter::new(std::fs::File::create(dst).unwrap());
    for i in 0..z.len() {
        let mut e = z.by_index(i).unwrap();
        let name = e.name().unwrap().to_string();
        let mut bytes = Vec::new();
        e.read_to_end(&mut bytes).unwrap();
        if let Some(b) = f(&name, bytes) {
            w.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(&b).unwrap();
        }
    }
    w.finish().unwrap();
}

#[test]
fn format_1_scenes_load_and_broken_mesh_entries_are_refused() {
    let (mut s, b) = gridded_box();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("plain.ntopo");
    s.save(&p).unwrap();
    // A format 1 file (as older builds wrote it): no meshes.
    let v1 = dir.path().join("v1.ntopo");
    rewrite(&p, &v1, |name, bytes| {
        if name != "scene.json" {
            return Some(bytes);
        }
        let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        v["format_version"] = serde_json::json!(1);
        Some(serde_json::to_vec(&v).unwrap())
    });
    // Without mesh edits the file is format 1 already, so v0.1.0 reads it.
    let format_of = |p: &std::path::Path| -> serde_json::Value {
        let mut z = zip::ZipArchive::new(std::fs::File::open(p).unwrap()).unwrap();
        let mut json = String::new();
        z.by_name("scene.json")
            .unwrap()
            .read_to_string(&mut json)
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("meshes").is_none());
        v["format_version"].clone()
    };
    assert_eq!(format_of(&p), 1);
    let s0 = newtopo_core::Scene::load(Arc::new(MockKernel::new()), &p).unwrap();
    assert_eq!(s0.doc().format_version, 2, "migrated in memory");
    assert!(!s0.is_modified());
    let mut s1 = newtopo_core::Scene::load(Arc::new(MockKernel::new()), &v1).unwrap();
    assert_eq!(s1.doc().format_version, 2, "migrated");
    assert!(s1.mesh_edited_bodies().is_empty());
    assert_eq!(s1.stats(), s.stats());

    // A format 2 scene whose mesh entry is missing or broken is refused.
    let m = s.editable_mesh(b).unwrap();
    s.set_meshes(vec![(b, Some(edited(&m)))]).unwrap();
    let p2 = dir.path().join("edited.ntopo");
    s.save(&p2).unwrap();
    let missing = dir.path().join("missing.ntopo");
    rewrite(&p2, &missing, |name, bytes| {
        (!name.starts_with("mesh/")).then_some(bytes)
    });
    let e = newtopo_core::Scene::load(Arc::new(MockKernel::new()), &missing)
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("mesh/"), "{e:#}");
    let broken = dir.path().join("broken.ntopo");
    rewrite(&p2, &broken, |name, bytes| {
        if !name.starts_with("mesh/") {
            return Some(bytes);
        }
        let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        v["polygons"][0]["verts"] = serde_json::json!([0, 0, 1]);
        Some(serde_json::to_vec(&v).unwrap())
    });
    let e = newtopo_core::Scene::load(Arc::new(MockKernel::new()), &broken)
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("mesh edits"), "{e:#}");
}

#[test]
fn export_writes_the_edited_mesh() {
    let (mut s, b) = gridded_box();
    let m = s.editable_mesh(b).unwrap();
    let e = edited(&m);
    s.set_meshes(vec![(b, Some(e.clone()))]).unwrap();
    let meshes = s.output().to_vec();
    let doc = s.doc().clone();
    let objects: Vec<newtopo_core::export::ExportObject> = meshes
        .iter()
        .map(|m| newtopo_core::export::ExportObject {
            name: "box",
            layer: "Default",
            color: None,
            transform: doc.body(m.body).unwrap().transform,
            mesh: m,
        })
        .collect();
    let opts = newtopo_core::export::ExportOptions {
        up: newtopo_core::export::ExportUp::Z,
        scale: 1.0,
        ..Default::default()
    };
    let files =
        newtopo_core::export::export(newtopo_core::export::Format::Obj, &objects, &opts).unwrap();
    let obj = String::from_utf8(files.main).unwrap();
    let vs = obj.lines().filter(|l| l.starts_with("v ")).count();
    assert!(vs >= e.positions.len(), "{vs}");
    // A moved vertex is written where the edit put it.
    let free = (0..e.positions.len()).find(|&i| !e.pinned[i]).unwrap();
    let q = e.positions[free];
    let found = obj.lines().filter(|l| l.starts_with("v ")).any(|l| {
        let c: Vec<f64> = l[2..]
            .split_whitespace()
            .map(|x| x.parse().unwrap())
            .collect();
        (c[0] - q.x).abs() < 1e-5 && (c[1] - q.y).abs() < 1e-5 && (c[2] - q.z).abs() < 1e-5
    });
    assert!(found, "edited position {q:?} not in the OBJ");
}

#[test]
fn project_to_cad_lands_on_the_face() {
    let (s, b) = gridded_box();
    // Face 0 of the mock box: whatever plane it is, a point projected onto
    // it projects onto itself again.
    let p = glam::DVec3::new(0.3, 0.2, 5.0);
    let q = s.project_to_cad(b, newtopo_kernel::FaceId(0), &[p])[0].unwrap();
    let q2 = s.project_to_cad(b, newtopo_kernel::FaceId(0), &[q])[0].unwrap();
    assert!(q.distance(q2) < 1e-9);
    assert_eq!(
        s.project_to_cad(b, newtopo_kernel::FaceId(999), &[p]),
        vec![None]
    );
}
