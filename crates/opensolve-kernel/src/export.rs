//! The solid for other programs: 3MF for slicers, STEP for CAD. Both are
//! written by hand, in millimetres.
//!
//! STEP holds the mesh as a faceted boundary representation (AP214, a
//! `FACETED_BREP` of one `CLOSED_SHELL`): every planar face of the part is one
//! face, so a CAD program sees flat faces as flat, and round ones as the
//! facets they are printed with.

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
    let mut mesh = String::new();
    mesh.push_str("<vertices>\n");
    for p in &solid.points {
        let _ = writeln!(mesh, r#"<vertex x="{}" y="{}" z="{}"/>"#, p[0], p[1], p[2]);
    }
    mesh.push_str("</vertices>\n<triangles>\n");
    for t in &solid.tris {
        let _ = writeln!(
            mesh,
            r#"<triangle v1="{}" v2="{}" v3="{}"/>"#,
            t[0], t[1], t[2]
        );
    }
    mesh.push_str("</triangles>\n");
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <model unit=\"millimeter\" xml:lang=\"en-US\" \
         xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n\
         <resources>\n<object id=\"1\" type=\"model\" name=\"{}\">\n<mesh>\n{mesh}</mesh>\n</object>\n\
         </resources>\n<build>\n<item objectid=\"1\"/>\n</build>\n</model>\n",
        xml_text(name)
    );
    let mut files = BTreeMap::new();
    files.insert(
        "[Content_Types].xml".to_string(),
        CONTENT_TYPES.as_bytes().to_vec(),
    );
    files.insert("_rels/.rels".to_string(), RELATIONSHIPS.as_bytes().to_vec());
    files.insert("3D/3dmodel.model".to_string(), xml.into_bytes());
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

/// The solid as STEP (ISO 10303-21, AP214): a faceted boundary
/// representation, one face per facet of the part with its outer loop and holes.
pub fn step(solid: &Solid, facets: &Facets, name: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut add = |l: String| {
        lines.push(l);
        lines.len()
    };
    let mut points = HashMap::new();
    let mut point = |i: u32, add: &mut dyn FnMut(String) -> usize| -> usize {
        *points.entry(i).or_insert_with(|| {
            let p = solid.points[i as usize];
            add(format!(
                "CARTESIAN_POINT('',({:.6},{:.6},{:.6}))",
                p[0], p[1], p[2]
            ))
        })
    };
    // Triangles grouped by the plane they lie in.
    let mut by_facet: BTreeMap<u32, Vec<[u32; 3]>> = BTreeMap::new();
    for (t, &f) in solid.tris.iter().zip(&solid.facets) {
        by_facet.entry(f).or_default().push(*t);
    }
    let mut faces = Vec::new();
    for tris in by_facet.values() {
        let mut bounds = Vec::new();
        for (k, ring) in loops(tris).into_iter().enumerate() {
            let ids: Vec<String> = ring
                .iter()
                .map(|&i| format!("#{}", point(i, &mut add)))
                .collect();
            let poly = add(format!("POLY_LOOP('',({}))", ids.join(",")));
            let kind = if k == 0 {
                "FACE_OUTER_BOUND"
            } else {
                "FACE_BOUND"
            };
            bounds.push(add(format!("{kind}('',#{poly},.T.)")));
        }
        if bounds.is_empty() {
            continue;
        }
        let list: Vec<String> = bounds.iter().map(|b| format!("#{b}")).collect();
        faces.push(add(format!("FACE('',({}))", list.join(","))));
    }
    let _ = facets;
    let face_list: Vec<String> = faces.iter().map(|f| format!("#{f}")).collect();
    let shell = add(format!("CLOSED_SHELL('',({}))", face_list.join(",")));
    let brep = add(format!(
        "FACETED_BREP('{}',#{shell})",
        name.replace('\'', "")
    ));
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
    let rep = add(format!(
        "FACETED_BREP_SHAPE_REPRESENTATION('',(#{axis},#{brep}),#{ctx})"
    ));
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
    fn step_has_one_face_per_plane_with_its_hole() {
        let (s, f) = holed_block();
        let t = step(&s, &f, "block");
        assert!(t.starts_with("ISO-10303-21;") && t.trim_end().ends_with("END-ISO-10303-21;"));
        // 4 sides, 4 hole walls, top and bottom: 10 faces; top and bottom
        // have a hole.
        assert_eq!(t.matches("=FACE(").count(), 10, "{t}");
        assert_eq!(t.matches("=FACE_BOUND(").count(), 2);
        assert_eq!(t.matches("CLOSED_SHELL").count(), 1);
    }
}
