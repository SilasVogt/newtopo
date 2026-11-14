// Test-shape builder (feature `testshapes`); not part of the shim line budget.
#pragma once
#include "shim.h"

namespace ntshim {

struct XcafNode;

uint32_t ts_box(Store &s, double x, double y, double z, double dx, double dy, double dz);
uint32_t ts_cylinder(Store &s, double r, double h);
uint32_t ts_cone(Store &s, double r1, double r2, double h);
uint32_t ts_sphere(Store &s, double r);
uint32_t ts_torus(Store &s, double major, double minor);
uint32_t ts_transform(Store &s, uint32_t body, rust::Slice<const double> m);
uint32_t ts_fillet(Store &s, uint32_t body, rust::Slice<const uint32_t> edges, double r);
uint32_t ts_chamfer(Store &s, uint32_t body, rust::Slice<const uint32_t> edges, double d);
uint32_t ts_boolean(Store &s, uint8_t op, uint32_t a, rust::Slice<const uint32_t> tools);
uint32_t ts_revolve(Store &s, rust::Slice<const double> profile, int32_t spline_from, int32_t spline_to);
uint32_t ts_prism(Store &s, rust::Slice<const double> polygon, double height);
void ts_write_brep_text(Store &s, uint32_t body, rust::Str path);
void ts_write_step(Store &s, rust::Str path, rust::Slice<const XcafNode> nodes);

} // namespace ntshim
