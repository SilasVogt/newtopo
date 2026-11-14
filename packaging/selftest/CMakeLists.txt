# Packaging self-test (see occt_probe.cpp). Configure with
#   cmake -S packaging/selftest -B build-probe -DOpenCASCADE_DIR=$OCCT_DIR/lib/cmake/opencascade
cmake_minimum_required(VERSION 3.16)
project(occt_probe CXX)
set(CMAKE_CXX_STANDARD 17)
find_package(OpenCASCADE REQUIRED CONFIG)
add_executable(newtopo-import occt_probe.cpp)
target_include_directories(newtopo-import PRIVATE ${OpenCASCADE_INCLUDE_DIR})
target_link_libraries(newtopo-import PRIVATE TKDESTEP TKXSBase TKMesh TKPrim TKTopAlgo TKBRep TKGeomBase TKG3d TKMath TKernel)
