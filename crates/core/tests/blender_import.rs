//! SPEC §8.3 AC: exported files import into Blender headless with matching
//! counts, names, layer hierarchy, materials and hard edges.
//!
//! Opt-in: `NEWTOPO_BLENDER=1 cargo test -p newtopo-core --test blender_import -- --ignored`.
//! `NEWTOPO_BLENDER_CMD` overrides the Blender executable (default `blender`).

mod common;

use common::{blender_json, num};
use glam::DVec3;
use newtopo_core::export::samples::SampleScene;
use newtopo_core::export::{ExportOptions, ExportUp, Format, export};
use serde_json::Value;

struct Case {
    label: &'static str,
    importer: &'static str,
    format: Format,
    opts: ExportOptions,
}

#[test]
#[ignore = "needs Blender; set NEWTOPO_BLENDER=1 and run with --ignored"]
fn exports_import_into_blender() {
    if std::env::var("NEWTOPO_BLENDER").as_deref() != Ok("1") {
        eprintln!("skipped: NEWTOPO_BLENDER != 1");
        return;
    }
    let scene = SampleScene::new();
    let objects = scene.objects();
    let dir = tempfile::tempdir().unwrap();
    let d = ExportOptions::default();
    let zup_tri = ExportOptions {
        up: ExportUp::Z,
        triangulate: true,
        ..d.clone()
    };
    let cases = vec![
        Case {
            label: "obj",
            importer: "obj",
            format: Format::Obj,
            opts: d.clone(),
        },
        Case {
            label: "obj-zup-tri-flat",
            importer: "obj-zup",
            format: Format::Obj,
            opts: ExportOptions {
                layers_as_hierarchy: false,
                ..zup_tri.clone()
            },
        },
        Case {
            label: "glb",
            importer: "gltf",
            format: Format::Glb,
            opts: d.clone(),
        },
        Case {
            label: "fbx",
            importer: "fbx",
            format: Format::Fbx,
            opts: d.clone(),
        },
        Case {
            label: "fbx-zup-tri",
            importer: "fbx",
            format: Format::Fbx,
            opts: zup_tri.clone(),
        },
        Case {
            label: "fbx-legacy",
            importer: "fbx-legacy",
            format: Format::Fbx,
            opts: d.clone(),
        },
        Case {
            label: "fbx-legacy-zup-tri",
            importer: "fbx-legacy",
            format: Format::Fbx,
            opts: zup_tri.clone(),
        },
    ];
    let mut specs = Vec::new();
    for c in &cases {
        let path = dir
            .path()
            .join(format!("{}.{}", c.label, c.format.extension()));
        export(c.format, &objects, &c.opts)
            .unwrap()
            .save(&path)
            .unwrap();
        specs.push(format!("{}={}:{}", c.label, c.importer, path.display()));
    }
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/blender/check_import.py"
    );
    let out = blender_json(&["-P", script, "--"], &specs);
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&strip_positions(&out)).unwrap()
    );

    for c in &cases {
        let res = &out[c.label];
        assert!(res.get("error").is_none(), "{}: {}", c.label, res["error"]);
        let objs = &res["objects"];
        let tri = c.format == Format::Glb || c.opts.triangulate;
        let layers = c.opts.layers_as_hierarchy && c.format != Format::Obj;
        for o in &objects {
            let got = &objs[o.name];
            assert_eq!(
                got["type"], "MESH",
                "{}: {} missing in {objs}",
                c.label, o.name
            );
            let mesh = o.mesh;
            // Polygon counts (FBX/OBJ) or triangle counts (glTF).
            let polys = mesh.polygons.len() as u64;
            let tris: u64 = mesh.polygons.iter().map(|p| p.verts.len() as u64 - 2).sum();
            assert_eq!(
                num(&got["triangles"]),
                tris,
                "{} {} triangles",
                c.label,
                o.name
            );
            if !tri {
                assert_eq!(
                    num(&got["polygons"]),
                    polys,
                    "{} {} polygons",
                    c.label,
                    o.name
                );
            }
            // Unique world positions match the scene (after merge by distance).
            let want: Vec<DVec3> = mesh
                .vertices
                .iter()
                .map(|v| o.transform.transform_point3(v.position))
                .collect();
            assert_eq!(
                num(&got["unique_positions"]),
                want.len() as u64,
                "{} {} unique positions",
                c.label,
                o.name
            );
            let have: Vec<DVec3> = got["world_positions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    DVec3::new(
                        p[0].as_f64().unwrap(),
                        p[1].as_f64().unwrap(),
                        p[2].as_f64().unwrap(),
                    )
                })
                .collect();
            for w in &want {
                assert!(
                    have.iter().any(|h| (*h - *w).length() < 2e-3),
                    "{} {}: world position {w} not found in {have:?}",
                    c.label,
                    o.name
                );
            }
            if c.format != Format::Glb {
                assert_eq!(
                    num(&got["vertices"]),
                    want.len() as u64,
                    "{} {} vertices",
                    c.label,
                    o.name
                );
            }
            // Layer hierarchy.
            let want_parent = if layers {
                Value::from(o.layer)
            } else {
                Value::Null
            };
            assert_eq!(got["parent"], want_parent, "{} {} parent", c.label, o.name);
            // Materials.
            let mats: Vec<&str> = got["materials"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["name"].as_str().unwrap())
                .collect();
            let want_mats: Vec<String> = o
                .color
                .map(newtopo_core::export::prep::material_name)
                .into_iter()
                .collect();
            assert_eq!(mats, want_mats, "{} {} materials", c.label, o.name);
            // Visible hard edges: cube all 12 split, smooth tube and the
            // planar hexagon none.
            let want_split = if o.name == "Cube" { 12 } else { 0 };
            assert_eq!(
                num(&got["split_edges"]),
                want_split,
                "{} {} split-normal edges",
                c.label,
                o.name
            );
            // FBX carries edge smoothing: every hard edge is marked sharp.
            if c.format == Format::Fbx {
                let hard = mesh.edges.iter().filter(|e| e.hard).count() as u64;
                assert_eq!(
                    num(&got["sharp_edges"]),
                    hard,
                    "{} {} sharp edges",
                    c.label,
                    o.name
                );
            }
        }
        if layers {
            for l in ["Parts", "Misc"] {
                assert_eq!(objs[l]["type"], "EMPTY", "{}: layer {l}", c.label);
            }
        }
    }
}

fn strip_positions(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| *k != "world_positions")
                .map(|(k, v)| (k.clone(), strip_positions(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Physical size: a 100 mm cube in a millimetre scene, exported with the
/// app/CLI defaults (export unit = scene unit = mm), imports into Blender as
/// 0.1 m from FBX (both importers) and GLB. OBJ has no unit metadata, so it
/// arrives as 100 Blender units (the file's millimetre numbers).
#[test]
#[ignore = "needs Blender; set NEWTOPO_BLENDER=1 and run with --ignored"]
fn hundred_mm_cube_imports_at_real_size() {
    if std::env::var("NEWTOPO_BLENDER").as_deref() != Ok("1") {
        eprintln!("skipped: NEWTOPO_BLENDER != 1");
        return;
    }
    use newtopo_core::export::{ExportObject, samples, unit_metres};
    let mesh = samples::cuboid(DVec3::splat(100.0));
    let objects = [ExportObject {
        name: "Cube100",
        layer: "Parts",
        color: None,
        transform: glam::DMat4::IDENTITY,
        mesh: &mesh,
    }];
    // What the app and CLI pass for a mm scene exported in mm.
    let opts = ExportOptions {
        scale: 1.0,
        unit_metres: unit_metres("mm").unwrap(),
        ..ExportOptions::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let cases = [
        ("fbx-mm", "fbx", Format::Fbx, 0.1),
        ("fbx-legacy-mm", "fbx-legacy", Format::Fbx, 0.1),
        ("glb-mm", "gltf", Format::Glb, 0.1),
        ("obj-mm", "obj", Format::Obj, 100.0),
    ];
    let mut specs = Vec::new();
    for (label, importer, format, _) in &cases {
        let path = dir.path().join(format!("{label}.{}", format.extension()));
        export(*format, &objects, &opts)
            .unwrap()
            .save(&path)
            .unwrap();
        specs.push(format!("{label}={importer}:{}", path.display()));
    }
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/blender/check_import.py"
    );
    let out = blender_json(&["-P", script, "--"], &specs);
    for (label, _, _, want) in &cases {
        let res = &out[*label];
        assert!(res.get("error").is_none(), "{label}: {}", res["error"]);
        let pts: Vec<DVec3> = res["objects"]["Cube100"]["world_positions"]
            .as_array()
            .unwrap_or_else(|| panic!("{label}: no Cube100 in {res}"))
            .iter()
            .map(|p| {
                DVec3::new(
                    p[0].as_f64().unwrap(),
                    p[1].as_f64().unwrap(),
                    p[2].as_f64().unwrap(),
                )
            })
            .collect();
        let lo = pts
            .iter()
            .fold(DVec3::splat(f64::INFINITY), |a, p| a.min(*p));
        let hi = pts
            .iter()
            .fold(DVec3::splat(f64::NEG_INFINITY), |a, p| a.max(*p));
        let size = hi - lo;
        eprintln!(
            "{label}: Blender size {:.4} x {:.4} x {:.4}",
            size.x, size.y, size.z
        );
        assert!(
            (size - DVec3::splat(*want)).abs().max_element() < 1e-3 * want,
            "{label}: Blender size {size}, want {want}"
        );
    }
}
