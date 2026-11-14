// Store, body index and shared helpers.
#include "shim.h"
#include "newtopo-occt-shim/src/lib.rs.h"

#include <BRep_Tool.hxx>
#include <Message.hxx>
#include <Message_Messenger.hxx>
#include <OSD.hxx>
#include <Standard_Version.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <NCollection_IndexedDataMap.hxx>
#include <NCollection_List.hxx>
#include <TopoDS.hxx>

#include <stdexcept>

namespace ntshim {

void fail(const std::string &msg) { throw std::runtime_error(msg); }

std::string to_utf8(const TCollection_ExtendedString &s) {
  if (s.Length() == 0) {
    return std::string();
  }
  std::string out(static_cast<size_t>(s.LengthOfCString()) + 1, '\0');
  char *buf = &out[0];
  int n = s.ToUTF8CString(buf);
  out.resize(static_cast<size_t>(n > 0 ? n : 0));
  // ToUTF8CString may report a length including the terminator.
  while (!out.empty() && out.back() == '\0') {
    out.pop_back();
  }
  return out;
}

Body::Body(const TopoDS_Shape &s) : shape(s) { reindex(); }

void Body::reindex() {
  faces.Clear();
  edges.Clear();
  vertices.Clear();
  classifiers.clear();
  mesh_tol = -1.0;
  // Deterministic traversal order: ids survive reload of the same file.
  TopExp::MapShapes(shape, TopAbs_FACE, faces);
  TopExp::MapShapes(shape, TopAbs_EDGE, edges);
  TopExp::MapShapes(shape, TopAbs_VERTEX, vertices);
  edge_faces.assign(static_cast<size_t>(edges.Extent()), {});
  for (int fi = 1; fi <= faces.Extent(); ++fi) {
    for (TopExp_Explorer ex(faces(fi), TopAbs_EDGE); ex.More(); ex.Next()) {
      int ei = edges.FindIndex(ex.Current());
      if (ei == 0) {
        continue;
      }
      auto &list = edge_faces[static_cast<size_t>(ei - 1)];
      uint32_t f = static_cast<uint32_t>(fi - 1);
      bool seen = false;
      for (uint32_t g : list) {
        seen = seen || g == f;
      }
      if (!seen) {
        list.push_back(f);
      }
    }
  }
}

const TopoDS_Face &Body::face(uint32_t id) const {
  if (id >= static_cast<uint32_t>(faces.Extent())) {
    fail("unknown face " + std::to_string(id));
  }
  return TopoDS::Face(faces(static_cast<int>(id) + 1));
}

TopoDS_Face Body::face_fwd(uint32_t id) const { return TopoDS::Face(face(id).Oriented(TopAbs_FORWARD)); }

const TopoDS_Shape &Body::edge(uint32_t id) const {
  if (id >= static_cast<uint32_t>(edges.Extent())) {
    fail("unknown edge " + std::to_string(id));
  }
  return edges(static_cast<int>(id) + 1);
}

BRepTopAdaptor_FClass2d &Body::classifier(uint32_t f) {
  auto it = classifiers.find(f);
  if (it == classifiers.end()) {
    TopoDS_Face F = face_fwd(f);
    double tol = BRep_Tool::Tolerance(F);
    it = classifiers.emplace(f, std::make_unique<BRepTopAdaptor_FClass2d>(F, tol)).first;
  }
  return *it->second;
}

Body &Store::body(uint32_t id) {
  auto it = bodies.find(id);
  if (it == bodies.end()) {
    fail("unknown body " + std::to_string(id));
  }
  return *it->second;
}

uint32_t Store::add(const TopoDS_Shape &s) {
  if (s.IsNull()) {
    fail("empty shape");
  }
  uint32_t id = next_id++;
  bodies.emplace(id, std::make_unique<Body>(s));
  return id;
}

std::unique_ptr<Store> new_store() {
  // OCCT's default messenger prints to stdout, which the import helper uses
  // for its progress protocol. Silence it; failures surface as exceptions.
  Message::DefaultMessenger()->ChangePrinters().Clear();
  return std::make_unique<Store>();
}

rust::String occt_version() { return rust::String(OCC_VERSION_COMPLETE); }

void enable_signal_handlers() {
  // Process-wide; only the import helper calls this (SPEC §3.4). No FPE
  // trapping: Rust code may produce NaN/inf legitimately.
  OSD::SetSignal(false);
}

void unload(Store &s, uint32_t body) { s.bodies.erase(body); }

size_t body_count(const Store &s) { return s.bodies.size(); }

} // namespace ntshim
