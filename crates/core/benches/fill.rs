//! Auto-fill budget (SPEC §7 AC, §11): 2,000 typical polygons (6-16 sides).

use criterion::{Criterion, criterion_group, criterion_main};
use glam::{DVec2, DVec3};
use newtopo_core::fill::{FillInput, fill_polygon};

fn polygons() -> Vec<(Vec<DVec2>, Vec<DVec3>, Vec<u32>)> {
    // Deterministic mix of star-shaped and rectangle-with-side-points polygons.
    let mut seed = 2024u64;
    let mut rnd = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..2000)
        .map(|i| {
            let n = 6 + i % 11;
            let uv: Vec<DVec2> = if i % 2 == 0 {
                (0..n)
                    .map(|k| {
                        DVec2::from_angle(k as f64 * std::f64::consts::TAU / n as f64 + 0.3 * rnd())
                            * (0.4 + 0.6 * rnd())
                    })
                    .collect()
            } else {
                let (w, h) = (0.5 + 2.5 * rnd(), 0.5 + 2.5 * rnd());
                let c = [
                    DVec2::ZERO,
                    DVec2::new(w, 0.0),
                    DVec2::new(w, h),
                    DVec2::new(0.0, h),
                ];
                let per = [n / 4 + n % 4, n / 4, n / 4, n / 4];
                (0..4)
                    .flat_map(|s| {
                        (0..per[s])
                            .map(move |k| c[s].lerp(c[(s + 1) % 4], k as f64 / per[s] as f64))
                    })
                    .collect()
            };
            let pos = uv
                .iter()
                .map(|p| DVec3::new(p.x, p.y, 0.1 * p.x.sin()))
                .collect();
            let rank = (0..n as u32).collect();
            (uv, pos, rank)
        })
        .collect()
}

fn bench(c: &mut Criterion) {
    let polys = polygons();
    c.bench_function("fill_2000_typical", |b| {
        b.iter(|| {
            let mut quads = 0;
            for (uv, pos, rank) in &polys {
                quads += fill_polygon(&FillInput {
                    uv,
                    pos,
                    rank,
                    iso_dirs: [DVec2::X, DVec2::Y],
                })
                .quads();
            }
            quads
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
