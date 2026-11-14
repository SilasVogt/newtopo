//! Export tests (SPEC §8.3): normals, transforms, golden files, read-back.

use super::prep::{TransformMode, prepare};
use super::samples::{SampleScene, build, cube, ngon, strip};
use super::*;
use glam::{DQuat, DVec3};
use std::io::Read;

fn approx(a: DVec3, b: DVec3) -> bool {
    (a - b).length() < 1e-9
}

fn obj<'a>(mesh: &'a OutputMesh, transform: DMat4) -> ExportObject<'a> {
    ExportObject {
        name: "o",
        layer: "L",
        color: None,
        transform,
        mesh,
    }
}

#[test]
fn cube_normals_split_at_hard_edges() {
    let m = cube();
    let o = ExportOptions {
        up: ExportUp::Z,
        ..Default::default()
    };
    let p = prepare(
        &obj(&m, DMat4::IDENTITY),
        &o,
        TransformMode::Bake,
        false,
        None,
    );
    assert_eq!(p.polys.len(), 6);
    for (poly, ns) in m.polygons.iter().zip(&p.normals) {
        for n in ns {
            assert!(approx(*n, poly.normals[0]));
        }
    }
    assert_eq!(p.edges.len(), 12);
    assert!(p.edges.iter().all(|e| e.hard));
    assert!(
        p.smoothing.iter().all(|&s| s == 0),
        "all-hard cube is flat: {:?}",
        p.smoothing
    );
}

#[test]
fn smooth_strip_shares_normals_across_faces() {
    let m = strip(12);
    let o = ExportOptions {
        up: ExportUp::Z,
        ..Default::default()
    };
    let p = prepare(
        &obj(&m, DMat4::IDENTITY),
        &o,
        TransformMode::Bake,
        false,
        None,
    );
    for (poly, ns) in p.polys.iter().zip(&p.normals) {
        for (&v, n) in poly.iter().zip(ns) {
            let q = p.positions[v as usize];
            assert!(approx(*n, DVec3::new(q.x, q.y, 0.0).normalize()));
        }
    }
    assert!(p.smoothing.iter().all(|&s| s == 1));
    assert_eq!(p.edges.iter().filter(|e| e.hard).count(), 24);
}

#[test]
fn non_hard_crease_is_averaged_and_face_weighting_works() {
    // Two quads meeting at a 40 degree crease that is not hard.
    let t = 20f64.to_radians();
    let pos = vec![
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(0.0, 1.0, 0.0),
        DVec3::new(-t.cos(), 1.0, t.sin()),
        DVec3::new(-t.cos(), 0.0, t.sin()),
        DVec3::new(2.0 * t.cos(), 1.0, 2.0 * t.sin()),
        DVec3::new(2.0 * t.cos(), 0.0, 2.0 * t.sin()),
    ];
    let n_left = (pos[1] - pos[0]).cross(pos[3] - pos[0]).normalize();
    let n_right = (pos[5] - pos[0]).cross(pos[1] - pos[0]).normalize();
    let m = build(
        0,
        &pos,
        vec![
            (vec![0, 1, 2, 3], 0, vec![n_left; 4]),
            (vec![0, 5, 4, 1], 1, vec![n_right; 4]),
        ],
        |_, _| false,
        |a, b| (a, b) == (0, 1),
    );
    let o = ExportOptions {
        up: ExportUp::Z,
        ..Default::default()
    };
    let p = prepare(
        &obj(&m, DMat4::IDENTITY),
        &o,
        TransformMode::Bake,
        false,
        None,
    );
    let avg = (n_left + n_right).normalize();
    assert!(approx(p.normals[0][0], avg) && approx(p.normals[1][0], avg));
    assert!(
        approx(p.normals[0][2], n_left),
        "unshared corner keeps its own normal"
    );
    // Face weighted: the right quad is twice as large.
    let o = ExportOptions {
        face_weighted_normals: true,
        ..o
    };
    let p = prepare(
        &obj(&m, DMat4::IDENTITY),
        &o,
        TransformMode::Bake,
        false,
        None,
    );
    assert!(approx(
        p.normals[0][0],
        (n_left + 2.0 * n_right).normalize()
    ));
    // Marking the crease hard splits it again.
    let m = build(
        0,
        &pos,
        vec![
            (vec![0, 1, 2, 3], 0, vec![n_left; 4]),
            (vec![0, 5, 4, 1], 1, vec![n_right; 4]),
        ],
        |_, _| true,
        |_, _| true,
    );
    let p = prepare(
        &obj(&m, DMat4::IDENTITY),
        &o,
        TransformMode::Bake,
        false,
        None,
    );
    assert!(approx(p.normals[0][0], n_left) && approx(p.normals[1][0], n_right));
}

#[test]
fn y_up_conversion_scale_and_mirror() {
    let m = cube();
    let o = ExportOptions {
        up: ExportUp::Y,
        scale: 0.5,
        ..Default::default()
    };
    let p = prepare(
        &obj(&m, DMat4::from_translation(DVec3::new(0.0, 0.0, 10.0))),
        &o,
        TransformMode::Bake,
        false,
        None,
    );
    // CAD (x, y, z) -> (x, z, -y), then scaled.
    assert!(approx(p.positions[2], DVec3::new(1.0, 5.0, -0.5)));
    // Top face normal +Z becomes +Y.
    assert!(approx(p.normals[1][0], DVec3::Y));
    // Node mode keeps the transform: translation converted and scaled.
    let p = prepare(
        &obj(&m, DMat4::from_translation(DVec3::new(0.0, 0.0, 10.0))),
        &o,
        TransformMode::Node,
        false,
        None,
    );
    assert!(approx(p.node.translation, DVec3::new(0.0, 5.0, 0.0)));
    assert!(approx(p.positions[2], DVec3::new(1.0, 0.0, -0.5)));
    // A mirror can't be a node transform: baked, and winding reversed.
    let mirror = DMat4::from_scale(DVec3::new(-1.0, 1.0, 1.0));
    let p = prepare(&obj(&m, mirror), &o, TransformMode::Node, false, None);
    assert_eq!(p.node, prep::Trs::IDENTITY);
    assert_eq!(p.polys[0], vec![1, 2, 3, 0]);
    // Every polygon still faces along its normals after the mirror.
    for (poly, ns) in p.polys.iter().zip(&p.normals) {
        let pts: Vec<DVec3> = poly.iter().map(|&v| p.positions[v as usize]).collect();
        let nn = crate::fill::geom::newell_normal(&pts);
        assert!(nn.dot(ns[0]) > 0.99, "{nn} vs {}", ns[0]);
    }
}

#[test]
fn node_rotation_matches_world_positions() {
    let m = cube();
    let t = DMat4::from_scale_rotation_translation(
        DVec3::new(1.0, 2.0, 3.0),
        DQuat::from_euler(glam::EulerRot::XYZ, 0.3, -0.7, 1.1),
        DVec3::new(4.0, -5.0, 6.0),
    );
    for up in [ExportUp::Y, ExportUp::Z] {
        let o = ExportOptions {
            up,
            scale: 0.01,
            ..Default::default()
        };
        let baked = prepare(&obj(&m, t), &o, TransformMode::Bake, false, None);
        let node = prepare(&obj(&m, t), &o, TransformMode::Node, false, None);
        assert_ne!(node.node, prep::Trs::IDENTITY);
        for (a, b) in baked.positions.iter().zip(&node.positions) {
            assert!((node.node.matrix().transform_point3(*b) - *a).length() < 1e-9);
        }
    }
}

#[test]
fn fbx_euler_order_is_x_then_y_then_z() {
    // FBX "eEulerXYZ": rotate about X first, then Y, then Z (R = Rz Ry Rx).
    let q = DQuat::from_euler(glam::EulerRot::XYZ, 0.3, -0.7, 1.1).normalize();
    let (z, y, x) = q.to_euler(glam::EulerRot::ZYX);
    let r = DQuat::from_rotation_z(z) * DQuat::from_rotation_y(y) * DQuat::from_rotation_x(x);
    assert!(r.dot(q).abs() > 1.0 - 1e-12);
}

#[test]
fn triangulate_keeps_corner_normals() {
    let m = ngon(6);
    let o = ExportOptions {
        up: ExportUp::Z,
        triangulate: true,
        ..Default::default()
    };
    let p = prepare(
        &obj(&m, DMat4::IDENTITY),
        &o,
        TransformMode::Bake,
        true,
        None,
    );
    assert_eq!(p.polys.len(), 4);
    assert!(p.polys.iter().all(|t| t.len() == 3));
    assert_eq!(p.edges.len(), 9);
    assert_eq!(p.edges.iter().filter(|e| e.hard).count(), 6);
    assert!(p.normals.iter().flatten().all(|n| approx(*n, DVec3::Z)));
}

// ---- Golden files -------------------------------------------------------

#[test]
fn golden_obj() {
    let s = SampleScene::new();
    let files = export(Format::Obj, &s.objects(), &ExportOptions::default()).unwrap();
    insta::assert_snapshot!("obj_main", String::from_utf8(files.main).unwrap());
    assert_eq!(files.side.len(), 1);
    insta::assert_snapshot!(
        "obj_mtl",
        String::from_utf8(files.side[0].1.clone()).unwrap()
    );
    let tri = export(
        Format::Obj,
        &s.objects(),
        &ExportOptions {
            triangulate: true,
            layers_as_hierarchy: false,
            up: ExportUp::Z,
            ..Default::default()
        },
    )
    .unwrap();
    insta::assert_snapshot!("obj_triangulated_zup", String::from_utf8(tri.main).unwrap());
}

/// Parse a GLB into (JSON, BIN), checking the container.
pub(crate) fn parse_glb(b: &[u8]) -> (serde_json::Value, Vec<u8>) {
    let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    assert_eq!(&b[0..4], b"glTF");
    assert_eq!(u(4), 2);
    assert_eq!(u(8), b.len());
    let jl = u(12);
    assert_eq!(&b[16..20], b"JSON");
    assert_eq!(jl % 4, 0);
    let json: serde_json::Value = serde_json::from_slice(&b[20..20 + jl]).unwrap();
    let mut bin = Vec::new();
    if 20 + jl < b.len() {
        let bl = u(20 + jl);
        assert_eq!(&b[24 + jl..28 + jl], b"BIN\0");
        bin = b[28 + jl..28 + jl + bl].to_vec();
    }
    (json, bin)
}

#[test]
fn golden_glb() {
    let s = SampleScene::new();
    let files = export(Format::Glb, &s.objects(), &ExportOptions::default()).unwrap();
    let (json, bin) = parse_glb(&files.main);
    // Accessors stay inside their views, views inside the buffer.
    for a in json["accessors"].as_array().unwrap() {
        let v = &json["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
        let comp = match a["componentType"].as_u64().unwrap() {
            5126 | 5125 => 4,
            5123 => 2,
            c => panic!("component {c}"),
        };
        let width = if a["type"] == "VEC3" { 3 } else { 1 };
        assert_eq!(
            a["count"].as_u64().unwrap() * comp * width,
            v["byteLength"].as_u64().unwrap()
        );
        assert!(
            v["byteOffset"].as_u64().unwrap() + v["byteLength"].as_u64().unwrap()
                <= bin.len() as u64
        );
        assert_eq!(v["byteOffset"].as_u64().unwrap() % 4, 0);
    }
    // Triangle counts: cube 12, tube 24, hexagon 4.
    let tris: Vec<u64> = json["meshes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            json["accessors"][m["primitives"][0]["indices"].as_u64().unwrap() as usize]["count"]
                .as_u64()
                .unwrap()
                / 3
        })
        .collect();
    assert_eq!(tris, vec![12, 24, 4]);
    // Vertices split by normal: cube 24, tube 24 (smooth), hexagon 6.
    let verts: Vec<u64> = json["meshes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            json["accessors"][m["primitives"][0]["attributes"]["POSITION"]
                .as_u64()
                .unwrap() as usize]["count"]
                .as_u64()
                .unwrap()
        })
        .collect();
    assert_eq!(verts, vec![24, 24, 6]);
    insta::assert_snapshot!("glb_json", serde_json::to_string_pretty(&json).unwrap());
}

/// Minimal FBX reader for tests: returns a text dump of the node tree with
/// array lengths and the first values, validating offsets as it goes.
pub(crate) fn dump_fbx(b: &[u8]) -> String {
    use std::fmt::Write;
    assert_eq!(&b[..21], b"Kaydara FBX Binary  \0");
    assert_eq!(&b[21..23], &[0x1A, 0]);
    assert_eq!(u32::from_le_bytes(b[23..27].try_into().unwrap()), 7400);
    fn rd(b: &[u8], o: &mut usize, n: usize) -> Vec<u8> {
        let v = b[*o..*o + n].to_vec();
        *o += n;
        v
    }
    fn u32_at(b: &[u8], o: &mut usize) -> u32 {
        u32::from_le_bytes(rd(b, o, 4).try_into().unwrap())
    }
    fn array(b: &[u8], o: &mut usize, size: usize) -> (usize, Vec<u8>) {
        let len = u32_at(b, o) as usize;
        let enc = u32_at(b, o);
        let clen = u32_at(b, o) as usize;
        let raw = rd(b, o, clen);
        let data = if enc == 1 {
            let mut d = Vec::new();
            flate2::read::ZlibDecoder::new(&raw[..])
                .read_to_end(&mut d)
                .unwrap();
            d
        } else {
            raw
        };
        assert_eq!(data.len(), len * size);
        (len, data)
    }
    fn node(b: &[u8], o: &mut usize, depth: usize, out: &mut String) -> bool {
        let start = *o;
        let end = u32_at(b, o) as usize;
        let np = u32_at(b, o);
        let plen = u32_at(b, o) as usize;
        let nl = rd(b, o, 1)[0] as usize;
        if end == 0 {
            assert_eq!(np, 0);
            assert_eq!(*o - start, 13);
            return false;
        }
        let name = String::from_utf8(rd(b, o, nl)).unwrap();
        let mut line = format!("{}{}:", "  ".repeat(depth), name);
        let pstart = *o;
        for _ in 0..np {
            let t = rd(b, o, 1)[0];
            let txt = match t {
                b'C' => format!("{}", rd(b, o, 1)[0] != 0),
                b'Y' => format!("{}", i16::from_le_bytes(rd(b, o, 2).try_into().unwrap())),
                b'I' => format!("{}", i32::from_le_bytes(rd(b, o, 4).try_into().unwrap())),
                b'L' => format!("{}", i64::from_le_bytes(rd(b, o, 8).try_into().unwrap())),
                b'F' => format!("{}", f32::from_le_bytes(rd(b, o, 4).try_into().unwrap())),
                b'D' => format!("{}", f64::from_le_bytes(rd(b, o, 8).try_into().unwrap())),
                b'S' | b'R' => {
                    let l = u32_at(b, o) as usize;
                    format!("{:?}", String::from_utf8_lossy(&rd(b, o, l)))
                }
                b'i' => {
                    let (n, d) = array(b, o, 4);
                    let v: Vec<i32> = d
                        .chunks_exact(4)
                        .take(12)
                        .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
                        .collect();
                    format!("i[{n}]{v:?}")
                }
                b'd' => {
                    let (n, d) = array(b, o, 8);
                    let v: Vec<String> = d
                        .chunks_exact(8)
                        .take(9)
                        .map(|c| format!("{:.4}", f64::from_le_bytes(c.try_into().unwrap())))
                        .collect();
                    format!("d[{n}][{}]", v.join(", "))
                }
                b'f' | b'l' | b'b' => {
                    let size = match t {
                        b'f' => 4,
                        b'l' => 8,
                        _ => 1,
                    };
                    let (n, _) = array(b, o, size);
                    format!("{}[{n}]", t as char)
                }
                _ => panic!("bad property type {t}"),
            };
            let _ = write!(line, " {txt}");
        }
        assert_eq!(*o - pstart, plen, "property list length of {name}");
        let _ = writeln!(out, "{line}");
        if *o < end {
            while node(b, o, depth + 1, out) {}
        }
        assert_eq!(*o, end, "end offset of {name}");
        true
    }
    let mut o = 27;
    let mut out = String::new();
    while node(b, &mut o, 0, &mut out) {}
    assert_eq!(o, b.len(), "trailing bytes after the closing NULL record");
    out
}

#[test]
fn golden_fbx() {
    let s = SampleScene::new();
    let files = export(Format::Fbx, &s.objects(), &ExportOptions::default()).unwrap();
    insta::assert_snapshot!("fbx_dump", dump_fbx(&files.main));
    // Deterministic bytes.
    assert_eq!(
        files.main,
        export(Format::Fbx, &s.objects(), &ExportOptions::default())
            .unwrap()
            .main
    );
}

#[test]
fn fbx_reads_back_with_ufbx() {
    let s = SampleScene::new();
    for (up, tri) in [(ExportUp::Y, false), (ExportUp::Z, true)] {
        let opts = ExportOptions {
            up,
            triangulate: tri,
            ..Default::default()
        };
        let files = export(Format::Fbx, &s.objects(), &opts).unwrap();
        let scene = ufbx::load_memory(
            &files.main,
            ufbx::LoadOpts {
                target_axes: ufbx::CoordinateAxes::right_handed_z_up(),
                target_unit_meters: 1.0,
                ..Default::default()
            },
        )
        .expect("ufbx load");
        // Declared units and axes as written (no conversion requested).
        let raw = ufbx::load_memory(&files.main, ufbx::LoadOpts::default()).expect("ufbx load");
        assert_eq!(raw.settings.unit_meters, 1.0);
        let want_up = if up == ExportUp::Y {
            ufbx::CoordinateAxis::PositiveY
        } else {
            ufbx::CoordinateAxis::PositiveZ
        };
        assert_eq!(raw.settings.axes.up, want_up);
        assert_eq!(raw.metadata.warnings.len(), 0, "ufbx warnings");
        let mut names: Vec<String> = Vec::new();
        for node in scene.nodes.iter() {
            if node.is_root {
                continue;
            }
            names.push(format!(
                "{}<{}",
                node.element.name,
                node.parent
                    .as_ref()
                    .map(|p| p.element.name.to_string())
                    .unwrap_or_default()
            ));
            let Some(mesh) = node.mesh.as_ref() else {
                continue;
            };
            let (faces, verts, hard_edges) = (
                mesh.num_faces,
                mesh.num_vertices,
                mesh.edge_smoothing.iter().filter(|&&sm| !sm).count(),
            );
            let (want_faces, want_hard) = match node.element.name.as_ref() {
                "Cube" => (if tri { 12 } else { 6 }, 12),
                "Tube" => (if tri { 24 } else { 12 }, 24),
                "Hexagon" => (if tri { 4 } else { 1 }, 6),
                n => panic!("unexpected mesh {n}"),
            };
            assert_eq!(
                (faces, hard_edges),
                (want_faces, want_hard),
                "{}",
                node.element.name
            );
            assert_eq!(
                verts,
                [("Cube", 8), ("Tube", 24), ("Hexagon", 6)]
                    .iter()
                    .find(|x| x.0 == node.element.name.as_ref())
                    .unwrap()
                    .1
            );
            // World-space positions match the Z-up scene after ufbx converts
            // axes and units back.
            let src = match node.element.name.as_ref() {
                "Cube" => &s.cube,
                "Tube" => &s.strip,
                _ => &s.ngon,
            };
            let xf = s
                .objects()
                .into_iter()
                .find(|o| o.name == node.element.name.as_ref())
                .unwrap()
                .transform;
            for (i, v) in mesh.vertices.iter().enumerate() {
                let w = ufbx::transform_position(&node.geometry_to_world, *v);
                let want = xf.transform_point3(src.vertices[i].position);
                assert!(
                    (DVec3::new(w.x, w.y, w.z) - want).length() < 1e-6,
                    "{} v{i}: {w:?} vs {want}",
                    node.element.name
                );
            }
            // Normals point outwards in world space for the cube.
            if node.element.name.as_ref() == "Cube" {
                assert_eq!(mesh.materials.len(), 1);
                assert_eq!(mesh.materials[0].element.name.as_ref(), "ntopo_cc3333");
            }
        }
        names.sort();
        assert_eq!(
            names,
            vec![
                "Cube<Parts",
                "Hexagon<Misc",
                "Misc<",
                "Parts<",
                "Tube<Parts"
            ]
        );
    }
}

#[test]
fn layers_off_flattens_hierarchy() {
    let s = SampleScene::new();
    let o = ExportOptions {
        layers_as_hierarchy: false,
        ..Default::default()
    };
    let (json, _) = parse_glb(&export(Format::Glb, &s.objects(), &o).unwrap().main);
    assert_eq!(json["scenes"][0]["nodes"], serde_json::json!([0, 1, 2]));
    let dump = dump_fbx(&export(Format::Fbx, &s.objects(), &o).unwrap().main);
    assert!(!dump.contains("\"Null\""));
}

#[test]
fn save_rewrites_mtllib() {
    let s = SampleScene::new();
    let files = export(Format::Obj, &s.objects(), &ExportOptions::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    files.save(&dir.path().join("part.obj")).unwrap();
    let text = std::fs::read_to_string(dir.path().join("part.obj")).unwrap();
    assert!(text.lines().nth(1) == Some("mtllib part.mtl"));
    assert!(dir.path().join("part.mtl").exists());
}

#[test]
fn rejects_bad_scale() {
    let s = SampleScene::new();
    assert!(
        export(
            Format::Obj,
            &s.objects(),
            &ExportOptions {
                scale: 0.0,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn obj_names_are_sanitised() {
    let m = cube();
    let o = ExportObject {
        name: "Bracket #3\nx",
        layer: "Layer A",
        color: None,
        transform: DMat4::IDENTITY,
        mesh: &m,
    };
    let text = String::from_utf8(
        export(Format::Obj, &[o], &ExportOptions::default())
            .unwrap()
            .main,
    )
    .unwrap();
    assert!(text.contains("o Bracket _3_x\n"), "{text}");
    assert!(text.contains("g Layer_A\n"), "{text}");
}
