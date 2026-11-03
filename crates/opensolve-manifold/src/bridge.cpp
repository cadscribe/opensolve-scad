// The C++ side of the bridge to Manifold.

#include "opensolve-manifold/src/bridge.h"

#include "opensolve-manifold/src/lib.rs.h"

namespace opensolve_manifold {

namespace {

manifold::OpType op_of(uint8_t op) {
  switch (op) {
    case 1:
      return manifold::OpType::Subtract;
    case 2:
      return manifold::OpType::Intersect;
    default:
      return manifold::OpType::Add;
  }
}

}  // namespace

std::unique_ptr<Solid> solid_new(rust::Slice<const double> points,
                                 rust::Slice<const uint32_t> tris,
                                 rust::Slice<const uint64_t> faces) {
  manifold::MeshGL64 mesh;
  mesh.numProp = 3;
  mesh.vertProperties.assign(points.begin(), points.end());
  mesh.triVerts.assign(tris.begin(), tris.end());
  mesh.faceID.assign(faces.begin(), faces.end());
  manifold::Manifold m(mesh);
  // As OpenSCAD does: if it is not closed, copies of a corner on edges
  // that are open are made one corner, and it is tried again.
  if (m.Status() == manifold::Manifold::Error::NotManifold && mesh.Merge()) {
    m = manifold::Manifold(mesh);
  }
  return std::make_unique<Solid>(Solid{std::move(m)});
}

std::unique_ptr<Solid> solid_boolean(const Solid& a, const Solid& b,
                                     uint8_t op) {
  return std::make_unique<Solid>(Solid{a.m.Boolean(b.m, op_of(op))});
}

std::unique_ptr<Batch> batch_new() { return std::make_unique<Batch>(); }

void batch_push(Batch& batch, const Solid& s) { batch.list.push_back(s.m); }

std::unique_ptr<Solid> batch_boolean(const Batch& batch, uint8_t op) {
  return std::make_unique<Solid>(
      Solid{manifold::Manifold::BatchBoolean(batch.list, op_of(op))});
}

std::unique_ptr<Solid> solid_transform(const Solid& s,
                                       rust::Slice<const double> m) {
  manifold::mat3x4 t;
  for (size_t col = 0; col < 4; ++col) {
    for (size_t row = 0; row < 3; ++row) {
      t[col][row] = m[row * 4 + col];
    }
  }
  return std::make_unique<Solid>(Solid{s.m.Transform(t)});
}

std::unique_ptr<Solid> solid_copy(const Solid& s) {
  return std::make_unique<Solid>(Solid{s.m});
}

size_t solid_num_tri(const Solid& s) { return s.m.NumTri(); }

void solid_bounds(const Solid& s, rust::Slice<double> out) {
  const manifold::Box box = s.m.BoundingBox();
  for (size_t k = 0; k < 3 && k + 3 < out.size(); ++k) {
    out[k] = box.min[k];
    out[k + 3] = box.max[k];
  }
}

int32_t solid_status(const Solid& s) {
  return static_cast<int32_t>(s.m.Status());
}

void solid_mesh(const Solid& s, rust::Vec<double>& points,
                rust::Vec<uint32_t>& tris, rust::Vec<uint64_t>& faces,
                rust::Vec<uint32_t>& run_start,
                rust::Vec<double>& run_transform) {
  const manifold::MeshGL64 mesh = s.m.GetMeshGL64();
  const size_t n = mesh.NumVert();
  points.reserve(3 * n);
  for (size_t v = 0; v < n; ++v) {
    for (size_t k = 0; k < 3; ++k) {
      points.push_back(mesh.vertProperties[v * mesh.numProp + k]);
    }
  }
  tris.reserve(mesh.triVerts.size());
  for (uint64_t i : mesh.triVerts) {
    tris.push_back(static_cast<uint32_t>(i));
  }
  faces.reserve(mesh.faceID.size());
  for (uint64_t f : mesh.faceID) {
    faces.push_back(f);
  }
  const size_t runs = mesh.runOriginalID.size();
  for (size_t r = 0; r < runs && r < mesh.runIndex.size(); ++r) {
    run_start.push_back(static_cast<uint32_t>(mesh.runIndex[r] / 3));
  }
  run_start.push_back(static_cast<uint32_t>(mesh.triVerts.size() / 3));
  for (size_t r = 0; r < runs; ++r) {
    const manifold::mat3x4 t = mesh.GetRunTransform(r);
    for (size_t row = 0; row < 3; ++row) {
      for (size_t col = 0; col < 4; ++col) {
        run_transform.push_back(t[col][row]);
      }
    }
  }
}

rust::Vec<uint32_t> triangulate(rust::Slice<const double> points,
                                rust::Slice<const uint32_t> sizes) {
  manifold::PolygonsIdx polys;
  polys.reserve(sizes.size());
  int at = 0;
  for (uint32_t n : sizes) {
    manifold::SimplePolygonIdx poly;
    poly.reserve(n);
    for (uint32_t k = 0; k < n; ++k, ++at) {
      poly.push_back({manifold::vec2(points[2 * at], points[2 * at + 1]), at});
    }
    polys.push_back(std::move(poly));
  }
  rust::Vec<uint32_t> out;
  for (const manifold::ivec3& t : manifold::TriangulateIdx(polys)) {
    out.push_back(static_cast<uint32_t>(t[0]));
    out.push_back(static_cast<uint32_t>(t[1]));
    out.push_back(static_cast<uint32_t>(t[2]));
  }
  return out;
}

}  // namespace opensolve_manifold
