//! glTF 2.0 binary (.glb) writer (SPEC §8.3). Own packer after the public
//! glTF 2.0 specification (Khronos): 12-byte header, JSON chunk, BIN chunk.
//! The JSON is written with `serde_json` (hand-built, no `gltf-json`).
//!
//! Always triangulated. Vertices are split per distinct (vertex, normal), so
//! hard edges become split normals. glTF is +Y up by definition, so the
//! writer always converts to Y-up regardless of `ExportOptions::up`.
//! Layers become parent nodes; body transforms stay on the object nodes.

use super::prep::{Material, Prepared, Trs, layers};
use serde_json::{Value, json};
use std::collections::HashMap;

const GLB_MAGIC: u32 = 0x4654_6C67; // "glTF"
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;
const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// sRGB-encoded CAD colour to linear (glTF `baseColorFactor` is linear).
fn srgb_to_linear(c: f32) -> f64 {
    let c = c.clamp(0.0, 1.0) as f64;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn node_trs(t: &Trs, v: &mut serde_json::Map<String, Value>) {
    if t.translation != glam::DVec3::ZERO {
        v.insert(
            "translation".into(),
            json!([t.translation.x, t.translation.y, t.translation.z]),
        );
    }
    if t.rotation != glam::DQuat::IDENTITY {
        let q = t.rotation;
        v.insert("rotation".into(), json!([q.x, q.y, q.z, q.w]));
    }
    if t.scale != glam::DVec3::ONE {
        v.insert("scale".into(), json!([t.scale.x, t.scale.y, t.scale.z]));
    }
}

struct Bin {
    data: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Bin {
    fn push_view(&mut self, bytes: &[u8], target: u32) -> usize {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        let off = self.data.len();
        self.data.extend_from_slice(bytes);
        self.views.push(
            json!({"buffer": 0, "byteOffset": off, "byteLength": bytes.len(), "target": target}),
        );
        self.views.len() - 1
    }
    fn push_accessor(&mut self, a: Value) -> usize {
        self.accessors.push(a);
        self.accessors.len() - 1
    }
}

pub fn write_glb(objects: &[Prepared], mats: &[Material], layers_as_nodes: bool) -> Vec<u8> {
    let mut bin = Bin {
        data: Vec::new(),
        views: Vec::new(),
        accessors: Vec::new(),
    };
    let mut nodes: Vec<Value> = Vec::new();
    let mut meshes: Vec<Value> = Vec::new();
    let mut object_nodes: Vec<usize> = Vec::new();

    for ob in objects {
        // Split vertices per (vertex, normal bits).
        let mut map: HashMap<(u32, [u64; 3]), u32> = HashMap::new();
        let mut pos: Vec<f32> = Vec::new();
        let mut nrm: Vec<f32> = Vec::new();
        let mut idx: Vec<u32> = Vec::new();
        for (p, ns) in ob.polys.iter().zip(&ob.normals) {
            let corner: Vec<u32> = p
                .iter()
                .zip(ns)
                .map(|(&v, n)| {
                    let key = (v, [n.x.to_bits(), n.y.to_bits(), n.z.to_bits()]);
                    let next = map.len() as u32;
                    *map.entry(key).or_insert_with(|| {
                        let q = ob.positions[v as usize];
                        pos.extend_from_slice(&[q.x as f32, q.y as f32, q.z as f32]);
                        nrm.extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);
                        next
                    })
                })
                .collect();
            // Polygons are triangles here; fan defensively if not.
            for k in 1..corner.len().saturating_sub(1) {
                idx.extend_from_slice(&[corner[0], corner[k], corner[k + 1]]);
            }
        }
        let mut node = serde_json::Map::new();
        node.insert("name".into(), json!(ob.name));
        node_trs(&ob.node, &mut node);
        if !idx.is_empty() {
            let nv = pos.len() / 3;
            let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            for c in pos.chunks_exact(3) {
                for a in 0..3 {
                    lo[a] = lo[a].min(c[a]);
                    hi[a] = hi[a].max(c[a]);
                }
            }
            let pv = bin.push_view(bytemuck::cast_slice(&pos), ARRAY_BUFFER);
            let pa = bin.push_accessor(json!({"bufferView": pv, "componentType": FLOAT, "count": nv, "type": "VEC3", "min": lo, "max": hi}));
            let nvw = bin.push_view(bytemuck::cast_slice(&nrm), ARRAY_BUFFER);
            let na = bin.push_accessor(
                json!({"bufferView": nvw, "componentType": FLOAT, "count": nv, "type": "VEC3"}),
            );
            let ia = if nv <= u16::MAX as usize {
                let small: Vec<u16> = idx.iter().map(|&i| i as u16).collect();
                let iv = bin.push_view(bytemuck::cast_slice(&small), ELEMENT_ARRAY_BUFFER);
                bin.push_accessor(json!({"bufferView": iv, "componentType": UNSIGNED_SHORT, "count": idx.len(), "type": "SCALAR"}))
            } else {
                let iv = bin.push_view(bytemuck::cast_slice(&idx), ELEMENT_ARRAY_BUFFER);
                bin.push_accessor(json!({"bufferView": iv, "componentType": UNSIGNED_INT, "count": idx.len(), "type": "SCALAR"}))
            };
            let mut prim =
                json!({"attributes": {"POSITION": pa, "NORMAL": na}, "indices": ia, "mode": 4});
            if let Some(m) = ob.material {
                prim["material"] = json!(m);
            }
            meshes.push(json!({"name": ob.name, "primitives": [prim]}));
            node.insert("mesh".into(), json!(meshes.len() - 1));
        }
        nodes.push(Value::Object(node));
        object_nodes.push(nodes.len() - 1);
    }

    let scene_nodes: Vec<usize> = if layers_as_nodes {
        let mut roots = Vec::new();
        for layer in layers(objects) {
            let children: Vec<usize> = objects
                .iter()
                .zip(&object_nodes)
                .filter(|(o, _)| o.layer == layer)
                .map(|(_, &n)| n)
                .collect();
            nodes.push(json!({"name": layer, "children": children}));
            roots.push(nodes.len() - 1);
        }
        roots
    } else {
        object_nodes.clone()
    };

    while !bin.data.len().is_multiple_of(4) {
        bin.data.push(0);
    }
    let materials: Vec<Value> = mats
        .iter()
        .map(|m| {
            let c = m.color.map(srgb_to_linear);
            json!({"name": m.name, "pbrMetallicRoughness": {"baseColorFactor": [c[0], c[1], c[2], 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.5}})
        })
        .collect();
    let mut root = json!({
        "asset": {"version": "2.0", "generator": concat!("newtopo ", env!("CARGO_PKG_VERSION"))},
        "scene": 0,
        "scenes": [{"name": "Scene", "nodes": scene_nodes}],
        "nodes": nodes,
    });
    if !meshes.is_empty() {
        root["meshes"] = json!(meshes);
        root["accessors"] = json!(bin.accessors);
        root["bufferViews"] = json!(bin.views);
        root["buffers"] = json!([{"byteLength": bin.data.len()}]);
    }
    if !materials.is_empty() {
        root["materials"] = json!(materials);
    }
    let mut js = serde_json::to_vec(&root).expect("json");
    while !js.len().is_multiple_of(4) {
        js.push(b' ');
    }
    let has_bin = !bin.data.is_empty();
    let total = 12 + 8 + js.len() + if has_bin { 8 + bin.data.len() } else { 0 };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&js);
    if has_bin {
        out.extend_from_slice(&(bin.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(&bin.data);
    }
    out
}
