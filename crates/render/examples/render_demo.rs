//! Render the synthetic demo model headless:
//! `cargo run -p newtopo-render --example render_demo -- out.png [--preview] [--wire] [--view iso] [--ortho] [--size 1600x1000]`
use glam::{DVec3, Vec3};
use newtopo_render::synth::{SynthDisplay, build_scene, demo_bodies, ground_grid};
use newtopo_render::{NamedView, OrbitCamera, OverlayColors, RenderOptions, ShadeMode};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.first().cloned().unwrap_or_else(|| "demo.png".into());
    let has = |f: &str| args.iter().any(|a| a == f);
    let val = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let size: Vec<u32> = val("--size")
        .unwrap_or_else(|| "1600x1000".into())
        .split('x')
        .filter_map(|s| s.parse().ok())
        .collect();
    let size = [
        size.first().copied().unwrap_or(1600),
        size.get(1).copied().unwrap_or(1000),
    ];
    let colors = OverlayColors::default();
    let bodies = demo_bodies();
    let disp = SynthDisplay {
        preview: has("--preview"),
        wireframe: has("--wire"),
        show_lines: !has("--no-lines"),
        show_vertices: has("--verts"),
    };
    let mut scene = build_scene(&bodies, &colors, disp);
    let (min, max) = scene.bounds().unwrap();
    let mut grid = newtopo_render::RenderScene::default();
    ground_grid(
        min,
        max,
        -0.01,
        newtopo_render::UpAxis::Z,
        colors.grid,
        &mut grid,
    );
    if has("--sym") {
        scene.add_plane(
            Vec3::new(0.0, 0.0, 20.0),
            Vec3::Y,
            70.0,
            colors.symmetry_plane,
        );
    }
    let mut cam = OrbitCamera {
        ortho: has("--ortho"),
        ..Default::default()
    };
    cam.set_view(
        NamedView::parse(&val("--view").unwrap_or_else(|| "iso".into())).unwrap_or(NamedView::Iso),
    );
    cam.yaw += 0.35;
    cam.pitch -= 0.15;
    cam.frame_aabb(
        min.as_dvec3(),
        max.as_dvec3() + DVec3::ZERO,
        size[0] as f64 / size[1] as f64,
    );
    let opts = RenderOptions {
        shade: if has("--flat") {
            ShadeMode::Flat
        } else {
            ShadeMode::Smooth
        },
        pixels_per_point: size[1] as f32 / 800.0,
        ..Default::default()
    };
    let mut h = newtopo_render::Headless::new()?;
    eprintln!("adapter: {}", h.adapter_info);
    let img = h.render(&[&grid, &scene], &cam, size, &opts)?;
    img.save(&out)?;
    eprintln!("wrote {out}");
    Ok(())
}
