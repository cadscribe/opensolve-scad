//! Manifold 3.5.2 (github.com/elalish/manifold, Apache-2.0), compiled from
//! its sources: the mesh booleans OpenSCAD 2026.10 does its 3D work with,
//! for the mesh kernel. Only what the kernel uses: a solid from a mesh whose
//! triangles carry a number each, booleans of two or of many at once, and
//! the mesh of a result, each of its triangles with the number of the
//! triangle it is a piece of.
//!
//! Not for wasm: Manifold is C++.

use cxx::UniquePtr;

#[cxx::bridge(namespace = "opensolve_manifold")]
mod ffi {
    unsafe extern "C++" {
        include!("opensolve-manifold/src/bridge.h");

        type Solid;
        type Batch;

        fn solid_new(
            points: &[f64],
            tris: &[u32],
            faces: &[u64],
            tolerance: f64,
        ) -> Result<UniquePtr<Solid>>;
        fn solid_boolean(a: &Solid, b: &Solid, op: u8) -> Result<UniquePtr<Solid>>;
        fn batch_new() -> UniquePtr<Batch>;
        fn batch_push(batch: Pin<&mut Batch>, s: &Solid);
        fn batch_boolean(batch: &Batch, op: u8) -> Result<UniquePtr<Solid>>;
        fn batch_compose(batch: &Batch) -> Result<UniquePtr<Solid>>;
        fn solid_copy(s: &Solid) -> UniquePtr<Solid>;
        fn solid_num_tri(s: &Solid) -> usize;
        fn solid_bounds(s: &Solid, out: &mut [f64]);
        fn solid_status(s: &Solid) -> Result<i32>;
        fn solid_mesh(
            s: &Solid,
            points: &mut Vec<f64>,
            tris: &mut Vec<u32>,
            faces: &mut Vec<u64>,
        ) -> Result<()>;
        fn triangulate(points: &[f64], sizes: &[u32]) -> Result<Vec<u32>>;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Subtract,
    Intersect,
}

impl Op {
    fn code(self) -> u8 {
        match self {
            Op::Add => 0,
            Op::Subtract => 1,
            Op::Intersect => 2,
        }
    }
}

/// Why Manifold refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The mesh is not closed, or an edge has more than two triangles.
    NotManifold,
    /// Another of Manifold's errors, by its name.
    Status(&'static str),
    /// Manifold threw, with its message.
    Thrown(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotManifold => f.write_str("the mesh is not closed or not manifold"),
            Error::Status(s) => f.write_str(s),
            Error::Thrown(m) => write!(f, "Manifold failed: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// Manifold's `Error` by its number.
fn status(code: i32) -> Result<(), Error> {
    let name = match code {
        0 => return Ok(()),
        2 => return Err(Error::NotManifold),
        1 => "a corner is not a finite number",
        3 => "a triangle names a corner that does not exist",
        11 => "invalid construction",
        12 => "the result is too large",
        14 => "cancelled",
        _ => "the mesh is invalid",
    };
    Err(Error::Status(name))
}

fn thrown(e: cxx::Exception) -> Error {
    Error::Thrown(e.what().to_string())
}

/// A solid of Manifold's, its booleans done.
pub struct Solid(UniquePtr<ffi::Solid>);

// SAFETY: a `Solid` is only made by the functions here, which compute the
// result before they return it (`checked`); Manifold's object is then
// immutable, and its `const` methods only read it.
unsafe impl Send for Solid {}
// SAFETY: as above.
unsafe impl Sync for Solid {}

impl Clone for Solid {
    /// The same solid: Manifold shares its mesh, nothing is copied.
    fn clone(&self) -> Solid {
        Solid(ffi::solid_copy(&self.0))
    }
}

impl std::fmt::Debug for Solid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Solid").finish_non_exhaustive()
    }
}

/// A mesh: corners as flat xyz, triangles as three corners each,
/// counter-clockwise from outside, and a number for each triangle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub points: Vec<f64>,
    pub tris: Vec<u32>,
    pub faces: Vec<u64>,
}

impl Solid {
    /// The solid of a closed mesh; each triangle's number goes to the pieces
    /// of it in results. Edges shorter than `tolerance`, and slivers thinner,
    /// may be collapsed in results (Manifold's own baseline, from the size of
    /// the box, at the least).
    pub fn new(
        points: &[f64],
        tris: &[u32],
        faces: &[u64],
        tolerance: f64,
    ) -> Result<Solid, Error> {
        checked(ffi::solid_new(points, tris, faces, tolerance))
    }

    pub fn boolean(&self, other: &Solid, op: Op) -> Result<Solid, Error> {
        checked(ffi::solid_boolean(&self.0, &other.0, op.code()))
    }

    /// The first and the others by `op` at once: for a union, Manifold joins
    /// the small ones first; for a difference, the others are joined and
    /// then cut away.
    pub fn batch(list: &[&Solid], op: Op) -> Result<Solid, Error> {
        let mut batch = ffi::batch_new();
        for s in list {
            ffi::batch_push(batch.pin_mut(), &s.0);
        }
        checked(ffi::batch_boolean(&batch, op.code()))
    }

    /// Solids that do not meet as one, without a boolean: a mesh with a
    /// part for each.
    pub fn compose(list: &[&Solid]) -> Result<Solid, Error> {
        let mut batch = ffi::batch_new();
        for s in list {
            ffi::batch_push(batch.pin_mut(), &s.0);
        }
        checked(ffi::batch_compose(&batch))
    }

    pub fn is_empty(&self) -> bool {
        self.num_tri() == 0
    }

    pub fn num_tri(&self) -> usize {
        ffi::solid_num_tri(&self.0)
    }

    /// The bounding box: the lowest and the highest corner.
    pub fn bounds(&self) -> ([f64; 3], [f64; 3]) {
        let mut b = [0.0; 6];
        ffi::solid_bounds(&self.0, &mut b);
        ([b[0], b[1], b[2]], [b[3], b[4], b[5]])
    }

    pub fn mesh(&self) -> Result<Mesh, Error> {
        let mut m = Mesh::default();
        ffi::solid_mesh(&self.0, &mut m.points, &mut m.tris, &mut m.faces).map_err(thrown)?;
        Ok(m)
    }
}

/// Triangles of loops in the plane, as Manifold cuts them (the sharpest
/// ears first): outlines counter-clockwise, holes clockwise, any of them
/// nested; corners numbered through all loops in order.
pub fn triangulate(loops: &[Vec<[f64; 2]>]) -> Result<Vec<[u32; 3]>, Error> {
    let points: Vec<f64> = loops.iter().flatten().flatten().copied().collect();
    let sizes: Vec<u32> = loops.iter().map(|l| l.len() as u32).collect();
    let flat = ffi::triangulate(&points, &sizes).map_err(thrown)?;
    Ok(flat.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect())
}

/// The solid with its result computed, or Manifold's error.
fn checked(made: Result<UniquePtr<ffi::Solid>, cxx::Exception>) -> Result<Solid, Error> {
    let s = made.map_err(thrown)?;
    status(ffi::solid_status(&s).map_err(thrown)?)?;
    Ok(Solid(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box from `lo` to `hi`, the triangles of each side numbered by it.
    fn cube(lo: [f64; 3], hi: [f64; 3], first: u64) -> Solid {
        let mut points = Vec::new();
        for i in 0..8 {
            for k in 0..3 {
                points.push(if i >> k & 1 == 1 { hi[k] } else { lo[k] });
            }
        }
        let quads = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let mut tris = Vec::new();
        let mut faces = Vec::new();
        for (k, q) in quads.iter().enumerate() {
            tris.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
            faces.extend([first + k as u64; 2]);
        }
        Solid::new(&points, &tris, &faces, 0.0).unwrap()
    }

    fn volume(m: &Mesh) -> f64 {
        let p = |i: u32| {
            let i = i as usize * 3;
            [m.points[i], m.points[i + 1], m.points[i + 2]]
        };
        m.tris
            .chunks(3)
            .map(|t| {
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0])
            })
            .sum::<f64>()
            / 6.0
    }

    #[test]
    fn a_cut_keeps_the_numbers_of_both() {
        let a = cube([0.0; 3], [2.0; 3], 0);
        let b = cube([1.0; 3], [3.0; 3], 10);
        let m = a.boolean(&b, Op::Subtract).unwrap().mesh().unwrap();
        assert!((volume(&m) - 7.0).abs() < 1e-9);
        assert!(m.faces.iter().all(|&f| f < 16));
        assert!(m.faces.iter().any(|&f| f < 6) && m.faces.iter().any(|&f| f >= 10));
    }

    #[test]
    fn a_batch_joins_all() {
        let list: Vec<Solid> = (0..4)
            .map(|i| {
                cube(
                    [i as f64 * 0.5, 0.0, 0.0],
                    [i as f64 * 0.5 + 1.0, 1.0, 1.0],
                    0,
                )
            })
            .collect();
        let refs: Vec<&Solid> = list.iter().collect();
        let m = Solid::batch(&refs, Op::Add).unwrap().mesh().unwrap();
        assert!((volume(&m) - 2.5).abs() < 1e-9);
    }

    #[test]
    fn an_open_mesh_is_refused() {
        let points = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        assert_eq!(
            Solid::new(&points, &[0, 1, 2], &[0], 0.0).unwrap_err(),
            Error::NotManifold
        );
    }

    #[test]
    fn nothing_left_is_an_empty_mesh() {
        let a = cube([0.0; 3], [1.0; 3], 0);
        let b = cube([-1.0; 3], [2.0; 3], 0);
        let m = a.boolean(&b, Op::Subtract).unwrap().mesh().unwrap();
        assert!(m.tris.is_empty());
    }
}
