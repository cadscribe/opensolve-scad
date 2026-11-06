//! The solid for other programs: 3MF for slicers, STEP for CAD. Both are
//! written by hand, in millimetres.
//!
//! STEP holds the mesh as a faceted boundary representation (AP214, a
//! `FACETED_BREP` of a `CLOSED_SHELL` for each piece): the triangles next to
//! each other in one plane are one `FACE_SURFACE` on its `PLANE`, so a CAD
//! program sees flat faces as flat, and round ones as the facets they are
//! printed with.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Write as _;

use crate::{Facets, Solid, zip};

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>
"#;
const RELATIONSHIPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rel0" Target="/3D/3dmodel.model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>
"#;

fn xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The file for an extension (`stl`, `3mf`, `step` or `stp`, any case);
/// `None` for one that is not written.
#[must_use]
pub fn encode(solid: &Solid, facets: &Facets, ext: &str, name: &str) -> Option<Vec<u8>> {
    match ext.to_ascii_lowercase().as_str() {
        "stl" => Some(solid.to_stl()),
        "3mf" => Some(three_mf(solid, name)),
        "step" | "stp" => Some(step(solid, facets, name).into_bytes()),
        _ => None,
    }
}

/// The solid as a 3MF package: one object, one build item.
pub fn three_mf(solid: &Solid, name: &str) -> Vec<u8> {
    three_mf_objects(&[(solid, name, 1)])
}

/// A name for an id that stays the same for the same input, shaped as a
/// UUID, which Bambu Studio wants on objects and build items.
fn uuid(seed: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h);
    let a = h.finish();
    (seed, a).hash(&mut h);
    let b = h.finish();
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xfff,
        b >> 52,
        b & 0xffff_ffff_ffff
    )
}

/// An object to print: its name and its parts, each a solid with its name
/// and its filament (extruder) slot, from 1.
pub type PrintObject<'a> = (&'a str, Vec<(&'a Solid, &'a str, u32)>);

/// Solids as a 3MF package, each an object of its own with a build item, so
/// that a slicer takes them as parts to place one by one, each with the
/// filament slot given. See [`three_mf_print`].
pub fn three_mf_objects(objects: &[(&Solid, &str, u32)]) -> Vec<u8> {
    let objects: Vec<PrintObject> = objects
        .iter()
        .map(|&(solid, name, extruder)| (name, vec![(solid, name, extruder)]))
        .collect();
    three_mf_print(&objects)
}

/// Solids as one object of a 3MF package made of them as its parts: put
/// together they print together, each part in its colour.
pub fn three_mf_assembly(name: &str, parts: &[(&Solid, &str, u32)]) -> Vec<u8> {
    three_mf_print(&[(name, parts.to_vec())])
}

/// Objects to print as a 3MF package, each with a build item; an object of
/// several parts is a slicer's multi-part object, printed as one, each part
/// in its filament. Written as Bambu Studio writes its own (its namespace,
/// UUIDs of the production extension, and `Metadata/model_settings.config`
/// with the objects, their parts' extruders and one plate), so that it
/// takes more than the geometry; other slicers read the core 3MF in it.
pub fn three_mf_print(objects: &[PrintObject]) -> Vec<u8> {
    print_package_painted(objects, &[], &[])
}

/// Red, green, blue and opacity, each from 0 to 1.
pub type Rgba = [f64; 4];

/// Solids in colours as one object of a 3MF package, as
/// [`three_mf_assembly`] puts them together: each part with its name and
/// colour, or none. The colours are the core's base materials, which
/// slicers show; each colour is a filament slot of its own as well, in the
/// order they first come, slot 1 for parts without one.
pub fn three_mf_painted(name: &str, parts: &[(&Solid, &str, Option<Rgba>)]) -> Vec<u8> {
    let mut colours: Vec<Rgba> = Vec::new();
    let mut slots = Vec::new();
    for (solid, part, colour) in parts {
        let slot = match colour {
            Some(c) => match colours
                .iter()
                .position(|k| k.map(f64::to_bits) == c.map(f64::to_bits))
            {
                Some(i) => i + 1,
                None => {
                    colours.push(*c);
                    colours.len()
                }
            },
            None => 0,
        };
        slots.push((*solid, *part, slot as u32));
    }
    // Slot 0 is for parts without a colour: the first filament.
    let parts: Vec<(&Solid, &str, u32)> = slots.iter().map(|&(s, p, k)| (s, p, k.max(1))).collect();
    let painted: Vec<Option<usize>> = slots
        .iter()
        .map(|&(_, _, k)| (k > 0).then(|| k as usize - 1))
        .collect();
    print_package_painted(&[(name, parts)], &colours, &painted)
}

/// `#RRGGBBAA` of a colour.
fn hex(c: &Rgba) -> String {
    let byte = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        byte(c[0]),
        byte(c[1]),
        byte(c[2]),
        byte(c[3])
    )
}

/// [`three_mf_print`], the parts given colours (the base material of
/// `colours` that `painted` names, part by part in order) where there are.
fn print_package_painted(
    objects: &[PrintObject],
    colours: &[Rgba],
    painted: &[Option<usize>],
) -> Vec<u8> {
    let identity = "1 0 0 0 1 0 0 0 1 0 0 0";
    let mut resources = String::new();
    let mut build = String::new();
    let mut settings = String::new();
    let mut instances = String::new();
    let mut assemble = String::new();
    let mut next = 1;
    // The colours first: a resource is defined before it is used.
    let materials = (!colours.is_empty()).then(|| {
        let id = next;
        next += 1;
        let _ = writeln!(resources, "<basematerials id=\"{id}\">");
        for (i, c) in colours.iter().enumerate() {
            let _ = writeln!(
                resources,
                "<base name=\"Colour {}\" displaycolor=\"{}\"/>",
                i + 1,
                hex(c)
            );
        }
        resources.push_str("</basematerials>\n");
        id
    });
    let mut part_no = 0;
    for (k, (name, parts)) in objects.iter().enumerate() {
        let name = xml_text(name);
        let mut components = String::new();
        let mut volumes = String::new();
        let mut faces = 0;
        let first = next;
        for (v, (solid, part, extruder)) in parts.iter().enumerate() {
            let id = next;
            next += 1;
            let part = xml_text(part);
            let material = materials.zip(painted.get(part_no).copied().flatten());
            part_no += 1;
            mesh_object(&mut resources, id, &part, solid, material);
            let _ = writeln!(
                components,
                "<component objectid=\"{id}\" p:UUID=\"{}\" transform=\"{identity}\"/>",
                uuid(&format!("component {id} {part}"))
            );
            faces += solid.tris.len();
            let _ = write!(
                volumes,
                "    <part id=\"{id}\" subtype=\"normal_part\">\n\
                 \x20     <metadata key=\"name\" value=\"{part}\"/>\n\
                 \x20     <metadata key=\"matrix\" value=\"1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1\"/>\n\
                 \x20     <metadata key=\"source_object_id\" value=\"{k}\"/>\n\
                 \x20     <metadata key=\"source_volume_id\" value=\"{v}\"/>\n\
                 \x20     <metadata key=\"extruder\" value=\"{extruder}\"/>\n\
                 \x20     <mesh_stat face_count=\"{}\" edges_fixed=\"0\" degenerate_facets=\"0\" \
                 facets_removed=\"0\" facets_reversed=\"0\" backwards_edges=\"0\"/>\n\
                 \x20   </part>\n",
                solid.tris.len()
            );
        }
        // One part: the mesh is the object; several: an object of them.
        let id = if parts.len() == 1 {
            first
        } else {
            let id = next;
            next += 1;
            let _ = writeln!(
                resources,
                "<object id=\"{id}\" name=\"{name}\" type=\"model\" p:UUID=\"{}\">\n<components>\n{components}</components>\n</object>",
                uuid(&format!("object {id} {name}"))
            );
            id
        };
        let extruder = parts.first().map_or(1, |p| p.2);
        let _ = writeln!(
            build,
            "<item objectid=\"{id}\" p:UUID=\"{}\" transform=\"{identity}\" printable=\"1\"/>",
            uuid(&format!("item {id} {name}"))
        );
        let _ = write!(
            settings,
            "  <object id=\"{id}\">\n\
             \x20   <metadata key=\"name\" value=\"{name}\"/>\n\
             \x20   <metadata key=\"extruder\" value=\"{extruder}\"/>\n\
             \x20   <metadata key=\"\" face_count=\"{faces}\"/>\n{volumes}  </object>\n"
        );
        let _ = write!(
            instances,
            "    <model_instance>\n\
             \x20     <metadata key=\"object_id\" value=\"{id}\"/>\n\
             \x20     <metadata key=\"instance_id\" value=\"0\"/>\n\
             \x20     <metadata key=\"identify_id\" value=\"{id}\"/>\n\
             \x20   </model_instance>\n"
        );
        let _ = writeln!(
            assemble,
            "    <assemble_item object_id=\"{id}\" instance_id=\"0\" transform=\"{identity}\" offset=\"0 0 0\"/>"
        );
    }
    package(&resources, &build, &settings, &instances, &assemble)
}

/// A mesh as an object of a 3MF package.
/// With `material`, the base materials' id and the number of its colour.
fn mesh_object(
    out: &mut String,
    id: usize,
    name: &str,
    solid: &Solid,
    material: Option<(usize, usize)>,
) {
    let paint = material.map_or(String::new(), |(m, i)| {
        format!(" pid=\"{m}\" pindex=\"{i}\"")
    });
    let _ = writeln!(
        out,
        "<object id=\"{id}\" name=\"{name}\" type=\"model\"{paint} p:UUID=\"{}\">\n<mesh>\n<vertices>",
        uuid(&format!("object {id} {name}"))
    );
    for p in &solid.points {
        let _ = writeln!(out, r#"<vertex x="{}" y="{}" z="{}"/>"#, p[0], p[1], p[2]);
    }
    out.push_str("</vertices>\n<triangles>\n");
    for t in &solid.tris {
        let _ = writeln!(
            out,
            r#"<triangle v1="{}" v2="{}" v3="{}"/>"#,
            t[0], t[1], t[2]
        );
    }
    out.push_str("</triangles>\n</mesh>\n</object>\n");
}

/// The package round the objects: the model with Bambu Studio's namespace
/// and the production extension, and its model settings.
fn package(
    resources: &str,
    build: &str,
    settings: &str,
    instances: &str,
    assemble: &str,
) -> Vec<u8> {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <model unit=\"millimeter\" xml:lang=\"en-US\" \
         xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\" \
         xmlns:p=\"http://schemas.microsoft.com/3dmanufacturing/production/2015/06\" \
         xmlns:BambuStudio=\"http://schemas.bambulab.com/package/2021\" requiredextensions=\"p\">\n\
         <metadata name=\"Application\">OpenSolve</metadata>\n\
         <metadata name=\"BambuStudio:3mfVersion\">1</metadata>\n\
         <resources>\n{resources}</resources>\n\
         <build p:UUID=\"{}\">\n{build}</build>\n</model>\n",
        uuid("build")
    );
    let config = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n{settings}  <plate>\n\
         \x20   <metadata key=\"plater_id\" value=\"1\"/>\n\
         \x20   <metadata key=\"plater_name\" value=\"\"/>\n\
         \x20   <metadata key=\"locked\" value=\"false\"/>\n{instances}  </plate>\n  <assemble>\n{assemble}  </assemble>\n</config>\n"
    );
    let mut files = BTreeMap::new();
    files.insert(
        "[Content_Types].xml".to_string(),
        CONTENT_TYPES.as_bytes().to_vec(),
    );
    files.insert("_rels/.rels".to_string(), RELATIONSHIPS.as_bytes().to_vec());
    files.insert("3D/3dmodel.model".to_string(), xml.into_bytes());
    files.insert(
        "Metadata/model_settings.config".to_string(),
        config.into_bytes(),
    );
    zip::write(&files)
}

/// The boundary loops of a set of triangles: each loop as point indices, the
/// triangles' winding kept.
fn loops(tris: &[[u32; 3]]) -> Vec<Vec<u32>> {
    let mut count: HashMap<(u32, u32), i32> = HashMap::new();
    for t in tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *count.entry((a, b)).or_default() += 1;
            *count.entry((b, a)).or_default() -= 1;
        }
    }
    // An edge that runs once more forwards than back is on the boundary.
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&(a, b), &n) in &count {
        if n > 0 {
            next.entry(a).or_default().push(b);
        }
    }
    let mut out = Vec::new();
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    for s in starts {
        while let Some(first) = next.get_mut(&s).and_then(Vec::pop) {
            let mut ring = vec![s];
            let mut at = first;
            while at != s {
                ring.push(at);
                match next.get_mut(&at).and_then(Vec::pop) {
                    Some(n) => at = n,
                    None => break,
                }
            }
            if ring.len() >= 3 {
                out.push(ring);
            }
        }
    }
    out
}

/// How far a corner may be from a face's plane and still be on it, in
/// millimetres: the file's own `distance_accuracy_value`.
const ON_PLANE: f64 = 1e-6;

/// Corners closer than this are one, in millimetres: the file has corners
/// to the micrometre, and two that it writes at one place would bound an
/// edge of no length, on which Open CASCADE drops the whole solid. A
/// triangle less high than this is a sliver, whose own normal says nothing.
const WELD: f64 = 1e-5;

fn v_dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn v_sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn v_cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn v_unit(a: [f64; 3]) -> [f64; 3] {
    let l = v_dot(a, a).sqrt().max(1e-300);
    a.map(|x| x / l)
}

/// The mesh as STEP sees it: corners at the same place made one, so that
/// faces meet at their edges, and each triangle's outward normal (its
/// facet's plane, where the facet is the triangle's), area and whether it
/// is a sliver.
struct Mesh {
    points: Vec<[f64; 3]>,
    tris: Vec<[u32; 3]>,
    normals: Vec<[f64; 3]>,
    areas: Vec<f64>,
    slivers: Vec<bool>,
}

impl Mesh {
    fn new(solid: &Solid, facets: &Facets) -> Mesh {
        let cell = |p: [f64; 3]| p.map(|x| (x / WELD).floor() as i64);
        let mut at: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
        let mut points: Vec<[f64; 3]> = Vec::new();
        let same: Vec<u32> = solid
            .points
            .iter()
            .map(|&p| {
                let c = cell(p);
                let mut near = None;
                'look: for dx in -1..=1 {
                    for dy in -1..=1 {
                        for dz in -1..=1 {
                            let k = [c[0] + dx, c[1] + dy, c[2] + dz];
                            for &i in at.get(&k).map_or(&[][..], Vec::as_slice) {
                                let d = v_sub(points[i as usize], p);
                                if v_dot(d, d) <= WELD * WELD {
                                    near = Some(i);
                                    break 'look;
                                }
                            }
                        }
                    }
                }
                near.unwrap_or_else(|| {
                    points.push(p);
                    let i = points.len() as u32 - 1;
                    at.entry(c).or_default().push(i);
                    i
                })
            })
            .collect();
        let mut tris = Vec::new();
        let mut normals = Vec::new();
        let mut areas = Vec::new();
        let mut slivers = Vec::new();
        for (k, t) in solid.tris.iter().enumerate() {
            let t = t.map(|i| same[i as usize]);
            if t[0] == t[1] || t[1] == t[2] || t[2] == t[0] {
                continue;
            }
            let [a, b, c] = t.map(|i| points[i as usize]);
            let n = v_cross(v_sub(b, a), v_sub(c, a));
            let area = v_dot(n, n).sqrt() / 2.0;
            let longest = [v_sub(b, a), v_sub(c, b), v_sub(a, c)]
                .map(|e| v_dot(e, e))
                .into_iter()
                .fold(0.0, f64::max)
                .sqrt();
            let sliver = 2.0 * area < WELD * longest;
            let own = v_unit(n);
            // The facet's plane is exact where the corners are rounded; a
            // facet that does not face the triangle's way is not taken.
            let normal = solid
                .facets
                .get(k)
                .filter(|&&f| (f as usize) < facets.len())
                .map(|&f| v_unit(facets.get(f).normal))
                .filter(|&f| sliver || v_dot(f, own) > 0.0)
                .unwrap_or(own);
            tris.push(t);
            normals.push(normal);
            areas.push(area);
            slivers.push(sliver);
        }
        Mesh {
            points,
            tris,
            normals,
            areas,
            slivers,
        }
    }

    fn point(&self, i: u32) -> [f64; 3] {
        self.points[i as usize]
    }

    /// The triangles at each edge, either way round.
    fn edges(&self) -> HashMap<(u32, u32), Vec<usize>> {
        let mut out: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (k, t) in self.tris.iter().enumerate() {
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                out.entry((a.min(b), a.max(b))).or_default().push(k);
            }
        }
        out
    }

    /// Six times the volume the triangles close.
    fn volume6(&self, tris: &[usize]) -> f64 {
        tris.iter()
            .map(|&k| {
                let [a, b, c] = self.tris[k].map(|i| self.point(i));
                v_dot(a, v_cross(b, c))
            })
            .sum()
    }

    /// Whether `p` is inside the closed triangles: the crossings of a ray
    /// from it, in a direction no mesh of a CAD program lines up with.
    fn contains(&self, tris: &[usize], p: [f64; 3]) -> bool {
        let d = v_unit([0.577_215_664_9, 0.618_033_988_7, 0.533_049_088_1]);
        let mut inside = false;
        for &k in tris {
            let [a, b, c] = self.tris[k].map(|i| self.point(i));
            // Möller-Trumbore.
            let (e1, e2) = (v_sub(b, a), v_sub(c, a));
            let h = v_cross(d, e2);
            let det = v_dot(e1, h);
            if det.abs() < 1e-18 {
                continue;
            }
            let s = v_sub(p, a);
            let u = v_dot(s, h) / det;
            let q = v_cross(s, e1);
            let v = v_dot(d, q) / det;
            if u < 0.0 || v < 0.0 || u + v > 1.0 {
                continue;
            }
            if v_dot(e2, q) / det > 0.0 {
                inside = !inside;
            }
        }
        inside
    }
}

/// The pieces of the mesh that hang together by their edges, in the order
/// of their first triangles. Only an edge of two triangles holds: solids
/// that touch along an edge are pieces of their own, each closed.
fn pieces(mesh: &Mesh, edges: &HashMap<(u32, u32), Vec<usize>>) -> Vec<Vec<usize>> {
    let mut seen = vec![false; mesh.tris.len()];
    let mut out = Vec::new();
    for s in 0..mesh.tris.len() {
        if seen[s] {
            continue;
        }
        seen[s] = true;
        let mut piece = vec![s];
        let mut i = 0;
        while i < piece.len() {
            let t = mesh.tris[piece[i]];
            i += 1;
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                let by = &edges[&(a.min(b), a.max(b))];
                if by.len() != 2 {
                    continue;
                }
                for &n in by {
                    if !seen[n] {
                        seen[n] = true;
                        piece.push(n);
                    }
                }
            }
        }
        piece.sort_unstable();
        out.push(piece);
    }
    out
}

/// The faces of a piece: triangles next to each other in one plane made
/// one, each face its plane's normal and its loops, the outer one first.
/// `flip` turns the piece inside out, as a void's shell is written.
fn faces_of(
    mesh: &Mesh,
    edges: &HashMap<(u32, u32), Vec<usize>>,
    piece: &[usize],
    flip: bool,
) -> Vec<([f64; 3], Vec<Vec<u32>>)> {
    // The largest triangles first: a face's plane is its largest one's.
    let mut order = piece.to_vec();
    order.sort_by(|&a, &b| mesh.areas[b].total_cmp(&mesh.areas[a]).then(a.cmp(&b)));
    let mut taken: HashMap<usize, bool> = piece.iter().map(|&k| (k, false)).collect();
    let mut out = Vec::new();
    for s in order {
        if taken[&s] {
            continue;
        }
        taken.insert(s, true);
        let n = mesh.normals[s];
        let d = v_dot(n, mesh.point(mesh.tris[s][0]));
        let mut face = vec![s];
        let mut i = 0;
        while i < face.len() {
            let t = mesh.tris[face[i]];
            i += 1;
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                for &u in &edges[&(a.min(b), a.max(b))] {
                    if taken.get(&u) != Some(&false) {
                        continue;
                    }
                    // A sliver goes with the face it lies on, whichever way
                    // it seems to face.
                    let sliver = mesh.slivers[u];
                    let near = if sliver { WELD } else { ON_PLANE };
                    let flat = (sliver || v_dot(mesh.normals[u], n) > 0.0)
                        && mesh.tris[u]
                            .iter()
                            .all(|&c| (v_dot(n, mesh.point(c)) - d).abs() <= near);
                    if flat {
                        taken.insert(u, true);
                        face.push(u);
                    }
                }
            }
        }
        let tris: Vec<[u32; 3]> = face
            .iter()
            .map(|&k| {
                let [a, b, c] = mesh.tris[k];
                if flip { [a, c, b] } else { [a, b, c] }
            })
            .collect();
        let normal = if flip { n.map(|x| -x) } else { n };
        // The outer loop is the one round the most area.
        let area = |ring: &Vec<u32>| {
            let mut sum = [0.0; 3];
            for (j, &p) in ring.iter().enumerate() {
                let q = ring[(j + 1) % ring.len()];
                let c = v_cross(mesh.point(p), mesh.point(q));
                sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
            }
            v_dot(sum, normal)
        };
        let mut rings = loops(&tris);
        if rings.is_empty() {
            continue;
        }
        let outer = (0..rings.len())
            .max_by(|&a, &b| area(&rings[a]).total_cmp(&area(&rings[b])))
            .unwrap_or(0);
        rings.swap(0, outer);
        out.push((normal, rings));
    }
    out
}

/// The solid as STEP (ISO 10303-21, AP214): a faceted boundary
/// representation. Each face is a `FACE_SURFACE` on its `PLANE`, bounded by
/// `POLY_LOOP`s, as the standard wants a faceted B-rep's faces: a bare `FACE`
/// has no surface, and importers (Open CASCADE's, so FreeCAD's) read none of
/// it. The triangles of the mesh next to each other in one plane are one
/// face. Each piece of the solid is a `FACETED_BREP` of its own, a void
/// inside one a shell of its `BREP_WITH_VOIDS`.
pub fn step(solid: &Solid, facets: &Facets, name: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut add = |l: String| {
        lines.push(l);
        lines.len()
    };
    let mesh = Mesh::new(solid, facets);
    let edges = mesh.edges();
    let mut points: HashMap<u32, usize> = HashMap::new();
    let mut shell = |piece: &[usize], flip: bool, add: &mut dyn FnMut(String) -> usize| {
        let mut faces = Vec::new();
        for (normal, rings) in faces_of(&mesh, &edges, piece, flip) {
            let mut bounds = Vec::new();
            for (k, ring) in rings.iter().enumerate() {
                let ids: Vec<String> = ring
                    .iter()
                    .map(|&i| {
                        let id = *points.entry(i).or_insert_with(|| {
                            let p = mesh.point(i);
                            add(format!(
                                "CARTESIAN_POINT('',({:.6},{:.6},{:.6}))",
                                p[0], p[1], p[2]
                            ))
                        });
                        format!("#{id}")
                    })
                    .collect();
                let poly = add(format!("POLY_LOOP('',({}))", ids.join(",")));
                let kind = if k == 0 {
                    "FACE_OUTER_BOUND"
                } else {
                    "FACE_BOUND"
                };
                bounds.push(format!("#{}", add(format!("{kind}('',#{poly},.T.)"))));
            }
            // The plane through the outer loop's first corner, its z the
            // face's normal, its x any direction in it.
            let at = mesh.point(rings[0][0]);
            let other = if normal[0].abs() < 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            let x = v_unit(v_cross(other, normal));
            let origin = add(format!(
                "CARTESIAN_POINT('',({:.6},{:.6},{:.6}))",
                at[0], at[1], at[2]
            ));
            let z = add(format!(
                "DIRECTION('',({:.12},{:.12},{:.12}))",
                normal[0], normal[1], normal[2]
            ));
            let x = add(format!(
                "DIRECTION('',({:.12},{:.12},{:.12}))",
                x[0], x[1], x[2]
            ));
            let axis = add(format!("AXIS2_PLACEMENT_3D('',#{origin},#{z},#{x})"));
            let plane = add(format!("PLANE('',#{axis})"));
            faces.push(format!(
                "#{}",
                add(format!(
                    "FACE_SURFACE('',({}),#{plane},.T.)",
                    bounds.join(",")
                ))
            ));
        }
        add(format!("CLOSED_SHELL('',({}))", faces.join(",")))
    };
    // Pieces closing a volume are solids; the others, inside out, voids in
    // the smallest solid round them (or solids of their own, turned).
    let all = pieces(&mesh, &edges);
    let volumes: Vec<f64> = all.iter().map(|p| mesh.volume6(p)).collect();
    let mut voids: Vec<Vec<usize>> = vec![Vec::new(); all.len()];
    let mut solids = Vec::new();
    for (k, piece) in all.iter().enumerate() {
        // A sheet of no thickness, faces back to back, is no solid.
        if volumes[k].abs() < 1e-9 {
            continue;
        }
        if volumes[k] >= 0.0 {
            solids.push(k);
            continue;
        }
        let p = mesh.point(mesh.tris[piece[0]][0]);
        let round = (0..all.len())
            .filter(|&o| volumes[o] > -volumes[k] && mesh.contains(&all[o], p))
            .min_by(|&a, &b| volumes[a].total_cmp(&volumes[b]));
        match round {
            Some(o) => voids[o].push(k),
            None => solids.push(k),
        }
    }
    solids.sort_unstable();
    let label = name.replace('\'', "");
    let mut breps = Vec::new();
    let voided = voids.iter().any(|v| !v.is_empty());
    for k in solids {
        let outer = shell(&all[k], volumes[k] < 0.0, &mut add);
        let brep = if voids[k].is_empty() {
            add(format!("FACETED_BREP('{label}',#{outer})"))
        } else {
            let inner: Vec<String> = voids[k]
                .iter()
                .map(|&v| {
                    let s = shell(&all[v], true, &mut add);
                    format!("#{}", add(format!("ORIENTED_CLOSED_SHELL('',*,#{s},.F.)")))
                })
                .collect();
            // Open CASCADE reads the void of a plain BREP_WITH_VOIDS, and
            // not of one made a FACETED_BREP as well.
            add(format!(
                "BREP_WITH_VOIDS('{label}',#{outer},({}))",
                inner.join(",")
            ))
        };
        breps.push(format!("#{brep}"));
    }
    let origin = add("CARTESIAN_POINT('',(0.,0.,0.))".into());
    let z = add("DIRECTION('',(0.,0.,1.))".into());
    let x = add("DIRECTION('',(1.,0.,0.))".into());
    let axis = add(format!("AXIS2_PLACEMENT_3D('',#{origin},#{z},#{x})"));
    let length = add("(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.))".into());
    let angle = add("(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.))".into());
    let solid_angle = add("(NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT())".into());
    let unc = add(format!(
        "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-6),#{length},'distance_accuracy_value','')"
    ));
    let ctx = add(format!(
        "(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc})) \
         GLOBAL_UNIT_ASSIGNED_CONTEXT((#{length},#{angle},#{solid_angle})) \
         REPRESENTATION_CONTEXT('',''))"
    ));
    let app = add("APPLICATION_CONTEXT('automotive design')".into());
    add(format!(
        "APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})"
    ));
    let pctx = add(format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
    let product = add(format!(
        "PRODUCT('{0}','{0}','',(#{pctx}))",
        name.replace('\'', "")
    ));
    let dctx = add(format!(
        "PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"
    ));
    let formation = add(format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
    let def = add(format!(
        "PRODUCT_DEFINITION('design','',#{formation},#{dctx})"
    ));
    let shape = add(format!("PRODUCT_DEFINITION_SHAPE('','',#{def})"));
    // A faceted B-rep's representation holds faceted B-reps only; with a
    // void it is a shape's.
    let kind = if voided {
        "SHAPE_REPRESENTATION"
    } else {
        "FACETED_BREP_SHAPE_REPRESENTATION"
    };
    let rep = add(format!("{kind}('',(#{axis},{}),#{ctx})", breps.join(",")));
    add(format!("SHAPE_DEFINITION_REPRESENTATION(#{shape},#{rep})"));
    let mut out =
        String::from("ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('OpenSolve part'),'2;1');\n");
    let _ = writeln!(
        out,
        "FILE_NAME('{}.stp','',(''),(''),'OpenSolve','OpenSolve','');",
        name.replace('\'', "")
    );
    out.push_str("FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\nENDSEC;\nDATA;\n");
    for (i, l) in lines.iter().enumerate() {
        let _ = writeln!(out, "#{}={l};", i + 1);
    }
    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FaceId, Op, Outline, prism};

    fn holed_block() -> (Solid, Facets) {
        let mut f = Facets::default();
        let o = |p: Vec<[f64; 2]>, b: u64| Outline {
            faces: (0..p.len() as u64).map(|i| FaceId(b + i)).collect(),
            points: p,
        };
        let a = prism(
            &mut f,
            &o(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]], 0),
            0.0,
            5.0,
            FaceId(100),
            FaceId(101),
        )
        .unwrap();
        let h = prism(
            &mut f,
            &o(vec![[4.0, 4.0], [6.0, 4.0], [6.0, 6.0], [4.0, 6.0]], 200),
            0.0,
            5.0,
            FaceId(300),
            FaceId(301),
        )
        .unwrap();
        (a.boolean(&h, Op::Subtract, &mut f).unwrap(), f)
    }

    #[test]
    fn three_mf_is_a_zip_with_the_mesh() {
        let (s, _) = holed_block();
        let z = three_mf(&s, "A & B");
        assert_eq!(&z[..2], b"PK");
        let text = String::from_utf8_lossy(&z);
        assert!(text.contains("<triangle ") && text.contains("A &amp; B"));
    }

    #[test]
    fn three_mf_keeps_each_part_an_object() {
        let (s, _) = holed_block();
        let z = three_mf_objects(&[(&s, "Lid", 1), (&s, "Case & Co", 2)]);
        let text = String::from_utf8_lossy(&z);
        assert_eq!(text.matches("<object id=").count(), 4);
        assert!(text.contains(r#"<item objectid="2" p:UUID=""#));
        assert!(text.contains(r#"name="Case &amp; Co""#));
        // What Bambu Studio looks for to take it as its own.
        assert!(text.contains("Metadata/model_settings.config"));
        assert!(text.contains(r#"<metadata name="BambuStudio:3mfVersion">1</metadata>"#));
        assert!(text.contains(r#"<metadata key="extruder" value="2"/>"#));
        assert!(text.contains(r#"<assemble_item object_id="2""#));
        // Same input, same ids.
        assert_eq!(three_mf_objects(&[(&s, "Lid", 1), (&s, "Case & Co", 2)]), z);
    }

    #[test]
    fn three_mf_puts_parts_together_as_one_object() {
        let (s, _) = holed_block();
        let z = three_mf_assembly("Case", &[(&s, "Front", 1), (&s, "Base", 2)]);
        let text = String::from_utf8_lossy(&z);
        // Two meshes and the object made of them, which alone is built.
        assert_eq!(text.matches(r#"type="model""#).count(), 3);
        assert_eq!(text.matches("<component objectid=").count(), 2);
        assert!(text.contains(r#"<item objectid="3" "#));
        assert_eq!(text.matches("<item ").count(), 1);
        // One object of two parts, each its filament.
        assert!(text.contains(r#"<part id="2" subtype="normal_part">"#));
        assert!(text.contains(r#"<metadata key="extruder" value="2"/>"#));
    }

    #[test]
    fn three_mf_paints_parts_in_their_colours() {
        let (s, _) = holed_block();
        let red = [1.0, 0.0, 0.0, 1.0];
        let z = three_mf_painted(
            "Box",
            &[(&s, "a", Some(red)), (&s, "b", None), (&s, "c", Some(red))],
        );
        let text = String::from_utf8_lossy(&z);
        // One colour, defined first; two parts in it, one in none.
        assert_eq!(text.matches("<base ").count(), 1);
        assert!(text.contains(r##"<basematerials id="1">"##));
        assert!(text.contains(r##"displaycolor="#FF0000FF""##));
        assert_eq!(text.matches(r#"pid="1" pindex="0""#).count(), 2);
        assert_eq!(text.matches("<mesh>").count(), 3);
        // Without colours, no materials.
        let plain = three_mf_assembly("Box", &[(&s, "a", 1)]);
        assert!(!String::from_utf8_lossy(&plain).contains("basematerials"));
    }

    #[test]
    fn three_mf_prints_objects_of_one_part_and_of_several() {
        let (s, _) = holed_block();
        let z = three_mf_print(&[
            ("Lid", vec![(&s, "Lid", 1)]),
            ("Front", vec![(&s, "Front", 1), (&s, "Base", 2)]),
        ]);
        let text = String::from_utf8_lossy(&z);
        // Three meshes, one object of two of them; two built.
        assert_eq!(text.matches("<mesh>").count(), 3);
        assert_eq!(text.matches("<component objectid=").count(), 2);
        assert_eq!(text.matches("<item ").count(), 2);
        assert!(text.contains(r#"<item objectid="1" "#) && text.contains(r#"<item objectid="4" "#));
        assert!(text.contains(r#"<part id="3" subtype="normal_part">"#));
    }

    #[test]
    fn step_has_one_face_per_plane_with_its_hole() {
        let (s, f) = holed_block();
        let t = step(&s, &f, "block");
        assert!(t.starts_with("ISO-10303-21;") && t.trim_end().ends_with("END-ISO-10303-21;"));
        // 4 sides, 4 hole walls, top and bottom: 10 faces, each on a plane;
        // top and bottom have a hole.
        assert_eq!(t.matches("=FACE_SURFACE(").count(), 10, "{t}");
        assert_eq!(t.matches("=PLANE(").count(), 10);
        assert!(!t.contains("=FACE("));
        assert_eq!(t.matches("=FACE_BOUND(").count(), 2);
        assert_eq!(t.matches("CLOSED_SHELL").count(), 1);
        assert_eq!(t.matches("=FACETED_BREP(").count(), 1);
        assert!(t.contains("=FACETED_BREP_SHAPE_REPRESENTATION("));
        // A corner is written once, however many faces it is on.
        assert_eq!(t.matches("CARTESIAN_POINT").count(), 16 + 10 + 1);
    }

    /// A cube of `size` at `at` from triangles of their own planes, as a
    /// mesh read from a file is; inside out with `inward`.
    fn mesh_cube(f: &mut Facets, at: [f64; 3], size: f64, inward: bool) -> Solid {
        let points: Vec<[f64; 3]> = (0..8)
            .map(|i| {
                let c = |b: usize| at[b] + if i >> b & 1 == 1 { size } else { 0.0 };
                [c(0), c(1), c(2)]
            })
            .collect();
        let quads = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let mut tris = Vec::new();
        for [a, b, c, d] in quads {
            tris.push([a, b, c]);
            tris.push([a, c, d]);
        }
        if inward {
            for t in &mut tris {
                t.swap(1, 2);
            }
        }
        let faces = vec![FaceId(0); tris.len()];
        Solid::from_mesh(f, points, tris, &faces)
    }

    fn joined(parts: &[Solid]) -> Solid {
        let mut s = Solid::default();
        for p in parts {
            let base = s.points.len() as u32;
            s.points.extend(&p.points);
            s.tris.extend(p.tris.iter().map(|t| t.map(|i| i + base)));
            s.facets.extend(&p.facets);
        }
        s
    }

    #[test]
    fn step_makes_a_meshs_triangles_in_one_plane_one_face() {
        let mut f = Facets::default();
        let s = mesh_cube(&mut f, [0.0; 3], 10.0, false);
        assert_eq!(
            s.facets
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            12
        );
        let t = step(&s, &f, "cube");
        assert_eq!(t.matches("=FACE_SURFACE(").count(), 6, "{t}");
        assert_eq!(t.matches("=POLY_LOOP(").count(), 6);
        assert_eq!(t.matches("=FACE_BOUND(").count(), 0);
        // The top's plane faces up.
        assert!(t.contains("DIRECTION('',(0.000000000000,0.000000000000,1.000000000000))"));
    }

    #[test]
    fn step_writes_each_piece_and_a_void_in_one() {
        let mut f = Facets::default();
        let outer = mesh_cube(&mut f, [0.0; 3], 10.0, false);
        let void = mesh_cube(&mut f, [3.0; 3], 4.0, true);
        let apart = mesh_cube(&mut f, [20.0, 0.0, 0.0], 5.0, false);
        let t = step(&joined(&[outer, void, apart]), &f, "pieces");
        assert_eq!(t.matches("=CLOSED_SHELL(").count(), 3, "{t}");
        assert_eq!(t.matches("=FACETED_BREP(").count(), 1);
        assert_eq!(t.matches("=BREP_WITH_VOIDS(").count(), 1);
        assert_eq!(t.matches("=ORIENTED_CLOSED_SHELL('',*,").count(), 1);
        assert!(t.contains("=SHAPE_REPRESENTATION("));
        assert_eq!(t.matches("=FACE_SURFACE(").count(), 18);
    }
}
