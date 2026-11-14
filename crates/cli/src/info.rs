//! `newtopo-cli info <model>`: imports through the helper process and prints
//! per-body statistics (SPEC §10, P0 exit criterion).

use anyhow::{Context, Result};
use clap::Args;
use newtopo_kernel::helper::{ImportOptions, ImportedFile, run_import};
use newtopo_kernel::{BodyTopology, CadKernel, HealReport, SurfaceKind};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Args, Debug)]
pub struct InfoArgs {
    /// Model file (.step .stp .iges .igs .brep).
    pub model: PathBuf,
    /// Machine-readable JSON output.
    #[arg(long)]
    pub json: bool,
    /// Skip healing on import.
    #[arg(long)]
    pub no_heal: bool,
    /// Sewing tolerance in model units.
    #[arg(long, default_value_t = 1e-3)]
    pub sewing_tol: f64,
    /// Target length unit.
    #[arg(long, default_value = "mm")]
    pub units: String,
    /// Sharp-edge threshold in degrees (SPEC §2).
    #[arg(long, default_value_t = 30.0)]
    pub sharp_angle: f64,
    /// Import timeout in seconds.
    #[arg(long, default_value_t = 300)]
    pub timeout: u64,
    /// Path to the newtopo-import helper (default: next to this executable).
    #[arg(long)]
    pub helper: Option<PathBuf>,
}

#[derive(Debug, Default, Serialize)]
pub struct EdgeStats {
    pub total: usize,
    pub seam: usize,
    pub degenerate: usize,
    pub sharp: usize,
    pub boundary: usize,
    pub closed: usize,
}

#[derive(Debug, Serialize)]
pub struct BodyInfo {
    pub name: String,
    pub layer: Option<String>,
    pub color: Option<String>,
    pub assembly_path: Vec<String>,
    pub faces: usize,
    pub surface_types: BTreeMap<String, usize>,
    pub edges: EdgeStats,
    pub vertices: usize,
    pub area: f64,
    pub heal: HealReport,
}

#[derive(Debug, Serialize)]
pub struct InfoReport {
    pub file: String,
    pub source_sha256: String,
    pub kernel: String,
    pub units: String,
    pub bodies: Vec<BodyInfo>,
    pub faces: usize,
    pub surface_types: BTreeMap<String, usize>,
    pub edges: EdgeStats,
    pub layers: BTreeMap<String, usize>,
    pub colors: BTreeMap<String, usize>,
    /// Added in P5 (SPEC §12).
    pub link_sets: Option<usize>,
    pub symmetry: Option<String>,
    pub warnings: Vec<String>,
}

fn hex(c: [f32; 3]) -> String {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

fn edge_stats(t: &BodyTopology, sharp: f64) -> EdgeStats {
    let mut s = EdgeStats { total: t.edges.len(), ..Default::default() };
    for e in &t.edges {
        s.seam += e.seam as usize;
        s.degenerate += e.degenerate as usize;
        s.closed += e.closed as usize;
        s.boundary += (e.faces.len() == 1 && !e.seam && !e.degenerate) as usize;
        s.sharp += e.dihedral.is_some_and(|d| d > sharp) as usize;
    }
    s
}

fn add_edges(a: &mut EdgeStats, b: &EdgeStats) {
    a.total += b.total;
    a.seam += b.seam;
    a.degenerate += b.degenerate;
    a.sharp += b.sharp;
    a.boundary += b.boundary;
    a.closed += b.closed;
}

/// Builds the report from an imported file using `kernel` for topology.
pub fn report(
    file_name: String,
    units: String,
    imported: &ImportedFile,
    kernel: &dyn CadKernel,
    sharp_deg: f64,
) -> Result<InfoReport> {
    let sharp = sharp_deg.to_radians();
    let mut r = InfoReport {
        file: file_name,
        source_sha256: imported.source_sha256.clone(),
        kernel: kernel.name(),
        units,
        bodies: Vec::new(),
        faces: 0,
        surface_types: BTreeMap::new(),
        edges: EdgeStats::default(),
        layers: BTreeMap::new(),
        colors: BTreeMap::new(),
        link_sets: None,
        symmetry: None,
        warnings: imported.warnings.clone(),
    };
    for b in &imported.bodies {
        let id = kernel.load_brep(&b.brep).with_context(|| format!("loading body {}", b.attrs.name))?;
        let topo = kernel.topology(id);
        kernel.unload(id);
        let topo = topo.with_context(|| format!("topology of {}", b.attrs.name))?;
        let mut hist = BTreeMap::new();
        for f in &topo.faces {
            *hist.entry(f.surface.kind.name().to_string()).or_insert(0) += 1;
            *r.surface_types.entry(f.surface.kind.name().to_string()).or_insert(0) += 1;
        }
        let edges = edge_stats(&topo, sharp);
        add_edges(&mut r.edges, &edges);
        r.faces += topo.faces.len();
        if let Some(l) = &b.attrs.layer {
            *r.layers.entry(l.clone()).or_insert(0) += 1;
        }
        let color = b.attrs.color.map(hex);
        if let Some(c) = &color {
            *r.colors.entry(c.clone()).or_insert(0) += 1;
        }
        r.bodies.push(BodyInfo {
            name: b.attrs.name.clone(),
            layer: b.attrs.layer.clone(),
            color,
            assembly_path: b.attrs.assembly_path.clone(),
            faces: topo.faces.len(),
            surface_types: hist,
            edges,
            vertices: topo.vertices.len(),
            area: topo.faces.iter().map(|f| f.area).sum(),
            heal: b.heal.clone(),
        });
    }
    Ok(r)
}

fn hist_text(h: &BTreeMap<String, usize>) -> String {
    // Fixed order: the SurfaceKind order, then anything else.
    let order = [
        SurfaceKind::Plane,
        SurfaceKind::Cylinder,
        SurfaceKind::Cone,
        SurfaceKind::Sphere,
        SurfaceKind::Torus,
        SurfaceKind::Revolution,
        SurfaceKind::Extrusion,
        SurfaceKind::BSpline,
        SurfaceKind::Other,
    ];
    order
        .iter()
        .filter_map(|k| h.get(k.name()).map(|n| format!("{} {n}", k.name())))
        .collect::<Vec<_>>()
        .join(", ")
}

fn edges_text(e: &EdgeStats) -> String {
    format!(
        "{} (seam {}, degenerate {}, sharp {}, boundary {}, closed {})",
        e.total, e.seam, e.degenerate, e.sharp, e.boundary, e.closed
    )
}

fn counts_text(m: &BTreeMap<String, usize>) -> String {
    if m.is_empty() {
        return "none".into();
    }
    m.iter().map(|(k, n)| format!("{k} ({n})")).collect::<Vec<_>>().join(", ")
}

/// Human-readable report.
pub fn render_text(r: &InfoReport) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "file:     {}", r.file);
    let _ = writeln!(s, "sha256:   {}", r.source_sha256);
    let _ = writeln!(s, "kernel:   {}  units: {}", r.kernel, r.units);
    let _ = writeln!(s, "bodies:   {}", r.bodies.len());
    for (i, b) in r.bodies.iter().enumerate() {
        let mut head = format!("  [{i}] {}", b.name);
        if !b.assembly_path.is_empty() {
            let _ = write!(head, "  path={}", b.assembly_path.join("/"));
        }
        if let Some(l) = &b.layer {
            let _ = write!(head, "  layer={l}");
        }
        if let Some(c) = &b.color {
            let _ = write!(head, "  colour={c}");
        }
        let _ = writeln!(s, "{head}");
        let _ = writeln!(s, "      faces {} ({})", b.faces, hist_text(&b.surface_types));
        let _ = writeln!(s, "      edges {}", edges_text(&b.edges));
        let _ = writeln!(s, "      vertices {}  area {:.3}", b.vertices, b.area);
        let h = &b.heal;
        let _ = write!(s, "      heal: faces {} -> {}", h.faces_before, h.faces_after);
        if !h.failed_faces.is_empty() {
            let ids: Vec<String> = h.failed_faces.iter().map(|f| f.to_string()).collect();
            let _ = write!(s, ", failed faces [{}]", ids.join(", "));
        }
        let _ = writeln!(s);
        for f in &h.fixes {
            let _ = writeln!(s, "        - {f}");
        }
        if h.faces_after < h.faces_before {
            let _ = writeln!(
                s,
                "        ! faces are missing after healing: the file is probably damaged; re-export it, \
                 or try IGES or another STEP schema"
            );
        }
    }
    let _ = writeln!(s, "faces:    {} ({})", r.faces, hist_text(&r.surface_types));
    let _ = writeln!(s, "edges:    {}", edges_text(&r.edges));
    let _ = writeln!(s, "layers:   {}", counts_text(&r.layers));
    let _ = writeln!(s, "colours:  {}", counts_text(&r.colors));
    let _ = writeln!(s, "link sets: {}", r.link_sets.map_or("n/a".into(), |n| n.to_string()));
    let _ = writeln!(s, "symmetry: {}", r.symmetry.clone().unwrap_or_else(|| "n/a".into()));
    for w in &r.warnings {
        let _ = writeln!(s, "warning:  {w}");
    }
    s
}

#[cfg(feature = "occt")]
fn kernel() -> Box<dyn CadKernel> {
    Box::new(newtopo_kernel::occt::OcctKernel::new())
}

#[cfg(not(feature = "occt"))]
fn kernel() -> Box<dyn CadKernel> {
    unreachable!("checked in run")
}

pub fn run(a: &InfoArgs) -> Result<()> {
    if !cfg!(feature = "occt") {
        anyhow::bail!("newtopo-cli was built without the `occt` feature");
    }
    let opts = ImportOptions {
        heal: !a.no_heal,
        sewing_tolerance: a.sewing_tol,
        units: a.units.clone(),
        timeout: Duration::from_secs(a.timeout),
        helper: a.helper.clone(),
    };
    let quiet = a.json;
    let imported = run_import(&a.model, &opts, &mut |f, m| {
        if !quiet && !m.is_empty() {
            log::info!("{:3.0}% {m}", f * 100.0);
        }
        true
    })?;
    let name = a.model.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let k = kernel();
    let r = report(name, a.units.clone(), &imported, k.as_ref(), a.sharp_angle)?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        print!("{}", render_text(&r));
    }
    Ok(())
}
