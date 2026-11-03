// The C++ side of the bridge to Manifold: only what the mesh kernel calls.

#pragma once

#include <cstdint>
#include <memory>
#include <vector>

#include "manifold/manifold.h"
#include "manifold/polygon.h"
#include "rust/cxx.h"

namespace opensolve_manifold {

// A mesh as Manifold holds it, its booleans done.
struct Solid {
  manifold::Manifold m;
};

// Solids gathered for one boolean of them all.
struct Batch {
  std::vector<manifold::Manifold> list;
};

// A solid from its corners (flat xyz), triangles (three corners each,
// counter-clockwise from outside) and a number for each triangle that its
// pieces keep through booleans. If it is not closed, corners at one place
// on edges that are open are joined, and it is tried again. Check it with
// `status`.
std::unique_ptr<Solid> solid_new(rust::Slice<const double> points,
                                 rust::Slice<const uint32_t> tris,
                                 rust::Slice<const uint64_t> faces);

// 0 union, 1 difference, 2 intersection.
std::unique_ptr<Solid> solid_boolean(const Solid& a, const Solid& b,
                                     uint8_t op);

std::unique_ptr<Batch> batch_new();
void batch_push(Batch& batch, const Solid& s);
// The first and the others by `op`, at once.
std::unique_ptr<Solid> batch_boolean(const Batch& batch, uint8_t op);

// The same solid: Manifold shares the mesh.
std::unique_ptr<Solid> solid_copy(const Solid& s);

size_t solid_num_tri(const Solid& s);

// The bounding box: lowest x, y, z, then highest.
void solid_bounds(const Solid& s, rust::Slice<double> out);

// Manifold's error, as its number (0 none); computes what is pending.
int32_t solid_status(const Solid& s);

// The mesh: corners, triangles and the number of each triangle.
void solid_mesh(const Solid& s, rust::Vec<double>& points,
                rust::Vec<uint32_t>& tris, rust::Vec<uint64_t>& faces);

// Triangles (three corners each) of loops given as their corners (flat xy)
// and the number of corners of each: outlines counter-clockwise, holes
// clockwise, any of them nested. Corners are numbered through all loops.
rust::Vec<uint32_t> triangulate(rust::Slice<const double> points,
                                rust::Slice<const uint32_t> sizes);

}  // namespace opensolve_manifold
