// Reading, writing and healing: BREP/BinTools, STEP/IGES through XCAF.
#include "shim.h"
#include "newtopo-occt-shim/src/lib.rs.h"

#include <BRepBuilderAPI_Copy.hxx>
#include <BRepBuilderAPI_Sewing.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepTools.hxx>
#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <BinTools.hxx>
#include <IFSelect_ReturnStatus.hxx>
#include <IGESCAFControl_Reader.hxx>
#include <Message_Msg.hxx>
#include <Quantity_Color.hxx>
#include <STEPCAFControl_Reader.hxx>
#include <ShapeExtend_MsgRegistrator.hxx>
#include <ShapeFix_Shape.hxx>
#include <ShapeFix_Solid.hxx>
#include <Standard_ErrorHandler.hxx>
#include <TCollection_AsciiString.hxx>
#include <TDF_Label.hxx>
#include <TDataStd_Name.hxx>
#include <TDocStd_Document.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <NCollection_IndexedDataMap.hxx>
#include <NCollection_List.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Shell.hxx>
#include <XCAFApp_Application.hxx>
#include <XCAFDoc_ColorTool.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_LayerTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>

#include <sstream>

namespace ntshim {

namespace {

TopoDS_Shape read_brep_stream(const std::string &data) {
  TopoDS_Shape shape;
  size_t start = data.find_first_not_of(" \t\r\n");
  bool binary = start != std::string::npos && data.compare(start, 21, "Open CASCADE Topology") == 0;
  if (binary) {
    std::istringstream in(data, std::ios::in | std::ios::binary);
    BinTools::Read(shape, in);
  } else {
    std::istringstream in(data);
    BRep_Builder builder;
    BRepTools::Read(shape, in, builder);
  }
  if (shape.IsNull()) {
    fail("not a B-rep file (no shape read)");
  }
  return shape;
}

int count_faces(const TopoDS_Shape &s) {
  ShapeMap m;
  TopExp::MapShapes(s, TopAbs_FACE, m);
  return m.Extent();
}

// Edges bounding exactly one face that are neither seams nor degenerate.
int count_free_edges(const TopoDS_Shape &s) {
  NCollection_IndexedDataMap<TopoDS_Shape, NCollection_List<TopoDS_Shape>, TopTools_ShapeMapHasher> m;
  TopExp::MapShapesAndAncestors(s, TopAbs_EDGE, TopAbs_FACE, m);
  int n = 0;
  for (int i = 1; i <= m.Extent(); ++i) {
    const TopoDS_Edge &e = TopoDS::Edge(m.FindKey(i));
    if (BRep_Tool::Degenerated(e)) {
      continue;
    }
    ShapeMap faces;
    for (const TopoDS_Shape &f : m(i)) {
      faces.Add(f);
    }
    if (faces.Extent() == 1 && !BRep_Tool::IsClosed(e, TopoDS::Face(faces(1)))) {
      ++n;
    }
  }
  return n;
}

// Closed shells become solids (oriented outward by ShapeFix_Solid).
TopoDS_Shape shells_to_solids(const TopoDS_Shape &s) {
  BRep_Builder bb;
  TopoDS_Compound out;
  bb.MakeCompound(out);
  int n = 0;
  for (TopExp_Explorer ex(s, TopAbs_SOLID); ex.More(); ex.Next(), ++n) {
    bb.Add(out, ex.Current());
  }
  for (TopExp_Explorer ex(s, TopAbs_SHELL, TopAbs_SOLID); ex.More(); ex.Next(), ++n) {
    const TopoDS_Shell &sh = TopoDS::Shell(ex.Current());
    if (BRep_Tool::IsClosed(sh)) {
      ShapeFix_Solid fix;
      bb.Add(out, fix.SolidFromShell(sh));
    } else {
      bb.Add(out, sh);
    }
  }
  for (TopExp_Explorer ex(s, TopAbs_FACE, TopAbs_SHELL); ex.More(); ex.Next(), ++n) {
    bb.Add(out, ex.Current());
  }
  if (n == 1) {
    TopoDS_Iterator it(out);
    return it.Value();
  }
  return out;
}

std::vector<uint32_t> invalid_faces(const ShapeMap &faces) {
  std::vector<uint32_t> bad;
  for (int i = 1; i <= faces.Extent(); ++i) {
    BRepCheck_Analyzer an(faces(i), true);
    if (!an.IsValid()) {
      bad.push_back(static_cast<uint32_t>(i - 1));
    }
  }
  return bad;
}

std::string label_name(const TDF_Label &l) {
  Handle(TDataStd_Name) n;
  if (!l.IsNull() && l.FindAttribute(TDataStd_Name::GetID(), n)) {
    return to_utf8(n->Get());
  }
  return std::string();
}

struct Attrs {
  std::string name;
  bool has_layer = false;
  std::string layer;
  bool has_color = false;
  Quantity_Color color;
};

class Walker {
public:
  Walker(Store &s, const Handle(TDocStd_Document) & doc, bool merge_loose_faces, ImportResultFfi &out)
      : s_(s), out_(out), merge_(merge_loose_faces) {
    st_ = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
    ct_ = XCAFDoc_DocumentTool::ColorTool(doc->Main());
    lt_ = XCAFDoc_DocumentTool::LayerTool(doc->Main());
    bb_.MakeCompound(loose_);
  }

  void run() {
    NCollection_Sequence<TDF_Label> roots;
    st_->GetFreeShapes(roots);
    for (const TDF_Label &r : roots) {
      Attrs a;
      visit(r, TDF_Label(), TopLoc_Location(), a);
    }
    if (loose_count_ > 0) {
      Attrs a;
      a.name = "Surfaces";
      a.has_layer = loose_layer_ok_ && !loose_layer_.empty();
      a.layer = loose_layer_;
      emit_body(loose_, a, TopLoc_Location());
      out_.warnings.push_back(rust::String(std::to_string(loose_count_) +
                                           " loose surfaces were merged into one body \"Surfaces\""));
    }
  }

private:
  bool find_color(const TDF_Label &l, Quantity_Color &c) {
    if (l.IsNull()) {
      return false;
    }
    return XCAFDoc_ColorTool::GetColor(l, XCAFDoc_ColorSurf, c) || XCAFDoc_ColorTool::GetColor(l, XCAFDoc_ColorGen, c);
  }

  bool find_layer(const TDF_Label &l, std::string &out) {
    if (l.IsNull()) {
      return false;
    }
    Handle(NCollection_HSequence<TCollection_ExtendedString>) ls = lt_->GetLayers(l);
    if (!ls.IsNull() && ls->Length() > 0) {
      out = to_utf8(ls->Value(1));
      return true;
    }
    return false;
  }

  void visit(const TDF_Label &proto, const TDF_Label &inst, const TopLoc_Location &loc, const Attrs &parent) {
    Attrs a;
    a.name = label_name(inst);
    if (a.name.empty()) {
      a.name = label_name(proto);
    }
    a.has_layer = find_layer(inst, a.layer) || find_layer(proto, a.layer);
    if (!a.has_layer && parent.has_layer) {
      a.has_layer = true;
      a.layer = parent.layer;
    }
    a.has_color = find_color(inst, a.color) || find_color(proto, a.color);
    if (!a.has_color) {
      // Colour attached to the body's shape or its first sub-shape label.
      TopoDS_Shape sh = XCAFDoc_ShapeTool::GetShape(proto);
      Quantity_Color c;
      if (!sh.IsNull() && (ct_->GetColor(sh, XCAFDoc_ColorSurf, c) || ct_->GetColor(sh, XCAFDoc_ColorGen, c))) {
        a.has_color = true;
        a.color = c;
      } else {
        NCollection_Sequence<TDF_Label> subs;
        XCAFDoc_ShapeTool::GetSubShapes(proto, subs);
        for (const TDF_Label &sl : subs) {
          if (find_color(sl, c)) {
            a.has_color = true;
            a.color = c;
            break;
          }
        }
      }
    }
    if (!a.has_color && parent.has_color) {
      a.has_color = true;
      a.color = parent.color;
    }

    if (XCAFDoc_ShapeTool::IsAssembly(proto)) {
      path_.push_back(a.name);
      NCollection_Sequence<TDF_Label> comps;
      XCAFDoc_ShapeTool::GetComponents(proto, comps, false);
      for (const TDF_Label &c : comps) {
        TDF_Label ref;
        if (XCAFDoc_ShapeTool::GetReferredShape(c, ref)) {
          visit(ref, c, loc * XCAFDoc_ShapeTool::GetLocation(c), a);
        }
      }
      path_.pop_back();
      return;
    }
    TopoDS_Shape shape = XCAFDoc_ShapeTool::GetShape(proto);
    if (shape.IsNull()) {
      return;
    }
    split_and_emit(shape, a, loc);
  }

  void split_and_emit(const TopoDS_Shape &shape, const Attrs &a, const TopLoc_Location &loc) {
    if (!TopExp_Explorer(shape, TopAbs_FACE).More()) {
      out_.warnings.push_back(rust::String("skipped \"" + a.name + "\": no faces"));
      return;
    }
    std::vector<TopoDS_Shape> parts;
    for (TopExp_Explorer ex(shape, TopAbs_SOLID); ex.More(); ex.Next()) {
      parts.push_back(ex.Current());
    }
    for (TopExp_Explorer ex(shape, TopAbs_SHELL, TopAbs_SOLID); ex.More(); ex.Next()) {
      parts.push_back(ex.Current());
    }
    TopoDS_Compound faces;
    bb_.MakeCompound(faces);
    int nfaces = 0;
    for (TopExp_Explorer ex(shape, TopAbs_FACE, TopAbs_SHELL); ex.More(); ex.Next(), ++nfaces) {
      bb_.Add(faces, ex.Current());
    }
    if (parts.empty() && nfaces == 0) {
      out_.warnings.push_back(rust::String("skipped \"" + a.name + "\": no faces"));
      return;
    }
    if (parts.empty() && merge_) {
      // IGES-style loose trimmed surfaces: merge (in world space) so healing
      // can sew them into one body.
      bb_.Add(loose_, faces.Moved(loc));
      ++loose_count_;
      std::string layer = a.has_layer ? a.layer : std::string();
      if (loose_count_ == 1) {
        loose_layer_ = layer;
      } else if (layer != loose_layer_) {
        loose_layer_ok_ = false;
      }
      return;
    }
    if (nfaces > 0) {
      parts.push_back(faces);
    }
    if (parts.size() == 1) {
      emit_body(shape, a, loc);
      return;
    }
    for (size_t i = 0; i < parts.size(); ++i) {
      Attrs pa = a;
      pa.name = a.name + "." + std::to_string(i + 1);
      emit_body(parts[i], pa, loc);
    }
  }

  void emit_body(const TopoDS_Shape &shape, const Attrs &a, const TopLoc_Location &loc) {
    ImportedBodyFfi b;
    // Instances of one XCAF prototype share a TShape; give every body its own
    // copy so meshing or healing one never touches another.
    bool shared = emitted_.Contains(shape);
    emitted_.Add(shape);
    b.body = s_.add(shared ? BRepBuilderAPI_Copy(shape, true, false).Shape() : shape);
    b.name = rust::String(a.name.empty() ? "Body " + std::to_string(out_.bodies.size() + 1) : a.name);
    b.has_layer = a.has_layer;
    b.layer = rust::String(a.layer);
    b.has_color = a.has_color;
    if (a.has_color) {
      double r, g, bl;
      a.color.Values(r, g, bl, Quantity_TOC_sRGB);
      b.color = {static_cast<float>(r), static_cast<float>(g), static_cast<float>(bl)};
    } else {
      b.color = {0.8f, 0.8f, 0.8f};
    }
    gp_Trsf t = loc.Transformation();
    for (int r = 1; r <= 3; ++r) {
      for (int c = 1; c <= 4; ++c) {
        b.transform[static_cast<size_t>((r - 1) * 4 + (c - 1))] = t.Value(r, c);
      }
    }
    b.transform[12] = 0.0;
    b.transform[13] = 0.0;
    b.transform[14] = 0.0;
    b.transform[15] = 1.0;
    for (const std::string &p : path_) {
      b.path.push_back(rust::String(p));
    }
    out_.bodies.push_back(std::move(b));
  }

  Store &s_;
  ImportResultFfi &out_;
  bool merge_;
  Handle(XCAFDoc_ShapeTool) st_;
  Handle(XCAFDoc_ColorTool) ct_;
  Handle(XCAFDoc_LayerTool) lt_;
  std::vector<std::string> path_;
  BRep_Builder bb_;
  TopoDS_Compound loose_;
  ShapeMap emitted_;
  int loose_count_ = 0;
  std::string loose_layer_;
  bool loose_layer_ok_ = true;
};

struct DocGuard {
  Handle(XCAFApp_Application) app;
  Handle(TDocStd_Document) doc;
  ~DocGuard() {
    if (!doc.IsNull() && doc->IsOpened()) {
      app->Close(doc);
    }
  }
};

} // namespace

uint32_t load_brep(Store &s, rust::Slice<const uint8_t> bytes) {
  std::string data(reinterpret_cast<const char *>(bytes.data()), bytes.size());
  return s.add(read_brep_stream(data));
}

rust::Vec<uint8_t> save_brep(Store &s, uint32_t body) {
  Body &b = s.body(body);
  std::ostringstream out(std::ios::out | std::ios::binary);
  BinTools::Write(b.shape, out, false, false, BinTools_FormatVersion_CURRENT);
  std::string data = out.str();
  rust::Vec<uint8_t> v;
  v.reserve(data.size());
  for (char c : data) {
    v.push_back(static_cast<uint8_t>(c));
  }
  return v;
}

ImportResultFfi import_cad(Store &s, rust::Str path, uint8_t format, double unit_m) {
  OCC_CATCH_SIGNALS
  std::string p(path);
  ImportResultFfi out;
  DocGuard g;
  g.app = XCAFApp_Application::GetApplication();
  g.app->NewDocument("MDTV-XCAF", g.doc);
  if (unit_m > 0.0) {
    XCAFDoc_DocumentTool::SetLengthUnit(g.doc, unit_m);
  }
  if (format == 0) {
    STEPCAFControl_Reader r;
    r.SetColorMode(true);
    r.SetNameMode(true);
    r.SetLayerMode(true);
    if (r.ReadFile(p.c_str()) != IFSelect_RetDone) {
      fail("STEP reader could not parse " + p);
    }
    if (!r.Transfer(g.doc)) {
      fail("STEP transfer failed");
    }
  } else if (format == 1) {
    IGESCAFControl_Reader r;
    r.SetColorMode(true);
    r.SetNameMode(true);
    r.SetLayerMode(true);
    if (r.ReadFile(p.c_str()) != IFSelect_RetDone) {
      fail("IGES reader could not parse " + p);
    }
    if (!r.Transfer(g.doc)) {
      fail("IGES transfer failed");
    }
  } else {
    fail("unknown format");
  }
  Walker w(s, g.doc, format == 1, out);
  w.run();
  if (out.bodies.empty()) {
    fail("the file contains no surfaces or solids");
  }
  return out;
}

HealResult heal(Store &s, uint32_t body, const HealOptions &o) {
  OCC_CATCH_SIGNALS
  Body &b = s.body(body);
  HealResult r;
  TopoDS_Shape shape = b.shape;
  r.faces_before = static_cast<size_t>(count_faces(shape));
  size_t invalid_before = invalid_faces(b.faces).size();

  if (o.fix) {
    Handle(ShapeFix_Shape) fx = new ShapeFix_Shape(shape);
    Handle(ShapeExtend_MsgRegistrator) reg = new ShapeExtend_MsgRegistrator;
    fx->SetMsgRegistrator(reg);
    fx->SetMaxTolerance(std::max(o.sewing_tolerance, 1e-4));
    fx->Perform();
    TopoDS_Shape fixed = fx->Shape();
    std::map<std::string, int> msgs;
    using MsgMap = std::decay_t<decltype(reg->MapShape())>;
    for (MsgMap::Iterator it(reg->MapShape()); it.More(); it.Next()) {
      for (NCollection_List<Message_Msg>::Iterator mi(it.Value()); mi.More(); mi.Next()) {
        msgs[to_utf8(mi.Value().Original())] += 1;
      }
    }
    // Faces ShapeFix replaced; flags alone (e.g. a shell re-flagged closed)
    // are not worth reporting when no face changed and nothing was logged.
    ShapeMap after;
    TopExp::MapShapes(fixed, TopAbs_FACE, after);
    int rebuilt = 0;
    for (int i = 1; i <= after.Extent(); ++i) {
      rebuilt += b.faces.Contains(after(i)) ? 0 : 1;
    }
    if (rebuilt > 0 || !msgs.empty()) {
      static const struct {
        ShapeExtend_Status st;
        const char *msg;
      } kinds[] = {{ShapeExtend_DONE1, "fixed free edges"}, {ShapeExtend_DONE2, "fixed free wires"},
                   {ShapeExtend_DONE3, "fixed faces"},      {ShapeExtend_DONE4, "fixed shells"},
                   {ShapeExtend_DONE5, "fixed solids"},     {ShapeExtend_DONE6, "fixed shapes in compounds"}};
      for (const auto &k : kinds) {
        if (fx->Status(k.st)) {
          r.fixes.push_back(rust::String(std::string("shape fix: ") + k.msg));
        }
      }
    }
    if (rebuilt > 0) {
      r.fixes.push_back(rust::String("shape fix: " + std::to_string(rebuilt) + " faces rebuilt"));
    }
    for (const auto &kv : msgs) {
      r.fixes.push_back(rust::String("shape fix: " + kv.first + " (x" + std::to_string(kv.second) + ")"));
    }
    shape = fixed;
  }

  if (o.sew) {
    int free_before = count_free_edges(shape);
    if (free_before > 0) {
      BRepBuilderAPI_Sewing sew(o.sewing_tolerance);
      sew.Add(shape);
      sew.Perform();
      TopoDS_Shape sewn = sew.SewedShape();
      if (!sewn.IsNull()) {
        sewn = shells_to_solids(sewn);
        int free_after = count_free_edges(sewn);
        std::ostringstream m;
        m << "sewing (tolerance " << o.sewing_tolerance << "): free edges " << free_before << " -> " << free_after;
        if (sew.NbMultipleEdges() > 0) {
          m << ", " << sew.NbMultipleEdges() << " non-manifold edges";
        }
        if (sew.NbDegeneratedShapes() > 0) {
          m << ", " << sew.NbDegeneratedShapes() << " degenerate shapes removed";
        }
        if (sew.NbDeletedFaces() > 0) {
          m << ", " << sew.NbDeletedFaces() << " faces deleted";
        }
        r.fixes.push_back(rust::String(m.str()));
        shape = sewn;
      }
    }
  }

  if (!shape.IsSame(b.shape) || shape.Orientation() != b.shape.Orientation()) {
    b.shape = shape;
    b.reindex();
  }
  std::vector<uint32_t> bad = invalid_faces(b.faces);
  if (invalid_before > 0 || !bad.empty()) {
    r.fixes.push_back(rust::String("invalid faces: " + std::to_string(invalid_before) + " before healing, " +
                                   std::to_string(bad.size()) + " after"));
  }
  for (uint32_t f : bad) {
    r.failed_faces.push_back(f);
  }
  r.faces_after = static_cast<size_t>(b.faces.Extent());
  return r;
}

} // namespace ntshim
