// newtopo OCCT shim (SPEC §3.4). The only C++ in the project.
// Flat, id-based API; no OCCT type crosses into Rust.
#pragma once

#include "rust/cxx.h"

#include <cstdint>
#include <exception>
#include <map>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

#include <BRepTopAdaptor_FClass2d.hxx>
#include <NCollection_IndexedMap.hxx>
#include <Standard_Failure.hxx>
#include <TCollection_ExtendedString.hxx>
#include <TopTools_ShapeMapHasher.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>

// Every bridge function declared `Result<..>` runs inside this: OCCT failures
// and any other C++ exception become a Rust `Err`, never an abort.
namespace rust {
namespace behavior {
template <typename Try, typename Fail>
static void trycatch(Try &&func, Fail &&fail) noexcept try {
  func();
} catch (const Standard_Failure &e) {
  // Print() writes "<type>: <message>" on OCCT 7.x and 8.x alike.
  std::ostringstream m;
  e.Print(m);
  fail(m.str());
} catch (const std::exception &e) {
  fail(e.what());
} catch (...) {
  fail("unknown C++ exception");
}
} // namespace behavior
} // namespace rust

namespace ntshim {

struct HealOptions;
struct HealResult;
struct TopologyFfi;
struct MeshFfi;
struct ImportResultFfi;

using ShapeMap = NCollection_IndexedMap<TopoDS_Shape, TopTools_ShapeMapHasher>;

// One loaded body: the shape plus the deterministic id tables.
struct Body {
  TopoDS_Shape shape;
  ShapeMap faces, edges, vertices;
  std::vector<std::vector<uint32_t>> edge_faces; // per edge, unique face ids
  std::map<uint32_t, std::unique_ptr<BRepTopAdaptor_FClass2d>> classifiers;
  double mesh_tol = -1.0;

  explicit Body(const TopoDS_Shape &s);
  void reindex();
  const TopoDS_Face &face(uint32_t id) const;      // as stored (oriented)
  TopoDS_Face face_fwd(uint32_t id) const;         // FORWARD copy
  const TopoDS_Shape &edge(uint32_t id) const;
  BRepTopAdaptor_FClass2d &classifier(uint32_t face);
};

class Store {
public:
  std::map<uint32_t, std::unique_ptr<Body>> bodies;
  uint32_t next_id = 1;

  Body &body(uint32_t id);
  uint32_t add(const TopoDS_Shape &s);
};

std::unique_ptr<Store> new_store();
rust::String occt_version();
void enable_signal_handlers();

// io.cpp
uint32_t load_brep(Store &s, rust::Slice<const uint8_t> bytes);
rust::Vec<uint8_t> save_brep(Store &s, uint32_t body);
void unload(Store &s, uint32_t body);
size_t body_count(const Store &s);
ImportResultFfi import_cad(Store &s, rust::Str path, uint8_t format, double unit_m);
HealResult heal(Store &s, uint32_t body, const HealOptions &opts);

// topo.cpp
TopologyFfi topology(Store &s, uint32_t body);
rust::Vec<double> face_eval(Store &s, uint32_t body, uint32_t face, rust::Slice<const double> uv);
rust::Vec<uint8_t> face_classify_uv(Store &s, uint32_t body, uint32_t face, rust::Slice<const double> uv);
rust::Vec<double> edge_eval(Store &s, uint32_t body, uint32_t edge, rust::Slice<const double> t);
rust::Vec<double> edge_pcurve_eval(Store &s, uint32_t body, uint32_t edge, uint32_t face, bool reversed,
                                   rust::Slice<const double> t);
rust::Vec<double> project_to_face(Store &s, uint32_t body, uint32_t face, rust::Slice<const double> p);
rust::Vec<double> project_to_edge(Store &s, uint32_t body, uint32_t edge, rust::Slice<const double> p);
MeshFfi tessellate(Store &s, uint32_t body, uint32_t face, double tol);

// Helpers shared between the .cpp files.
std::string to_utf8(const TCollection_ExtendedString &s);
[[noreturn]] void fail(const std::string &msg);

} // namespace ntshim
