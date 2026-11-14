// Packaging self-test: a small program linked against OCCT, packaged in
// place of newtopo-import so CI checks the dylib/.so/DLL bundling for real
// while the Rust binaries are stubs. Not shipped.
#include <STEPControl_Reader.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <TopExp_Explorer.hxx>
#include <iostream>
int main(int argc, char** argv) {
  TopoDS_Shape box = BRepPrimAPI_MakeBox(10, 20, 30).Shape();
  BRepMesh_IncrementalMesh mesh(box, 0.1);
  int faces = 0;
  for (TopExp_Explorer e(box, TopAbs_FACE); e.More(); e.Next()) ++faces;
  STEPControl_Reader reader;
  std::cout << "faces=" << faces << " occt-probe-ok" << std::endl;
  if (argc > 1) std::cout << "read=" << (int)reader.ReadFile(argv[1]) << std::endl;
  return 0;
}
