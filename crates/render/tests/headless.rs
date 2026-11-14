//! Headless render smoke test. Skips (passes) when no GPU adapter exists, so
//! CI without a GPU stays green; run under `gpu-lock` on dev machines.
use newtopo_render::synth::{SynthDisplay, build_scene, demo_bodies};
use newtopo_render::{Headless, NamedView, OrbitCamera, OverlayColors, RenderOptions};

#[test]
fn renders_demo_scene() {
    let mut h = match Headless::new() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let colors = OverlayColors::default();
    let scene = build_scene(
        &demo_bodies(),
        &colors,
        SynthDisplay {
            show_lines: true,
            ..Default::default()
        },
    );
    let (min, max) = scene.bounds().unwrap();
    let mut cam = OrbitCamera::default();
    cam.set_view(NamedView::Iso);
    cam.frame_aabb(min.as_dvec3(), max.as_dvec3(), 4.0 / 3.0);
    let opts = RenderOptions::default();
    let img = h
        .render(&[&scene], &cam, [320, 240], &opts)
        .expect("render");
    assert_eq!(img.dimensions(), (320, 240));
    // The model covers the centre; corners show the background.
    let centre = img.get_pixel(160, 120).0;
    let corner = img.get_pixel(2, 2).0;
    assert_ne!(centre, corner, "model should differ from background");
    let bg = colors.background_top;
    let expect = [
        (bg[0] * 255.0) as i32,
        (bg[1] * 255.0) as i32,
        (bg[2] * 255.0) as i32,
    ];
    for k in 0..3 {
        assert!(
            (corner[k] as i32 - expect[k]).abs() < 12,
            "corner {corner:?} vs bg {expect:?}"
        );
    }
    // Ortho renders too.
    cam.ortho = true;
    let img2 = h
        .render(&[&scene], &cam, [320, 240], &opts)
        .expect("render ortho");
    assert_ne!(img2.get_pixel(160, 120).0, img2.get_pixel(2, 2).0);
}
