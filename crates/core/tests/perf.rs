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
        s.apply(Op::Grid { patches: PatchSel { faces: vec![6], ..Default::default() }, u: 4, v: 2, start_on_seam: false, spacing: Spacing::ArcLength }).unwrap();
        let one = t.elapsed();
        let t = Instant::now();
        s.apply(Op::Grid { patches: PatchSel { faces: vec![6], ..Default::default() }, u: 5, v: 2, start_on_seam: false, spacing: Spacing::ArcLength }).unwrap();
        let one2 = t.elapsed();
        println!(
            "plate {n}x{m}: {faces} faces, ctx build {build:?}, full eval {full:?} ({} polys), one-patch apply {one:?} / {one2:?}",
            st.faces
        );
    }
}
