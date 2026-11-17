//! Timing against the SPEC §4.6 budgets (run with --release --ignored).
mod common;
use common::*;
use newtopo_core::Op;
use newtopo_core::doc::Spacing;
use newtopo_core::ops::PatchSel;
use newtopo_kernel::mock::MockBody;
use std::time::Instant;

#[test]
#[ignore = "timing; run with --release -- --ignored --nocapture"]
fn perf_budgets() {
    for (n, m) in [(2u32, 2u32), (5, 5), (10, 10)] {
        let t = Instant::now();
        let (mut s, b) = scene_with(MockBody::plate_with_holes(n, m));
        let build = t.elapsed();
        let faces = s.ctx(b).unwrap().topo.faces.len();
        let t = Instant::now();
        let st = s.output_of(b).unwrap().stats();
        let full = t.elapsed();
        // One-patch change: a grid on one hole wall.
        let t = Instant::now();
        s.apply(Op::Grid {
            patches: PatchSel {
                faces: vec![6],
                ..Default::default()
            },
            u: 4,
            v: 2,
            start_on_seam: false,
            spacing: Spacing::ArcLength,
        })
        .unwrap();
        let one = t.elapsed();
        let t = Instant::now();
        s.apply(Op::Grid {
            patches: PatchSel {
                faces: vec![6],
                ..Default::default()
            },
            u: 5,
            v: 2,
            start_on_seam: false,
            spacing: Spacing::ArcLength,
        })
        .unwrap();
        let one2 = t.elapsed();
        // Count change only (wheel step), output only (no validation gate).
        let t = Instant::now();
        s.apply_unchecked(Op::Grid {
            patches: PatchSel {
                faces: vec![6],
                ..Default::default()
            },
            u: 6,
            v: 2,
            start_on_seam: false,
            spacing: Spacing::ArcLength,
        })
        .unwrap();
        let _ = s.output_of(b);
        let eval_only = t.elapsed();
        println!(
            "plate {n}x{m}: {faces} faces, ctx build {build:?}, full eval {full:?} ({} polys), one-patch apply {one:?} / {one2:?}, one-patch eval only {eval_only:?}, cache {:?}",
            st.faces,
            s.cache_stats(b)
        );
    }
}

#[test]
#[ignore = "timing; run with --release -- --ignored --nocapture"]
fn perf_occt_models() {
    use newtopo_kernel::occt::{CadFormat, HealSettings, OcctKernel};
    use std::sync::Arc;
    for name in ["filleted_box", "plate_holes", "flange"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{name}.step"));
        newtopo_testmodels::write_model(newtopo_testmodels::model(name).unwrap(), &path).unwrap();
        let k = Arc::new(OcctKernel::new());
        let file = k.import_file(&path, CadFormat::Step, 0.001).unwrap();
        let mut s = newtopo_core::Scene::new(k.clone());
        let bb = file.bodies.into_iter().next().unwrap();
        let heal = k.heal(bb.body, HealSettings::default()).unwrap_or_default();
        let b = s.add_kernel_body(bb.body, bb.attrs, heal, None).unwrap();
        let t = Instant::now();
        let _ = s.output_of(b);
        let full = t.elapsed();
        let curved = s
            .ctx(b)
            .unwrap()
            .topo
            .face_ids()
            .find(|f| {
                s.ctx(b).unwrap().topo.face(*f).surface.kind != newtopo_kernel::SurfaceKind::Plane
            })
            .unwrap();
        let mut times = Vec::new();
        for a in [4u32, 6, 8] {
            let t = Instant::now();
            let _ = s.apply(Op::Connect {
                patches: PatchSel {
                    body: Some(b),
                    faces: vec![curved.0],
                    ..Default::default()
                },
                a,
                b: 0,
                start_on_seam: false,
            });
            times.push(t.elapsed());
        }
        println!(
            "occt {name}: {} faces, full eval {full:?}, Connect count changes {times:?}, cache {:?}",
            s.ctx(b).unwrap().topo.faces.len(),
            s.cache_stats(b)
        );
    }
}
