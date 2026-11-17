#![allow(dead_code)]
use glam::DMat4;
use newtopo_core::Scene;
use newtopo_core::eval::IssueKind;
use newtopo_core::mesh::{OutEdgeKind, OutputMesh};
use newtopo_kernel::mock::{MockBody, MockKernel};
use newtopo_kernel::{BodyAttributes, BodyId, HealReport};
use std::sync::Arc;

pub fn scene_with(body: MockBody) -> (Scene, BodyId) {
    let k = Arc::new(MockKernel::new());
    let mut s = Scene::new(k);
    let attrs = BodyAttributes {
        name: body.name.clone(),
        transform: DMat4::IDENTITY,
        ..Default::default()
    };
    let id = s
        .add_body(&body.to_brep_bytes(), attrs, HealReport::default())
        .unwrap();
    (s, id)
}

pub fn assert_valid(s: &mut Scene, b: BodyId, what: &str) {
    let issues: Vec<_> = s
        .validate(b)
        .into_iter()
        .filter(|i| i.kind != IssueKind::FanFallback)
        .collect();
    if !issues.is_empty() {
        let m = s.output_of(b).unwrap().clone();
        for i in issues.iter().take(10) {
            eprintln!(
                "  {:?}: {} {:?}",
                i.kind,
                i.message,
                i.vertices
                    .iter()
                    .map(|v| (v, &m.vertices[*v as usize].key))
                    .collect::<Vec<_>>()
            );
        }
        panic!("{what}: {} validation issue(s)", issues.len());
    }
}

pub fn line_edges(m: &OutputMesh) -> Vec<usize> {
    m.edges
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, OutEdgeKind::Line(_)))
        .map(|(i, _)| i)
        .collect()
}

pub fn set_autofill(s: &mut Scene, on: bool) {
    s.apply(newtopo_core::Op::SetAutofill { on }).unwrap();
}

// ---- Blender test helpers ----

use serde_json::Value;
use std::process::{Command, Stdio};

/// Blender executable: `NEWTOPO_BLENDER_CMD` or `blender`.
pub fn blender_cmd() -> String {
    std::env::var("NEWTOPO_BLENDER_CMD").unwrap_or_else(|_| "blender".to_string())
}

/// Run Blender headless (`-b --factory-startup`) with `pre` arguments and
/// `post` script arguments, and parse the `NEWTOPO_JSON` line it prints.
pub fn blender_json(pre: &[&str], post: &[String]) -> Value {
    let out = Command::new(blender_cmd())
        .args(["-b", "--factory-startup"])
        .args(pre)
        .args(post)
        .stdin(Stdio::null())
        .output()
        .expect("failed to start Blender (set NEWTOPO_BLENDER_CMD)");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("NEWTOPO_JSON "))
        .unwrap_or_else(|| {
            panic!(
                "no NEWTOPO_JSON line; status {:?}\nstdout:\n{stdout}\nstderr:\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            )
        });
    serde_json::from_str(line).expect("bad JSON from Blender")
}

pub fn num(v: &Value) -> u64 {
    v.as_u64().unwrap_or_else(|| panic!("not a number: {v}"))
}
