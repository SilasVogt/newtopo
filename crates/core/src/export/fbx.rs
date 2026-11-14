//! Binary FBX 7.4 writer (SPEC §8.3). Own writer. The container format
//! (header, node records, property encodings) follows the public "FBX binary
//! file format specification" post on the Blender developer blog; the
//! object/property names follow Autodesk's public FBX documentation. The
//! exact subset written is documented in `docs/fbx-subset.md`.

use super::ExportUp;
use super::prep::{Material, Prepared, layers};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::io::Write;

pub const FBX_VERSION: u32 = 7400;
/// Arrays larger than this many bytes are zlib-compressed (encoding 1).
const COMPRESS_MIN_BYTES: usize = 128;

/// One property value (type codes per the format post).
#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    Bool(bool),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Str(String),
    Raw(Vec<u8>),
    ArrI32(Vec<i32>),
    ArrI64(Vec<i64>),
    ArrF32(Vec<f32>),
    ArrF64(Vec<f64>),
    ArrBool(Vec<bool>),
}

/// One node record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(name: &str) -> Self {
        Node {
            name: name.to_string(),
            props: Vec::new(),
            children: Vec::new(),
        }
    }
    pub fn p(mut self, p: Prop) -> Self {
        self.props.push(p);
        self
    }
    pub fn c(mut self, n: Node) -> Self {
        self.children.push(n);
        self
    }
    pub fn leaf(name: &str, p: Prop) -> Self {
        Node::new(name).p(p)
    }
}

fn s(x: &str) -> Prop {
    Prop::Str(x.to_string())
}

fn write_array(out: &mut Vec<u8>, code: u8, count: usize, raw: &[u8]) {
    out.push(code);
    out.extend_from_slice(&(count as u32).to_le_bytes());
    if raw.len() > COMPRESS_MIN_BYTES {
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        z.write_all(raw).expect("zlib to memory");
        let zb = z.finish().expect("zlib to memory");
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(zb.len() as u32).to_le_bytes());
        out.extend_from_slice(&zb);
    } else {
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        out.extend_from_slice(raw);
    }
}

fn write_prop(out: &mut Vec<u8>, p: &Prop) {
    match p {
        Prop::Bool(b) => {
            out.push(b'C');
            out.push(u8::from(*b));
        }
        Prop::I16(v) => {
            out.push(b'Y');
            out.extend_from_slice(&v.to_le_bytes());
        }
        Prop::I32(v) => {
            out.push(b'I');
            out.extend_from_slice(&v.to_le_bytes());
        }
        Prop::I64(v) => {
            out.push(b'L');
            out.extend_from_slice(&v.to_le_bytes());
        }
        Prop::F32(v) => {
            out.push(b'F');
            out.extend_from_slice(&v.to_le_bytes());
        }
        Prop::F64(v) => {
            out.push(b'D');
            out.extend_from_slice(&v.to_le_bytes());
        }
        Prop::Str(v) => {
            out.push(b'S');
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
            out.extend_from_slice(v.as_bytes());
        }
        Prop::Raw(v) => {
            out.push(b'R');
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
            out.extend_from_slice(v);
        }
        Prop::ArrI32(v) => write_array(out, b'i', v.len(), bytemuck::cast_slice(v)),
        Prop::ArrI64(v) => write_array(out, b'l', v.len(), bytemuck::cast_slice(v)),
        Prop::ArrF32(v) => write_array(out, b'f', v.len(), bytemuck::cast_slice(v)),
        Prop::ArrF64(v) => write_array(out, b'd', v.len(), bytemuck::cast_slice(v)),
        Prop::ArrBool(v) => {
            let raw: Vec<u8> = v.iter().map(|&b| u8::from(b)).collect();
            write_array(out, b'b', v.len(), &raw)
        }
    }
}

const NULL_RECORD: [u8; 13] = [0; 13];

/// Append one node record. `EndOffset` is absolute from the file start.
fn write_node(out: &mut Vec<u8>, n: &Node) {
    let start = out.len();
    out.extend_from_slice(&[0u8; 12]); // EndOffset, NumProperties, PropertyListLen
    let name = n.name.as_bytes();
    assert!(name.len() <= 255, "FBX node name too long");
    out.push(name.len() as u8);
    out.extend_from_slice(name);
    let pstart = out.len();
    for p in &n.props {
        write_prop(out, p);
    }
    let plen = out.len() - pstart;
    for c in &n.children {
        write_node(out, c);
    }
    // Terminate nested lists; property-less nodes also get one (as the SDK
    // does) so readers see an explicit empty list.
    if !n.children.is_empty() || n.props.is_empty() {
        out.extend_from_slice(&NULL_RECORD);
    }
    let end = out.len() as u32;
    out[start..start + 4].copy_from_slice(&end.to_le_bytes());
    out[start + 4..start + 8].copy_from_slice(&(n.props.len() as u32).to_le_bytes());
    out[start + 8..start + 12].copy_from_slice(&(plen as u32).to_le_bytes());
}

/// Serialise a whole file: header, top-level nodes, closing NULL record.
pub fn encode(top: &[Node]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"Kaydara FBX Binary  \x00");
    out.extend_from_slice(&[0x1A, 0x00]);
    out.extend_from_slice(&FBX_VERSION.to_le_bytes());
    for n in top {
        write_node(&mut out, n);
    }
    out.extend_from_slice(&NULL_RECORD);
    out
}

// ---- Scene ------------------------------------------------------------------

/// `Properties70` entry: P: name, type, label, flags, values...
fn p70(name: &str, ty: &str, label: &str, flags: &str, vals: Vec<Prop>) -> Node {
    let mut n = Node::new("P").p(s(name)).p(s(ty)).p(s(label)).p(s(flags));
    n.props.extend(vals);
    n
}

/// FBX object name: `name\x00\x01Class`. Strips characters that would break
/// the separator.
fn obj_name(name: &str, class: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|&c| c != '\0' && c != '\u{1}')
        .collect();
    format!("{clean}\u{0}\u{1}{class}")
}

fn layer_element_ref(ty: &str) -> Node {
    Node::new("LayerElement")
        .c(Node::leaf("Type", s(ty)))
        .c(Node::leaf("TypedIndex", Prop::I32(0)))
}

fn global_settings(up: ExportUp) -> Node {
    // Axis convention: right-handed. Y-up: up +Y, front +Z, coord +X.
    // Z-up: up +Z, front -Y, coord +X.
    let (up_axis, front, front_sign) = match up {
        ExportUp::Y => (1, 2, 1),
        ExportUp::Z => (2, 1, -1),
    };
    let int = |n: &str, v: i32| p70(n, "int", "Integer", "", vec![Prop::I32(v)]);
    let dbl = |n: &str, v: f64| p70(n, "double", "Number", "", vec![Prop::F64(v)]);
    Node::new("GlobalSettings")
        .c(Node::leaf("Version", Prop::I32(1000)))
        .c(Node::new("Properties70")
            .c(int("UpAxis", up_axis))
            .c(int("UpAxisSign", 1))
            .c(int("FrontAxis", front))
            .c(int("FrontAxisSign", front_sign))
            .c(int("CoordAxis", 0))
            .c(int("CoordAxisSign", 1))
            .c(int("OriginalUpAxis", up_axis))
            .c(int("OriginalUpAxisSign", 1))
            // 1 file unit = 1 metre (the unit is centimetres, factor 100).
            .c(dbl("UnitScaleFactor", 100.0))
            .c(dbl("OriginalUnitScaleFactor", 100.0))
            .c(p70(
                "AmbientColor",
                "ColorRGB",
                "Color",
                "",
                vec![Prop::F64(0.0), Prop::F64(0.0), Prop::F64(0.0)],
            ))
            .c(p70(
                "DefaultCamera",
                "KString",
                "",
                "",
                vec![s("Producer Perspective")],
            ))
            .c(p70("TimeMode", "enum", "", "", vec![Prop::I32(11)]))
            .c(p70(
                "TimeSpanStart",
                "KTime",
                "Time",
                "",
                vec![Prop::I64(0)],
            ))
            .c(p70(
                "TimeSpanStop",
                "KTime",
                "Time",
                "",
                vec![Prop::I64(46_186_158_000)],
            ))
            .c(dbl("CustomFrameRate", -1.0)))
}

fn header_extension() -> Node {
    // Fixed timestamp keeps output reproducible (golden tests, diffs).
    let ts = Node::new("CreationTimeStamp")
        .c(Node::leaf("Version", Prop::I32(1000)))
        .c(Node::leaf("Year", Prop::I32(2026)))
        .c(Node::leaf("Month", Prop::I32(1)))
        .c(Node::leaf("Day", Prop::I32(1)))
        .c(Node::leaf("Hour", Prop::I32(0)))
        .c(Node::leaf("Minute", Prop::I32(0)))
        .c(Node::leaf("Second", Prop::I32(0)))
        .c(Node::leaf("Millisecond", Prop::I32(0)));
    Node::new("FBXHeaderExtension")
        .c(Node::leaf("FBXHeaderVersion", Prop::I32(1003)))
        .c(Node::leaf("FBXVersion", Prop::I32(FBX_VERSION as i32)))
        .c(Node::leaf("EncryptionType", Prop::I32(0)))
        .c(ts)
        .c(Node::leaf(
            "Creator",
            s(concat!("newtopo ", env!("CARGO_PKG_VERSION"))),
        ))
}

fn model(id: i64, name: &str, kind: &str, t: &super::prep::Trs) -> Node {
    let (rz, ry, rx) = t.rotation.to_euler(glam::EulerRot::ZYX);
    let v3 = |v: [f64; 3]| v.iter().map(|&x| Prop::F64(x)).collect::<Vec<_>>();
    Node::new("Model")
        .p(Prop::I64(id))
        .p(s(&obj_name(name, "Model")))
        .p(s(kind))
        .c(Node::leaf("Version", Prop::I32(232)))
        .c(Node::new("Properties70")
            .c(p70(
                "Lcl Translation",
                "Lcl Translation",
                "",
                "A",
                v3(t.translation.to_array()),
            ))
            .c(p70(
                "Lcl Rotation",
                "Lcl Rotation",
                "",
                "A",
                v3([rx.to_degrees(), ry.to_degrees(), rz.to_degrees()]),
            ))
            .c(p70(
                "Lcl Scaling",
                "Lcl Scaling",
                "",
                "A",
                v3(t.scale.to_array()),
            ))
            .c(p70(
                "DefaultAttributeIndex",
                "int",
                "Integer",
                "",
                vec![Prop::I32(0)],
            ))
            .c(p70("InheritType", "enum", "", "", vec![Prop::I32(1)])))
        .c(Node::leaf("Shading", Prop::Bool(true)))
        .c(Node::leaf("Culling", s("CullingOff")))
}

fn geometry(id: i64, ob: &Prepared) -> Node {
    let verts: Vec<f64> = ob.positions.iter().flat_map(|p| p.to_array()).collect();
    let mut pvi: Vec<i32> = Vec::with_capacity(ob.corner_count());
    let mut first_corner = Vec::with_capacity(ob.polys.len());
    for p in &ob.polys {
        first_corner.push(pvi.len() as i32);
        for (k, &v) in p.iter().enumerate() {
            // The last index of each polygon is stored as -(index + 1).
            pvi.push(if k + 1 == p.len() {
                !(v as i32)
            } else {
                v as i32
            });
        }
    }
    let edges: Vec<i32> = ob
        .edges
        .iter()
        .map(|e| first_corner[e.poly as usize] + e.corner as i32)
        .collect();
    let smoothing: Vec<i32> = ob.edges.iter().map(|e| i32::from(!e.hard)).collect();
    let normals: Vec<f64> = ob
        .normals
        .iter()
        .flatten()
        .flat_map(|n| n.to_array())
        .collect();
    let mut layer = Node::new("Layer")
        .p(Prop::I32(0))
        .c(Node::leaf("Version", Prop::I32(100)))
        .c(layer_element_ref("LayerElementNormal"))
        .c(layer_element_ref("LayerElementSmoothing"));
    let mut g = Node::new("Geometry")
        .p(Prop::I64(id))
        .p(s(&obj_name(&ob.name, "Geometry")))
        .p(s("Mesh"))
        .c(Node::new("Properties70"))
        .c(Node::leaf("GeometryVersion", Prop::I32(124)))
        .c(Node::leaf("Vertices", Prop::ArrF64(verts)))
        .c(Node::leaf("PolygonVertexIndex", Prop::ArrI32(pvi)))
        .c(Node::leaf("Edges", Prop::ArrI32(edges)))
        .c(Node::new("LayerElementNormal")
            .p(Prop::I32(0))
            .c(Node::leaf("Version", Prop::I32(101)))
            .c(Node::leaf("Name", s("")))
            .c(Node::leaf("MappingInformationType", s("ByPolygonVertex")))
            .c(Node::leaf("ReferenceInformationType", s("Direct")))
            .c(Node::leaf("Normals", Prop::ArrF64(normals))))
        .c(Node::new("LayerElementSmoothing")
            .p(Prop::I32(0))
            .c(Node::leaf("Version", Prop::I32(102)))
            .c(Node::leaf("Name", s("")))
            .c(Node::leaf("MappingInformationType", s("ByEdge")))
            .c(Node::leaf("ReferenceInformationType", s("Direct")))
            .c(Node::leaf("Smoothing", Prop::ArrI32(smoothing))));
    if ob.material.is_some() {
        g = g.c(Node::new("LayerElementMaterial")
            .p(Prop::I32(0))
            .c(Node::leaf("Version", Prop::I32(101)))
            .c(Node::leaf("Name", s("")))
            .c(Node::leaf("MappingInformationType", s("AllSame")))
            .c(Node::leaf("ReferenceInformationType", s("IndexToDirect")))
            .c(Node::leaf("Materials", Prop::ArrI32(vec![0]))));
        layer = layer.c(layer_element_ref("LayerElementMaterial"));
    }
    g.c(layer)
}

fn material(id: i64, m: &Material) -> Node {
    let c = m.color.map(|x| Prop::F64((x as f64 * 1e6).round() / 1e6));
    Node::new("Material")
        .p(Prop::I64(id))
        .p(s(&obj_name(&m.name, "Material")))
        .p(s(""))
        .c(Node::leaf("Version", Prop::I32(102)))
        .c(Node::leaf("ShadingModel", s("lambert")))
        .c(Node::leaf("MultiLayer", Prop::I32(0)))
        .c(Node::new("Properties70")
            .c(p70("DiffuseColor", "Color", "", "A", c.to_vec()))
            .c(p70(
                "DiffuseFactor",
                "Number",
                "",
                "A",
                vec![Prop::F64(1.0)],
            ))
            .c(p70(
                "AmbientColor",
                "Color",
                "",
                "A",
                vec![Prop::F64(0.0), Prop::F64(0.0), Prop::F64(0.0)],
            )))
}

fn connect(child: i64, parent: i64) -> Node {
    Node::new("C")
        .p(s("OO"))
        .p(Prop::I64(child))
        .p(Prop::I64(parent))
}

/// Build the node tree for a scene.
pub fn scene_nodes(
    objects: &[Prepared],
    mats: &[Material],
    up: ExportUp,
    layers_as_nodes: bool,
) -> Vec<Node> {
    let mut next_id = 1_000_000i64;
    let mut id = || {
        next_id += 1;
        next_id
    };
    let mut objs = Node::new("Objects");
    let mut conns = Node::new("Connections");
    let (mut n_model, mut n_geom, mut n_attr) = (0, 0, 0);

    let mat_ids: Vec<i64> = mats.iter().map(|_| id()).collect();
    for (m, &mid) in mats.iter().zip(&mat_ids) {
        objs = objs.c(material(mid, m));
    }
    let mut layer_ids: Vec<(String, i64)> = Vec::new();
    if layers_as_nodes {
        for l in layers(objects) {
            let (mid, aid) = (id(), id());
            objs = objs
                .c(Node::new("NodeAttribute")
                    .p(Prop::I64(aid))
                    .p(s(&obj_name("", "NodeAttribute")))
                    .p(s("Null"))
                    .c(Node::leaf("TypeFlags", s("Null"))))
                .c(model(mid, &l, "Null", &super::prep::Trs::IDENTITY));
            conns = conns.c(connect(mid, 0)).c(connect(aid, mid));
            layer_ids.push((l, mid));
            n_model += 1;
            n_attr += 1;
        }
    }
    for ob in objects {
        let mid = id();
        let parent = layer_ids
            .iter()
            .find(|(l, _)| *l == ob.layer)
            .map(|(_, i)| *i)
            .unwrap_or(0);
        if ob.polys.is_empty() {
            objs = objs.c(model(mid, &ob.name, "Null", &ob.node));
            conns = conns.c(connect(mid, parent));
            n_model += 1;
            continue;
        }
        let gid = id();
        objs = objs
            .c(geometry(gid, ob))
            .c(model(mid, &ob.name, "Mesh", &ob.node));
        conns = conns.c(connect(mid, parent)).c(connect(gid, mid));
        if let Some(m) = ob.material {
            conns = conns.c(connect(mat_ids[m], mid));
        }
        n_model += 1;
        n_geom += 1;
    }
    let def = |ty: &str, n: i32| {
        Node::new("ObjectType")
            .p(s(ty))
            .c(Node::leaf("Count", Prop::I32(n)))
    };
    let mut defs = Node::new("Definitions")
        .c(Node::leaf("Version", Prop::I32(100)))
        .c(Node::leaf(
            "Count",
            Prop::I32(1 + n_model + n_geom + n_attr + mats.len() as i32),
        ))
        .c(def("GlobalSettings", 1));
    if n_model > 0 {
        defs = defs.c(def("Model", n_model));
    }
    if n_geom > 0 {
        defs = defs.c(def("Geometry", n_geom));
    }
    if n_attr > 0 {
        defs = defs.c(def("NodeAttribute", n_attr));
    }
    if !mats.is_empty() {
        defs = defs.c(def("Material", mats.len() as i32));
    }
    let doc_id = 999_999i64;
    vec![
        header_extension(),
        Node::leaf("FileId", Prop::Raw(b"newtopo-fbx-7400".to_vec())),
        Node::leaf("CreationTime", s("2026-01-01 00:00:00:000")),
        Node::leaf("Creator", s(concat!("newtopo ", env!("CARGO_PKG_VERSION")))),
        global_settings(up),
        Node::new("Documents")
            .c(Node::leaf("Count", Prop::I32(1)))
            .c(Node::new("Document")
                .p(Prop::I64(doc_id))
                .p(s("Scene"))
                .p(s("Scene"))
                .c(Node::new("Properties70")
                    .c(p70("SourceObject", "object", "", "", vec![]))
                    .c(p70("ActiveAnimStackName", "KString", "", "", vec![s("")])))
                .c(Node::leaf("RootNode", Prop::I64(0)))),
        Node::new("References"),
        defs,
        objs,
        conns,
    ]
}

pub fn write_fbx(
    objects: &[Prepared],
    mats: &[Material],
    up: ExportUp,
    layers_as_nodes: bool,
) -> Vec<u8> {
    encode(&scene_nodes(objects, mats, up, layers_as_nodes))
}
