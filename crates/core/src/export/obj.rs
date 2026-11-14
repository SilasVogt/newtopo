//! Wavefront OBJ + MTL writer (SPEC §8.3). Own writer.
//!
//! Per object: `o <name>`, a `g` line (the layer when layers become
//! hierarchy, else the object name), all vertices, unique per-corner normals
//! (`vn`, split at hard edges), `usemtl`, smoothing groups (`s`) and faces
//! `f v//vn`. Transforms are baked. Quads and n-gons are kept unless
//! triangulating.

use super::prep::{Material, Prepared};
use std::collections::HashMap;
use std::fmt::Write;

/// Default MTL file name referenced by `mtllib`; [`super::ExportFiles::save`]
/// rewrites it to match the saved file's stem.
pub const DEFAULT_MTL: &str = "newtopo.mtl";

/// Number formatting: rounded to 1e-6, shortest round-trip text.
pub fn num(x: f64) -> String {
    let r = (x * 1e6).round() / 1e6;
    if r == 0.0 {
        "0".to_string()
    } else {
        format!("{r}")
    }
}

/// Make a name safe for an OBJ/MTL statement: one line, no `#` (some
/// readers treat it as a comment anywhere on a line).
fn line_safe(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_control() || c == '#' { '_' } else { c })
        .collect();
    let t = t.trim();
    if t.is_empty() {
        "unnamed".to_string()
    } else {
        t.to_string()
    }
}

/// Group names are whitespace-separated lists in OBJ: no spaces.
fn group_safe(s: &str) -> String {
    line_safe(s)
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect()
}

pub fn write_obj(
    objects: &[Prepared],
    mats: &[Material],
    layers_as_groups: bool,
) -> (Vec<u8>, Vec<u8>) {
    let mut o = String::new();
    o.push_str("# newtopo OBJ export\n");
    if !mats.is_empty() {
        let _ = writeln!(o, "mtllib {DEFAULT_MTL}");
    }
    let mut v_base = 1usize;
    let mut n_base = 1usize;
    // `usemtl` stays in effect across `o` statements and OBJ has no "no
    // material" statement, so objects without a material go first.
    let mut order: Vec<&Prepared> = objects.iter().collect();
    order.sort_by_key(|o| o.material.is_some());
    for ob in order {
        let _ = writeln!(o, "o {}", line_safe(&ob.name));
        let _ = writeln!(
            o,
            "g {}",
            group_safe(if layers_as_groups {
                &ob.layer
            } else {
                &ob.name
            })
        );
        for p in &ob.positions {
            let _ = writeln!(o, "v {} {} {}", num(p.x), num(p.y), num(p.z));
        }
        // Unique normals (by printed text, so equal output shares one vn).
        let mut nidx: HashMap<String, usize> = HashMap::new();
        let mut corner_n: Vec<Vec<usize>> = Vec::with_capacity(ob.polys.len());
        for ns in &ob.normals {
            let mut row = Vec::with_capacity(ns.len());
            for n in ns {
                let key = format!("{} {} {}", num(n.x), num(n.y), num(n.z));
                let next = nidx.len();
                let id = *nidx.entry(key.clone()).or_insert_with(|| {
                    let _ = writeln!(o, "vn {key}");
                    next
                });
                row.push(id);
            }
            corner_n.push(row);
        }
        if let Some(m) = ob.material {
            let _ = writeln!(o, "usemtl {}", line_safe(&mats[m].name));
        }
        let mut cur_s: Option<u32> = None;
        for (pi, p) in ob.polys.iter().enumerate() {
            let s = ob.smoothing[pi];
            if cur_s != Some(s) {
                if s == 0 {
                    o.push_str("s off\n");
                } else {
                    let _ = writeln!(o, "s {s}");
                }
                cur_s = Some(s);
            }
            o.push('f');
            for (k, &v) in p.iter().enumerate() {
                let _ = write!(o, " {}//{}", v as usize + v_base, corner_n[pi][k] + n_base);
            }
            o.push('\n');
        }
        v_base += ob.positions.len();
        n_base += nidx.len();
    }
    let mut m = String::new();
    m.push_str("# newtopo MTL export\n");
    for mat in mats {
        let _ = writeln!(m, "\nnewmtl {}", line_safe(&mat.name));
        let [r, g, b] = mat.color.map(|c| c as f64);
        let _ = writeln!(m, "Kd {} {} {}", num(r), num(g), num(b));
        m.push_str("Ka 0 0 0\nKs 0 0 0\nd 1\nillum 1\n");
    }
    (o.into_bytes(), m.into_bytes())
}
