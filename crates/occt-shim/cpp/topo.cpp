// Topology, surface info, evaluation, classification, projection, meshing.
#include "newtopo-occt-shim/cpp/shim.h"
#include "newtopo-occt-shim/src/lib.rs.h"

#include <BRepAdaptor_Curve.hxx>
#include <BRepAdaptor_Surface.hxx>
#include <BRepGProp.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepTools.hxx>
#include <BRepTools_WireExplorer.hxx>
#include <BRep_Tool.hxx>
#include <GCPnts_AbscissaPoint.hxx>
#include <GProp_GProps.hxx>
#include <Geom2d_Curve.hxx>
#include <GeomAPI_ProjectPointOnCurve.hxx>
#include <GeomAPI_ProjectPointOnSurf.hxx>
#include <Geom_Curve.hxx>
#include <Geom_Surface.hxx>
#include <Poly_Triangulation.hxx>
#include <ShapeAnalysis.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Vertex.hxx>
#include <TopoDS_Wire.hxx>
#include <gp_Pnt2d.hxx>

#include <cmath>
#include <limits>

namespace ntshim {

namespace {

const double kNaN = std::numeric_limits<double>::quiet_NaN();

TopoDS_Edge edge_fwd(const Body &b, uint32_t e) { return TopoDS::Edge(b.edge(e).Oriented(TopAbs_FORWARD)); }

bool face_has_edge(const Body &b, uint32_t e, uint32_t f) {
  for (uint32_t g : b.edge_faces[e]) {
    if (g == f) {
      return true;
    }
  }
  return false;
}

// Oriented unit normal; at singular points (poles, apexes) the limit of
// normals sampled just off the point towards the middle of the face's UV box.
gp_Vec oriented_normal(const BRepAdaptor_Surface &S, double u, double v, bool reversed, gp_Pnt &p, gp_Vec &du,
                       gp_Vec &dv) {
  S.D1(u, v, p, du, dv);
  gp_Vec n = du.Crossed(dv);
  if (n.Magnitude() <= 1e-12 * std::max(1.0, du.Magnitude() * dv.Magnitude())) {
    double uc = 0.5 * (S.FirstUParameter() + S.LastUParameter());
    double vc = 0.5 * (S.FirstVParameter() + S.LastVParameter());
    for (double k : {1e-7, 1e-5, 1e-3, 1e-2}) {
      gp_Pnt q;
      gp_Vec a, c;
      S.D1(u + k * (uc - u), v + k * (vc - v), q, a, c);
      gp_Vec m = a.Crossed(c);
      if (m.Magnitude() > 1e-14) {
        n = m;
        break;
      }
    }
  }
  if (n.Magnitude() > 0.0) {
    n.Normalize();
  }
  return reversed ? -n : n;
}

uint8_t surface_kind(GeomAbs_SurfaceType t) {
  switch (t) {
  case GeomAbs_Plane: return 0;
  case GeomAbs_Cylinder: return 1;
  case GeomAbs_Cone: return 2;
  case GeomAbs_Sphere: return 3;
  case GeomAbs_Torus: return 4;
  case GeomAbs_SurfaceOfRevolution: return 5;
  case GeomAbs_SurfaceOfExtrusion: return 6;
  case GeomAbs_BSplineSurface:
  case GeomAbs_BezierSurface: return 7;
  default: return 8;
  }
}

uint8_t curve_kind(GeomAbs_CurveType t) {
  switch (t) {
  case GeomAbs_Line: return 0;
  case GeomAbs_Circle: return 1;
  case GeomAbs_Ellipse: return 2;
  case GeomAbs_BSplineCurve:
  case GeomAbs_BezierCurve: return 3;
  default: return 4;
  }
}

void set3(std::array<double, 3> &a, const gp_XYZ &x) { a = {x.X(), x.Y(), x.Z()}; }

SurfaceInfoFfi surface_info(const Body &b, uint32_t fi) {
  TopoDS_Face F = b.face_fwd(fi);
  BRepAdaptor_Surface S(F, false);
  SurfaceInfoFfi si;
  si.kind = surface_kind(S.GetType());
  BRepTools::UVBounds(F, si.u0, si.u1, si.v0, si.v1);
  si.u_period = S.IsUPeriodic() ? S.UPeriod() : 0.0;
  si.v_period = S.IsVPeriodic() ? S.VPeriod() : 0.0;
  si.u_closed = false;
  si.v_closed = false;
  si.has_origin = false;
  si.has_axis = false;
  si.axis_origin = {0, 0, 0};
  si.axis_dir = {0, 0, 0};
  // Closed directions: a seam edge's two pcurves differ by a shift in u or v.
  for (TopExp_Explorer ex(F, TopAbs_EDGE); ex.More(); ex.Next()) {
    const TopoDS_Edge &E = TopoDS::Edge(ex.Current());
    if (!BRep_Tool::IsClosed(E, F) || BRep_Tool::Degenerated(E)) {
      continue;
    }
    double f, l;
    Handle(Geom2d_Curve) c1 = BRep_Tool::CurveOnSurface(TopoDS::Edge(E.Oriented(TopAbs_FORWARD)), F, f, l);
    Handle(Geom2d_Curve) c2 = BRep_Tool::CurveOnSurface(TopoDS::Edge(E.Oriented(TopAbs_REVERSED)), F, f, l);
    if (c1.IsNull() || c2.IsNull()) {
      continue;
    }
    gp_Vec2d d(c1->Value(0.5 * (f + l)), c2->Value(0.5 * (f + l)));
    if (std::abs(d.X()) > std::abs(d.Y())) {
      si.u_closed = true;
    } else {
      si.v_closed = true;
    }
  }
  auto axis = [&](const gp_Ax1 &ax, bool origin) {
    si.has_axis = true;
    set3(si.axis_dir, ax.Direction().XYZ());
    if (origin) {
      si.has_origin = true;
      set3(si.axis_origin, ax.Location().XYZ());
    }
  };
  switch (S.GetType()) {
  case GeomAbs_Plane: axis(S.Plane().Axis(), true); break;
  case GeomAbs_Cylinder:
    axis(S.Cylinder().Axis(), true);
    si.params.push_back(S.Cylinder().Radius());
    break;
  case GeomAbs_Cone:
    axis(S.Cone().Axis(), true);
    si.params.push_back(S.Cone().RefRadius());
    si.params.push_back(S.Cone().SemiAngle());
    break;
  case GeomAbs_Sphere:
    axis(S.Sphere().Position().Axis(), true);
    si.params.push_back(S.Sphere().Radius());
    break;
  case GeomAbs_Torus:
    axis(S.Torus().Axis(), true);
    si.params.push_back(S.Torus().MajorRadius());
    si.params.push_back(S.Torus().MinorRadius());
    break;
  case GeomAbs_SurfaceOfRevolution: axis(S.AxeOfRevolution(), true); break;
  case GeomAbs_SurfaceOfExtrusion:
    si.has_axis = true;
    set3(si.axis_dir, S.Direction().XYZ());
    break;
  default: break;
  }
  return si;
}

WireFfi wire_info(const Body &b, const TopoDS_Wire &W, const TopoDS_Face &F, bool outer) {
  WireFfi w;
  w.outer = outer;
  int total = 0;
  for (TopExp_Explorer ex(W, TopAbs_EDGE); ex.More(); ex.Next()) {
    ++total;
  }
  int n = 0;
  for (BRepTools_WireExplorer we(W, F); we.More(); we.Next()) {
    ++n;
  }
  auto push = [&](const TopoDS_Shape &e) {
    int idx = b.edges.FindIndex(e);
    if (idx == 0) {
      return;
    }
    w.edges.push_back(static_cast<uint32_t>(idx - 1));
    w.reversed.push_back(e.Orientation() == TopAbs_REVERSED);
  };
  if (n == total) {
    for (BRepTools_WireExplorer we(W, F); we.More(); we.Next()) {
      push(we.Current());
    }
  } else {
    // Wire explorer could not chain every edge (unusual wires): stored order.
    for (TopExp_Explorer ex(W, TopAbs_EDGE); ex.More(); ex.Next()) {
      push(ex.Current());
    }
  }
  return w;
}

// Mean angle between the two faces' oriented normals along the edge.
double dihedral(const Body &b, uint32_t e, uint32_t f0, uint32_t f1) {
  TopoDS_Edge E = edge_fwd(b, e);
  double t0, t1;
  BRep_Tool::Range(E, t0, t1);
  TopoDS_Face F[2] = {b.face(f0), b.face(f1)};
  double sum = 0.0;
  int n = 0;
  for (int i = 0; i < 5; ++i) {
    double t = t0 + (t1 - t0) * (0.1 + 0.2 * i);
    gp_Vec nn[2];
    bool ok = true;
    for (int k = 0; k < 2; ++k) {
      TopoDS_Face Ff = TopoDS::Face(F[k].Oriented(TopAbs_FORWARD));
      double f, l;
      Handle(Geom2d_Curve) c = BRep_Tool::CurveOnSurface(E, Ff, f, l);
      if (c.IsNull()) {
        ok = false;
        break;
      }
      gp_Pnt2d uv = c->Value(t);
      BRepAdaptor_Surface S(Ff, true);
      gp_Pnt p;
      gp_Vec du, dv;
      nn[k] = oriented_normal(S, uv.X(), uv.Y(), F[k].Orientation() == TopAbs_REVERSED, p, du, dv);
      ok = ok && nn[k].Magnitude() > 0.5;
    }
    if (ok) {
      sum += nn[0].Angle(nn[1]);
      ++n;
    }
  }
  return n > 0 ? sum / n : kNaN;
}

} // namespace

TopologyFfi topology(Store &s, uint32_t body) {
  Body &b = s.body(body);
  TopologyFfi t;
  for (int vi = 1; vi <= b.vertices.Extent(); ++vi) {
    gp_Pnt p = BRep_Tool::Pnt(TopoDS::Vertex(b.vertices(vi)));
    t.vertices.push_back(p.X());
    t.vertices.push_back(p.Y());
    t.vertices.push_back(p.Z());
  }
  for (uint32_t fi = 0; fi < static_cast<uint32_t>(b.faces.Extent()); ++fi) {
    FaceFfi f;
    f.surface = surface_info(b, fi);
    f.reversed = b.face(fi).Orientation() == TopAbs_REVERSED;
    GProp_GProps props;
    BRepGProp::SurfaceProperties(b.face(fi), props);
    f.area = props.Mass();
    // Wires of the FORWARD face: outer wire counter-clockwise in UV.
    TopoDS_Face F = b.face_fwd(fi);
    TopoDS_Wire outer = ShapeAnalysis::OuterWire(F);
    std::vector<WireFfi> rest;
    for (TopExp_Explorer ex(F, TopAbs_WIRE); ex.More(); ex.Next()) {
      const TopoDS_Wire &W = TopoDS::Wire(ex.Current());
      bool is_outer = !outer.IsNull() && W.IsSame(outer);
      WireFfi w = wire_info(b, W, F, is_outer);
      if (is_outer) {
        f.wires.push_back(std::move(w));
      } else {
        rest.push_back(std::move(w));
      }
    }
    for (WireFfi &w : rest) {
      f.wires.push_back(std::move(w));
    }
    t.faces.push_back(std::move(f));
  }
  for (uint32_t ei = 0; ei < static_cast<uint32_t>(b.edges.Extent()); ++ei) {
    TopoDS_Edge E = edge_fwd(b, ei);
    EdgeFfi e;
    TopoDS_Vertex v0, v1;
    TopExp::Vertices(E, v0, v1);
    e.v0 = v0.IsNull() ? -1 : b.vertices.FindIndex(v0) - 1;
    e.v1 = v1.IsNull() ? -1 : b.vertices.FindIndex(v1) - 1;
    BRep_Tool::Range(E, e.t0, e.t1);
    for (uint32_t f : b.edge_faces[ei]) {
      e.faces.push_back(f);
    }
    e.degenerate = BRep_Tool::Degenerated(E);
    e.seam = false;
    for (uint32_t f : b.edge_faces[ei]) {
      e.seam = e.seam || (!e.degenerate && BRep_Tool::IsClosed(E, b.face(f)));
    }
    e.closed = !e.degenerate && !v0.IsNull() && v0.IsSame(v1);
    if (e.degenerate) {
      e.curve = 4;
      e.length = 0.0;
    } else {
      BRepAdaptor_Curve C(E);
      e.curve = curve_kind(C.GetType());
      e.length = GCPnts_AbscissaPoint::Length(C);
    }
    e.dihedral = kNaN;
    if (!e.degenerate && !e.seam && b.edge_faces[ei].size() == 2) {
      e.dihedral = dihedral(b, ei, b.edge_faces[ei][0], b.edge_faces[ei][1]);
    }
    t.edges.push_back(std::move(e));
  }
  return t;
}

rust::Vec<double> face_eval(Store &s, uint32_t body, uint32_t face, rust::Slice<const double> uv) {
  Body &b = s.body(body);
  TopoDS_Face F = b.face_fwd(face);
  bool rev = b.face(face).Orientation() == TopAbs_REVERSED;
  BRepAdaptor_Surface S(F, true);
  rust::Vec<double> out;
  out.reserve(uv.size() / 2 * 12);
  for (size_t i = 0; i + 1 < uv.size(); i += 2) {
    gp_Pnt p;
    gp_Vec du, dv;
    gp_Vec n = oriented_normal(S, uv[i], uv[i + 1], rev, p, du, dv);
    for (double x : {p.X(), p.Y(), p.Z(), n.X(), n.Y(), n.Z(), du.X(), du.Y(), du.Z(), dv.X(), dv.Y(), dv.Z()}) {
      out.push_back(x);
    }
  }
  return out;
}

rust::Vec<uint8_t> face_classify_uv(Store &s, uint32_t body, uint32_t face, rust::Slice<const double> uv) {
  Body &b = s.body(body);
  BRepTopAdaptor_FClass2d &cls = b.classifier(face);
  rust::Vec<uint8_t> out;
  out.reserve(uv.size() / 2);
  for (size_t i = 0; i + 1 < uv.size(); i += 2) {
    TopAbs_State st = cls.Perform(gp_Pnt2d(uv[i], uv[i + 1]));
    out.push_back(st == TopAbs_IN ? 0 : st == TopAbs_ON ? 1 : 2);
  }
  return out;
}

rust::Vec<double> edge_eval(Store &s, uint32_t body, uint32_t edge, rust::Slice<const double> t) {
  Body &b = s.body(body);
  TopoDS_Edge E = edge_fwd(b, edge);
  rust::Vec<double> out;
  out.reserve(t.size() * 3);
  if (BRep_Tool::Degenerated(E)) {
    TopoDS_Vertex v0, v1;
    TopExp::Vertices(E, v0, v1);
    gp_Pnt p = BRep_Tool::Pnt(v0);
    for (size_t i = 0; i < t.size(); ++i) {
      out.push_back(p.X());
      out.push_back(p.Y());
      out.push_back(p.Z());
    }
    return out;
  }
  BRepAdaptor_Curve C(E);
  for (double x : t) {
    gp_Pnt p = C.Value(x);
    out.push_back(p.X());
    out.push_back(p.Y());
    out.push_back(p.Z());
  }
  return out;
}

rust::Vec<double> edge_pcurve_eval(Store &s, uint32_t body, uint32_t edge, uint32_t face, bool reversed,
                                   rust::Slice<const double> t) {
  Body &b = s.body(body);
  TopoDS_Face F = b.face_fwd(face);
  if (!face_has_edge(b, edge, face)) {
    fail("edge " + std::to_string(edge) + " does not bound face " + std::to_string(face));
  }
  TopoDS_Edge E = TopoDS::Edge(b.edge(edge).Oriented(reversed ? TopAbs_REVERSED : TopAbs_FORWARD));
  double f, l;
  Handle(Geom2d_Curve) c = BRep_Tool::CurveOnSurface(E, F, f, l);
  if (c.IsNull()) {
    fail("edge " + std::to_string(edge) + " has no pcurve on face " + std::to_string(face));
  }
  rust::Vec<double> out;
  out.reserve(t.size() * 2);
  for (double x : t) {
    gp_Pnt2d p = c->Value(x);
    out.push_back(p.X());
    out.push_back(p.Y());
  }
  return out;
}

rust::Vec<double> project_to_face(Store &s, uint32_t body, uint32_t face, rust::Slice<const double> p) {
  Body &b = s.body(body);
  TopoDS_Face F = b.face_fwd(face);
  BRepTopAdaptor_FClass2d &cls = b.classifier(face);
  Handle(Geom_Surface) surf = BRep_Tool::Surface(F);
  double u0, u1, v0, v1;
  BRepTools::UVBounds(F, u0, u1, v0, v1);
  GeomAPI_ProjectPointOnSurf proj;
  proj.Init(surf, u0, u1, v0, v1);
  // Boundary curves (3D + pcurve) for points whose nearest point lies on the trim.
  struct Bound {
    Handle(Geom_Curve) c3;
    Handle(Geom2d_Curve) c2;
    double f, l;
  };
  std::vector<Bound> bounds;
  for (TopExp_Explorer ex(F, TopAbs_EDGE); ex.More(); ex.Next()) {
    const TopoDS_Edge &E = TopoDS::Edge(ex.Current());
    if (BRep_Tool::Degenerated(E)) {
      continue;
    }
    Bound bd;
    double cf, cl;
    bd.c3 = BRep_Tool::Curve(E, cf, cl);
    bd.c2 = BRep_Tool::CurveOnSurface(E, F, bd.f, bd.l);
    if (!bd.c3.IsNull() && !bd.c2.IsNull()) {
      bd.f = cf;
      bd.l = cl;
      bounds.push_back(bd);
    }
  }
  rust::Vec<double> out;
  out.reserve(p.size() / 3 * 6);
  for (size_t i = 0; i + 2 < p.size(); i += 3) {
    gp_Pnt P(p[i], p[i + 1], p[i + 2]);
    double best = std::numeric_limits<double>::infinity();
    double bu = 0, bv = 0;
    gp_Pnt bp;
    proj.Perform(P);
    if (proj.IsDone()) {
      for (int k = 1; k <= proj.NbPoints(); ++k) {
        double u, v;
        proj.Parameters(k, u, v);
        double d = proj.Distance(k);
        if (d < best && cls.Perform(gp_Pnt2d(u, v)) != TopAbs_OUT) {
          best = d;
          bu = u;
          bv = v;
          bp = proj.Point(k);
        }
      }
    }
    for (const Bound &bd : bounds) {
      GeomAPI_ProjectPointOnCurve pc(P, bd.c3, bd.f, bd.l);
      double cand[3] = {bd.f, bd.l, kNaN};
      if (pc.NbPoints() > 0) {
        cand[2] = pc.LowerDistanceParameter();
      }
      for (double t : cand) {
        if (std::isnan(t)) {
          continue;
        }
        gp_Pnt q = bd.c3->Value(t);
        double d = q.Distance(P);
        if (d < best - 1e-12) {
          best = d;
          gp_Pnt2d uv = bd.c2->Value(t);
          bu = uv.X();
          bv = uv.Y();
          bp = q;
        }
      }
    }
    bool ok = std::isfinite(best);
    for (double x : {ok ? 1.0 : 0.0, bu, bv, bp.X(), bp.Y(), bp.Z()}) {
      out.push_back(x);
    }
  }
  return out;
}

rust::Vec<double> project_to_edge(Store &s, uint32_t body, uint32_t edge, rust::Slice<const double> p) {
  Body &b = s.body(body);
  TopoDS_Edge E = edge_fwd(b, edge);
  double f, l;
  BRep_Tool::Range(E, f, l);
  rust::Vec<double> out;
  out.reserve(p.size() / 3);
  if (BRep_Tool::Degenerated(E)) {
    for (size_t i = 0; i + 2 < p.size(); i += 3) {
      out.push_back(f);
    }
    return out;
  }
  Handle(Geom_Curve) c = BRep_Tool::Curve(E, f, l);
  for (size_t i = 0; i + 2 < p.size(); i += 3) {
    gp_Pnt P(p[i], p[i + 1], p[i + 2]);
    double bt = kNaN;
    double best = std::numeric_limits<double>::infinity();
    double cand[3] = {f, l, kNaN};
    GeomAPI_ProjectPointOnCurve pc(P, c, f, l);
    if (pc.NbPoints() > 0) {
      cand[2] = pc.LowerDistanceParameter();
    }
    for (double t : cand) {
      if (!std::isnan(t)) {
        double d = c->Value(t).Distance(P);
        if (d < best) {
          best = d;
          bt = t;
        }
      }
    }
    out.push_back(bt);
  }
  return out;
}

MeshFfi tessellate(Store &s, uint32_t body, uint32_t face, double tol) {
  Body &b = s.body(body);
  if (!(tol > 0.0)) {
    fail("tessellation tolerance must be positive");
  }
  const TopoDS_Face &Fo = b.face(face);
  if (b.mesh_tol != tol) {
    BRepTools::Clean(b.shape);
    BRepMesh_IncrementalMesh mesh(b.shape, tol, false, 0.5, false);
    b.mesh_tol = tol;
  }
  MeshFfi m;
  TopLoc_Location loc;
  Handle(Poly_Triangulation) tri = BRep_Tool::Triangulation(Fo, loc);
  if (tri.IsNull()) {
    b.mesh_tol = -1.0; // retry meshing on the next call
    fail("meshing produced no triangulation for face " + std::to_string(face));
  }
  bool rev = Fo.Orientation() == TopAbs_REVERSED;
  gp_Trsf tr = loc.Transformation();
  TopoDS_Face F = b.face_fwd(face);
  BRepAdaptor_Surface S(F, true);
  for (int i = 1; i <= tri->NbNodes(); ++i) {
    gp_Pnt p = tri->Node(i).Transformed(tr);
    m.positions.push_back(p.X());
    m.positions.push_back(p.Y());
    m.positions.push_back(p.Z());
    gp_Vec n(0, 0, 0);
    if (tri->HasUVNodes()) {
      gp_Pnt2d uv = tri->UVNode(i);
      m.uvs.push_back(uv.X());
      m.uvs.push_back(uv.Y());
      gp_Pnt q;
      gp_Vec du, dv;
      n = oriented_normal(S, uv.X(), uv.Y(), rev, q, du, dv);
    } else {
      m.uvs.push_back(0.0);
      m.uvs.push_back(0.0);
    }
    m.normals.push_back(n.X());
    m.normals.push_back(n.Y());
    m.normals.push_back(n.Z());
  }
  for (int i = 1; i <= tri->NbTriangles(); ++i) {
    int n1, n2, n3;
    tri->Triangle(i).Get(n1, n2, n3);
    if (rev) {
      std::swap(n2, n3);
    }
    m.triangles.push_back(static_cast<uint32_t>(n1 - 1));
    m.triangles.push_back(static_cast<uint32_t>(n2 - 1));
    m.triangles.push_back(static_cast<uint32_t>(n3 - 1));
  }
  return m;
}

} // namespace ntshim
