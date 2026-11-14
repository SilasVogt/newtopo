// Test-shape builder: primitives, fillets, chamfers, booleans, revolve,
// prisms and an XCAF STEP writer. Only used to make the SPEC §11 models.
#include "testshapes.h"
#include "newtopo-occt-shim/src/testshapes.rs.h"

#include <BRepAlgoAPI_Common.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakePolygon.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepBuilderAPI_Transform.hxx>
#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCone.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <BRepPrimAPI_MakeRevol.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepPrimAPI_MakeTorus.hxx>
#include <BRep_Tool.hxx>
#include <GeomAPI_Interpolate.hxx>
#include <Geom_BSplineCurve.hxx>
#include <IFSelect_ReturnStatus.hxx>
#include <Quantity_Color.hxx>
#include <STEPCAFControl_Writer.hxx>
#include <TDataStd_Name.hxx>
#include <TDocStd_Document.hxx>
#include <TopoDS.hxx>
#include <XCAFApp_Application.hxx>
#include <XCAFDoc_ColorTool.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_LayerTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>
#include <gp_Ax2.hxx>
#include <gp_Pln.hxx>

namespace ntshim {

namespace {

const TopoDS_Shape &shape_of(Store &s, uint32_t id) { return s.body(id).shape; }

std::vector<TopoDS_Edge> pick_edges(Store &s, uint32_t body, rust::Slice<const uint32_t> ids) {
  Body &b = s.body(body);
  std::vector<TopoDS_Edge> out;
  if (ids.empty()) {
    for (uint32_t e = 0; e < static_cast<uint32_t>(b.edges.Extent()); ++e) {
      const TopoDS_Edge &E = TopoDS::Edge(b.edge(e));
      bool seam = false;
      for (uint32_t f : b.edge_faces[e]) {
        seam = seam || BRep_Tool::IsClosed(E, b.face(f));
      }
      if (!seam && !BRep_Tool::Degenerated(E) && b.edge_faces[e].size() == 2) {
        out.push_back(E);
      }
    }
  } else {
    for (uint32_t e : ids) {
      out.push_back(TopoDS::Edge(b.edge(e)));
    }
  }
  return out;
}

uint32_t done(Store &s, BRepBuilderAPI_MakeShape &mk, const char *what) {
  if (!mk.IsDone()) {
    mk.Build();
  }
  if (!mk.IsDone()) {
    fail(std::string(what) + " failed");
  }
  return s.add(mk.Shape());
}

gp_Trsf trsf_from(const double *m) {
  gp_Trsf t;
  t.SetValues(m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8], m[9], m[10], m[11]);
  return t;
}

} // namespace

uint32_t ts_box(Store &s, double x, double y, double z, double dx, double dy, double dz) {
  BRepPrimAPI_MakeBox mk(gp_Pnt(x, y, z), dx, dy, dz);
  return done(s, mk, "box");
}

uint32_t ts_cylinder(Store &s, double r, double h) {
  BRepPrimAPI_MakeCylinder mk(r, h);
  return done(s, mk, "cylinder");
}

uint32_t ts_cone(Store &s, double r1, double r2, double h) {
  BRepPrimAPI_MakeCone mk(r1, r2, h);
  return done(s, mk, "cone");
}

uint32_t ts_sphere(Store &s, double r) {
  BRepPrimAPI_MakeSphere mk(r);
  return done(s, mk, "sphere");
}

uint32_t ts_torus(Store &s, double major, double minor) {
  BRepPrimAPI_MakeTorus mk(major, minor);
  return done(s, mk, "torus");
}

uint32_t ts_transform(Store &s, uint32_t body, rust::Slice<const double> m) {
  if (m.size() != 12) {
    fail("transform needs 12 values");
  }
  BRepBuilderAPI_Transform mk(shape_of(s, body), trsf_from(m.data()), true);
  return done(s, mk, "transform");
}

uint32_t ts_fillet(Store &s, uint32_t body, rust::Slice<const uint32_t> edges, double r) {
  BRepFilletAPI_MakeFillet mk(shape_of(s, body));
  for (const TopoDS_Edge &e : pick_edges(s, body, edges)) {
    mk.Add(r, e);
  }
  mk.Build();
  return done(s, mk, "fillet");
}

uint32_t ts_chamfer(Store &s, uint32_t body, rust::Slice<const uint32_t> edges, double d) {
  BRepFilletAPI_MakeChamfer mk(shape_of(s, body));
  for (const TopoDS_Edge &e : pick_edges(s, body, edges)) {
    mk.Add(d, e);
  }
  mk.Build();
  return done(s, mk, "chamfer");
}

uint32_t ts_boolean(Store &s, uint8_t op, uint32_t a, rust::Slice<const uint32_t> tools) {
  NCollection_List<TopoDS_Shape> args, tl;
  args.Append(shape_of(s, a));
  for (uint32_t t : tools) {
    tl.Append(shape_of(s, t));
  }
  std::unique_ptr<BRepAlgoAPI_BooleanOperation> mk;
  if (op == 0) {
    mk.reset(new BRepAlgoAPI_Fuse());
  } else if (op == 1) {
    mk.reset(new BRepAlgoAPI_Cut());
  } else {
    mk.reset(new BRepAlgoAPI_Common());
  }
  mk->SetArguments(args);
  mk->SetTools(tl);
  mk->Build();
  if (mk->HasErrors()) {
    fail("boolean failed");
  }
  mk->SimplifyResult();
  return done(s, *mk, "boolean");
}

uint32_t ts_revolve(Store &s, rust::Slice<const double> profile, int32_t spline_from, int32_t spline_to) {
  int n = static_cast<int>(profile.size() / 2);
  if (n < 3) {
    fail("profile needs at least 3 points");
  }
  auto pt = [&](int i) { return gp_Pnt(profile[2 * i], 0.0, profile[2 * i + 1]); };
  BRepBuilderAPI_MakeWire wire;
  int i = 0;
  while (i < n) {
    int j = (i + 1) % n;
    if (i == spline_from && spline_to > spline_from && spline_to < n) {
      Handle(NCollection_HArray1<gp_Pnt>) pts = new NCollection_HArray1<gp_Pnt>(1, spline_to - spline_from + 1);
      for (int k = spline_from; k <= spline_to; ++k) {
        pts->SetValue(k - spline_from + 1, pt(k));
      }
      GeomAPI_Interpolate interp(pts, false, 1e-7);
      interp.Perform();
      if (!interp.IsDone()) {
        fail("profile interpolation failed");
      }
      wire.Add(BRepBuilderAPI_MakeEdge(interp.Curve()).Edge());
      i = spline_to;
      if (i == n - 1) {
        wire.Add(BRepBuilderAPI_MakeEdge(pt(n - 1), pt(0)).Edge());
        break;
      }
      continue;
    }
    wire.Add(BRepBuilderAPI_MakeEdge(pt(i), pt(j)).Edge());
    ++i;
  }
  if (!wire.IsDone()) {
    fail("profile wire failed");
  }
  BRepBuilderAPI_MakeFace face(gp_Pln(gp_Ax3(gp::Origin(), gp::DY(), gp::DX())), wire.Wire(), true);
  if (!face.IsDone()) {
    fail("profile face failed");
  }
  BRepPrimAPI_MakeRevol mk(face.Face(), gp_Ax1(gp::Origin(), gp::DZ()));
  return done(s, mk, "revolve");
}

uint32_t ts_prism(Store &s, rust::Slice<const double> polygon, double height) {
  BRepBuilderAPI_MakePolygon poly;
  for (size_t i = 0; i + 1 < polygon.size(); i += 2) {
    poly.Add(gp_Pnt(polygon[i], polygon[i + 1], 0.0));
  }
  poly.Close();
  BRepBuilderAPI_MakeFace face(poly.Wire(), true);
  BRepPrimAPI_MakePrism mk(face.Face(), gp_Vec(0, 0, height));
  return done(s, mk, "prism");
}

void ts_write_step(Store &s, rust::Str path, rust::Slice<const XcafNode> nodes) {
  Handle(XCAFApp_Application) app = XCAFApp_Application::GetApplication();
  Handle(TDocStd_Document) doc;
  app->NewDocument("MDTV-XCAF", doc);
  XCAFDoc_DocumentTool::SetLengthUnit(doc, 0.001);
  Handle(XCAFDoc_ShapeTool) st = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
  Handle(XCAFDoc_ColorTool) ct = XCAFDoc_DocumentTool::ColorTool(doc->Main());
  Handle(XCAFDoc_LayerTool) lt = XCAFDoc_DocumentTool::LayerTool(doc->Main());
  std::vector<TDF_Label> labels(nodes.size());
  std::map<int32_t, TDF_Label> protos;
  auto style = [&](const TDF_Label &l, const XcafNode &n) {
    if (n.has_color) {
      Quantity_Color c(n.color[0], n.color[1], n.color[2], Quantity_TOC_sRGB);
      ct->SetColor(l, c, XCAFDoc_ColorGen);
    }
    if (!n.layer.empty()) {
      lt->SetLayer(l, TCollection_ExtendedString(std::string(n.layer).c_str(), true));
    }
  };
  for (size_t i = 0; i < nodes.size(); ++i) {
    const XcafNode &n = nodes[i];
    std::string name(n.name);
    TopLoc_Location loc(trsf_from(n.transform.data()));
    TDF_Label proto;
    if (n.body >= 0) {
      auto it = protos.find(n.body);
      if (it == protos.end()) {
        TopoDS_Shape sh = shape_of(s, static_cast<uint32_t>(n.body));
        if (n.parent < 0) {
          sh = sh.Moved(loc);
        }
        proto = st->AddShape(sh, false);
        TDataStd_Name::Set(proto, TCollection_ExtendedString(name.c_str(), true));
        style(proto, n);
        protos.emplace(n.body, proto);
      } else {
        proto = it->second;
      }
    } else {
      proto = st->NewShape();
      TDataStd_Name::Set(proto, TCollection_ExtendedString(name.c_str(), true));
    }
    labels[i] = proto;
    if (n.parent >= 0) {
      if (static_cast<size_t>(n.parent) >= i) {
        fail("XCAF node parents must come first");
      }
      TDF_Label comp = st->AddComponent(labels[static_cast<size_t>(n.parent)], proto, loc);
      TDataStd_Name::Set(comp, TCollection_ExtendedString(name.c_str(), true));
    }
  }
  st->UpdateAssemblies();
  STEPCAFControl_Writer w;
  w.SetColorMode(true);
  w.SetNameMode(true);
  w.SetLayerMode(true);
  bool ok = w.Transfer(doc, STEPControl_AsIs) && w.Write(std::string(path).c_str()) == IFSelect_RetDone;
  app->Close(doc);
  if (!ok) {
    fail("STEP write failed");
  }
}

} // namespace ntshim
