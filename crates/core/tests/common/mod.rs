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
