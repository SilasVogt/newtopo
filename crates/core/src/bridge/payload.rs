//! Push payload (SPEC §9.3): object metadata in the header's `layout`,
//! geometry as little-endian arrays in the binary payload.
//!
//! Per object, arrays (all 4-byte aligned, offsets from payload start):
//!
//! | name         | dtype | width | count          | meaning                         |
//! | ------------ | ----- | ----- | -------------- | ------------------------------- |
//! | `positions`  | f32   | 3     | vertices       | body-space positions            |
//! | `poly_sizes` | u32   | 1     | polygons       | corners per polygon             |
//! | `poly_verts` | u32   | 1     | corners        | vertex index per corner         |
//! | `normals`    | f32   | 3     | corners        | unit normal per corner (split at hard edges) |
//! | `hard_edges` | u32   | 2     | hard edges     | vertex index pairs              |
//! | `uvs`        | f32   | 2     | corners        | optional, per corner            |
//!
//! Positions are in body space, Z-up (the scene convention); `transform`
//! (column-major 4x4) places the body in the scene. Hosts multiply
//! positions and the transform's translation by `units.scale` and convert
//! the up axis themselves.

use crate::export::prep::{TransformMode, material_name, prepare};
use crate::export::{ExportObject, ExportOptions, ExportUp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One object to send: an export object plus optional per-corner UVs (in
/// `OutputMesh::polygons` corner order; ignored if the length doesn't match).
pub struct BridgeObject<'a> {
    pub object: ExportObject<'a>,
    pub uvs: Option<&'a [[f32; 2]]>,
}

impl<'a> From<ExportObject<'a>> for BridgeObject<'a> {
    fn from(object: ExportObject<'a>) -> Self {
        BridgeObject { object, uvs: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DType {
    F32,
    U32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArraySpec {
    pub offset: u64,
    pub count: u64,
    pub dtype: DType,
    pub width: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialSpec {
    pub name: String,
    pub color: [f32; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectLayout {
    pub name: String,
    pub layer: String,
    /// Column-major 4x4 body -> scene transform.
    pub transform: [f64; 16],
    pub material: Option<MaterialSpec>,
    pub arrays: BTreeMap<String, ArraySpec>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Units {
    /// Multiply positions and translations by this in the host.
    pub scale: f64,
    /// Up axis of the payload: always "z".
    pub up: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub units: Units,
    /// Keep existing same-named objects instead of replacing them.
    pub keep_both: bool,
    pub objects: Vec<ObjectLayout>,
}

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn array<T: bytemuck::Pod>(&mut self, data: &[T], dtype: DType, width: u32) -> ArraySpec {
        while !self.buf.len().is_multiple_of(4) {
            self.buf.push(0);
        }
        let offset = self.buf.len() as u64;
        self.buf.extend_from_slice(bytemuck::cast_slice(data));
        ArraySpec {
            offset,
            count: (data.len() / width as usize) as u64,
            dtype,
            width,
        }
    }
}

/// Build the layout and payload for `objects`.
pub fn build(objects: &[BridgeObject], scale: f64, keep_both: bool) -> (Layout, Vec<u8>) {
    let opts = ExportOptions {
        up: ExportUp::Z,
        scale: 1.0,
        triangulate: false,
        ..ExportOptions::default()
    };
    let mut w = Writer { buf: Vec::new() };
    let mut layouts = Vec::with_capacity(objects.len());
    for bo in objects {
        let o = &bo.object;
        let p = prepare(o, &opts, TransformMode::Body, false, None);
        let mut arrays = BTreeMap::new();
        let pos: Vec<f32> = p
            .positions
            .iter()
            .flat_map(|v| [v.x as f32, v.y as f32, v.z as f32])
            .collect();
        arrays.insert("positions".into(), w.array(&pos, DType::F32, 3));
        let sizes: Vec<u32> = p.polys.iter().map(|q| q.len() as u32).collect();
        arrays.insert("poly_sizes".into(), w.array(&sizes, DType::U32, 1));
        let verts: Vec<u32> = p.polys.iter().flatten().copied().collect();
        arrays.insert("poly_verts".into(), w.array(&verts, DType::U32, 1));
        let nrm: Vec<f32> = p
            .normals
            .iter()
            .flatten()
            .flat_map(|n| [n.x as f32, n.y as f32, n.z as f32])
            .collect();
        arrays.insert("normals".into(), w.array(&nrm, DType::F32, 3));
        let hard: Vec<u32> = p
            .edges
            .iter()
            .filter(|e| e.hard)
            .flat_map(|e| [e.a, e.b])
            .collect();
        arrays.insert("hard_edges".into(), w.array(&hard, DType::U32, 2));
        if let Some(uvs) = bo.uvs {
            // Keep the UVs of the polygons `prepare` kept (it skips invalid ones).
            let nv = o.mesh.vertices.len();
            let total: usize = o.mesh.polygons.iter().map(|q| q.verts.len()).sum();
            if uvs.len() == total {
                let mut flat = Vec::with_capacity(verts.len() * 2);
                let mut at = 0;
                for q in &o.mesh.polygons {
                    let n = q.verts.len();
                    if n >= 3 && q.verts.iter().all(|&v| (v as usize) < nv) {
                        flat.extend(uvs[at..at + n].iter().flat_map(|uv| *uv));
                    }
                    at += n;
                }
                arrays.insert("uvs".into(), w.array(&flat, DType::F32, 2));
            }
        }
        let material = o.color.map(|c| MaterialSpec {
            name: material_name(c),
            color: c,
        });
        layouts.push(ObjectLayout {
            name: o.name.to_string(),
            layer: o.layer.to_string(),
            transform: o.transform.to_cols_array(),
            material,
            arrays,
        });
    }
    (
        Layout {
            units: Units {
                scale,
                up: "z".into(),
            },
            keep_both,
            objects: layouts,
        },
        w.buf,
    )
}

/// Read an array back (tests and diagnostics).
pub fn read_u32(payload: &[u8], a: &ArraySpec) -> Vec<u32> {
    let n = (a.count * a.width as u64) as usize;
    let off = a.offset as usize;
    payload[off..off + 4 * n]
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().expect("4 bytes")))
        .collect()
}

/// Read an array back (tests and diagnostics).
pub fn read_f32(payload: &[u8], a: &ArraySpec) -> Vec<f32> {
    let n = (a.count * a.width as u64) as usize;
    let off = a.offset as usize;
    payload[off..off + 4 * n]
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().expect("4 bytes")))
        .collect()
}
