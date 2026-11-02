//! OpenSCAD's own modules, functions, special variables and keywords: what
//! completion offers and hover explains. The texts follow OpenSCAD's
//! manual; `supported` says whether OpenSolve builds it (ADR 0011).

/// What a built-in name is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinKind {
    Module,
    Function,
    /// `$fn`, `PI`, …
    Variable,
    Keyword,
}

/// A built-in name.
#[derive(Clone, Copy, Debug)]
pub struct Builtin {
    pub name: &'static str,
    pub kind: BuiltinKind,
    /// How it is called, with its parameters.
    pub signature: &'static str,
    pub doc: &'static str,
    /// Whether OpenSolve builds it; what it does not is noted when a file
    /// uses it.
    pub supported: bool,
}

const fn m(name: &'static str, signature: &'static str, doc: &'static str) -> Builtin {
    Builtin {
        name,
        kind: BuiltinKind::Module,
        signature,
        doc,
        supported: true,
    }
}

const fn f(name: &'static str, signature: &'static str, doc: &'static str) -> Builtin {
    Builtin {
        name,
        kind: BuiltinKind::Function,
        signature,
        doc,
        supported: true,
    }
}

const fn v(name: &'static str, doc: &'static str) -> Builtin {
    Builtin {
        name,
        kind: BuiltinKind::Variable,
        signature: name,
        doc,
        supported: true,
    }
}

const fn k(name: &'static str, signature: &'static str, doc: &'static str) -> Builtin {
    Builtin {
        name,
        kind: BuiltinKind::Keyword,
        signature,
        doc,
        supported: true,
    }
}

const fn no(b: Builtin) -> Builtin {
    Builtin {
        supported: false,
        ..b
    }
}

/// Every built-in name.
pub const BUILTINS: &[Builtin] = &[
    // 3D shapes
    m(
        "cube",
        "cube(size = [1, 1, 1], center = false)",
        "A box, `size` long in x, y and z (one number for all three); at the origin's corner, or around it with `center`.",
    ),
    m(
        "sphere",
        "sphere(r = 1, d, $fn, $fa, $fs)",
        "A sphere of radius `r` (or diameter `d`) around the origin, cut into pieces by `$fn`, `$fa` and `$fs`.",
    ),
    m(
        "cylinder",
        "cylinder(h = 1, r1 = 1, r2 = 1, center = false, r, d, d1, d2, $fn, $fa, $fs)",
        "A cylinder or cone along z, `h` high, radius `r1` at the bottom and `r2` at the top (or `r`, `d`, `d1`, `d2`).",
    ),
    m(
        "polyhedron",
        "polyhedron(points, faces, convexity = 1)",
        "A solid of `points` and `faces`, each face a list of point indices, clockwise seen from outside.",
    ),
    // 2D shapes
    m(
        "square",
        "square(size = [1, 1], center = false)",
        "A rectangle, `size` long in x and y (one number for both).",
    ),
    m(
        "circle",
        "circle(r = 1, d, $fn, $fa, $fs)",
        "A circle of radius `r` (or diameter `d`), cut into pieces by `$fn`, `$fa` and `$fs`.",
    ),
    m(
        "polygon",
        "polygon(points, paths, convexity = 1)",
        "A 2D shape of `points`; `paths` lists the outline and holes as point indices.",
    ),
    no(m(
        "text",
        "text(text, size = 10, font, halign, valign, spacing, direction, language, script, $fn)",
        "Text as a 2D shape. Not supported by OpenSolve yet.",
    )),
    no(m(
        "import",
        "import(file, convexity, layer)",
        "A shape read from an STL, OFF, DXF or SVG file. Not supported by OpenSolve yet.",
    )),
    no(m(
        "surface",
        "surface(file, center = false, invert = false, convexity)",
        "A height map read from a file. Not supported by OpenSolve yet.",
    )),
    no(m(
        "projection",
        "projection(cut = false)",
        "The 2D shadow of its 3D children on the xy plane. Not supported by OpenSolve yet.",
    )),
    no(m(
        "roof",
        "roof(method = \"voronoi\")",
        "A roof over a 2D shape (experimental in OpenSCAD). Not supported by OpenSolve.",
    )),
    no(m(
        "fill",
        "fill()",
        "Its 2D children with their holes filled (experimental in OpenSCAD). Not supported by OpenSolve.",
    )),
    // Transformations
    m(
        "translate",
        "translate(v)",
        "Moves its children by vector `v`.",
    ),
    m(
        "rotate",
        "rotate(a, v)",
        "Turns its children by `a` degrees: about x, then y, then z for a vector `a`; about axis `v` for a number.",
    ),
    m(
        "scale",
        "scale(v)",
        "Scales its children by `v` (one number for all axes).",
    ),
    m(
        "resize",
        "resize(newsize, auto = false)",
        "Scales its children to the size `newsize`; a 0 keeps that axis, or scales it with the others where `auto` says so.",
    ),
    m(
        "mirror",
        "mirror(v)",
        "Mirrors its children at the plane through the origin with normal `v`.",
    ),
    m(
        "multmatrix",
        "multmatrix(m)",
        "Transforms its children by the 4×4 (or 3×4) matrix `m`.",
    ),
    m(
        "color",
        "color(c, alpha = 1.0)",
        "Colours its children. OpenSolve keeps the shape and drops the colour: colours are per part.",
    ),
    no(m(
        "offset",
        "offset(r, delta, chamfer = false)",
        "Grows or shrinks a 2D shape by `r` (rounded) or `delta` (sharp). Not supported by OpenSolve yet.",
    )),
    m("hull", "hull()", "The convex hull of its children."),
    no(m(
        "minkowski",
        "minkowski(convexity)",
        "The Minkowski sum of its children. Not supported by OpenSolve yet.",
    )),
    // Booleans
    m("union", "union()", "Its children joined into one."),
    m(
        "difference",
        "difference()",
        "Its first child with all others cut away.",
    ),
    m(
        "intersection",
        "intersection()",
        "What all its children have in common.",
    ),
    m(
        "render",
        "render(convexity)",
        "Its children, rendered at once in OpenSCAD's preview; the same as `union` here.",
    ),
    // Extrusions
    m(
        "linear_extrude",
        "linear_extrude(height = 100, center = false, convexity, twist = 0, slices, scale = 1.0, $fn)",
        "Its 2D children extruded along z, `height` high; `twist` turns them by degrees on the way, `scale` scales the top.",
    ),
    m(
        "rotate_extrude",
        "rotate_extrude(angle = 360, convexity, $fn)",
        "Its 2D children (at x ≥ 0) turned about the z axis by `angle` degrees.",
    ),
    // Other modules
    m(
        "children",
        "children(index)",
        "In a module: the children it was called with, all or the one (or those) of `index`.",
    ),
    m(
        "echo",
        "echo(…)",
        "Writes its arguments to the console, as `ECHO: …`.",
    ),
    m(
        "assert",
        "assert(condition, message)",
        "Ends the evaluation with an error, and `message`, when `condition` is false.",
    ),
    m(
        "group",
        "group()",
        "Its children as one, as `union` (used in OpenSCAD's CSG files).",
    ),
    // Functions
    f("abs", "abs(x)", "The absolute value of `x`."),
    f("sign", "sign(x)", "-1, 0 or 1: the sign of `x`."),
    f("sin", "sin(degrees)", "The sine of an angle in degrees."),
    f("cos", "cos(degrees)", "The cosine of an angle in degrees."),
    f("tan", "tan(degrees)", "The tangent of an angle in degrees."),
    f("asin", "asin(x)", "The arc sine of `x`, in degrees."),
    f("acos", "acos(x)", "The arc cosine of `x`, in degrees."),
    f("atan", "atan(x)", "The arc tangent of `x`, in degrees."),
    f(
        "atan2",
        "atan2(y, x)",
        "The angle of the point (`x`, `y`) from the x axis, in degrees.",
    ),
    f(
        "floor",
        "floor(x)",
        "The largest whole number not above `x`.",
    ),
    f(
        "ceil",
        "ceil(x)",
        "The smallest whole number not below `x`.",
    ),
    f(
        "round",
        "round(x)",
        "`x` rounded to a whole number, halves away from zero.",
    ),
    f("sqrt", "sqrt(x)", "The square root of `x`."),
    f("exp", "exp(x)", "e to the power of `x`."),
    f("ln", "ln(x)", "The natural logarithm of `x`."),
    f(
        "log",
        "log(x) / log(base, x)",
        "The logarithm of `x` to base 10, or to `base`.",
    ),
    f(
        "pow",
        "pow(base, exponent)",
        "`base` to the power of `exponent`.",
    ),
    f(
        "min",
        "min(values…) / min(list)",
        "The smallest of its numbers, or of a list's.",
    ),
    f(
        "max",
        "max(values…) / max(list)",
        "The largest of its numbers, or of a list's.",
    ),
    f("norm", "norm(v)", "The length of vector `v`."),
    f(
        "cross",
        "cross(a, b)",
        "The cross product of two vectors in 2D or 3D.",
    ),
    f(
        "len",
        "len(value)",
        "The number of elements of a list, or of characters of a string.",
    ),
    f(
        "concat",
        "concat(lists…)",
        "The lists one after the other as one list; other values are taken as elements.",
    ),
    f(
        "lookup",
        "lookup(key, table)",
        "The value for `key` in a table of [key, value] pairs, interpolated between neighbours.",
    ),
    f("str", "str(values…)", "Its values as one string."),
    f(
        "chr",
        "chr(code…)",
        "The characters of Unicode code points, as a string.",
    ),
    f(
        "ord",
        "ord(character)",
        "The Unicode code point of a character.",
    ),
    f(
        "search",
        "search(match, in, num_returns_per_match = 1, index_col_num = 0)",
        "Where values occur in a string or list: their indices.",
    ),
    f(
        "rands",
        "rands(min, max, count, seed)",
        "`count` random numbers between `min` and `max`. OpenSolve's differ from OpenSCAD's.",
    ),
    f(
        "version",
        "version()",
        "The OpenSCAD version as [year, month, day]: 2021.01 here.",
    ),
    f(
        "version_num",
        "version_num()",
        "The OpenSCAD version as a number: 20210100 here.",
    ),
    f("is_undef", "is_undef(x)", "Whether `x` is `undef`."),
    f("is_bool", "is_bool(x)", "Whether `x` is `true` or `false`."),
    f("is_num", "is_num(x)", "Whether `x` is a number (not NaN)."),
    f("is_string", "is_string(x)", "Whether `x` is a string."),
    f("is_list", "is_list(x)", "Whether `x` is a list."),
    f(
        "is_function",
        "is_function(x)",
        "Whether `x` is a function literal.",
    ),
    // Special variables and constants
    v(
        "$fn",
        "The number of pieces of a circle; 0 leaves it to `$fa` and `$fs`.",
    ),
    v(
        "$fa",
        "The smallest angle of a circle's piece, in degrees (default 12).",
    ),
    v(
        "$fs",
        "The smallest length of a circle's piece (default 2).",
    ),
    v("$t", "The animation's time, from 0 to 1 (0 here)."),
    v("$children", "In a module: the number of its children."),
    v(
        "$preview",
        "Whether OpenSCAD shows a preview: false here, as when rendering.",
    ),
    v("PI", "π, 3.14159…"),
    // Keywords
    k(
        "module",
        "module name(parameters) { … }",
        "Defines a module: shapes made from its parameters and children.",
    ),
    k(
        "function",
        "function name(parameters) = expression;",
        "Defines a function: a value made from its parameters.",
    ),
    k(
        "include",
        "include <file.scad>",
        "Puts the whole file here, as if written in its place.",
    ),
    k(
        "use",
        "use <file.scad>",
        "Brings the file's modules and functions, not its variables or shapes.",
    ),
    k(
        "if",
        "if (condition) … else …",
        "Makes one branch or the other.",
    ),
    k(
        "else",
        "else …",
        "The branch of `if` when the condition is false.",
    ),
    k(
        "for",
        "for (i = [start : step : end]) …",
        "Its body for each value; the shapes of all are joined.",
    ),
    k(
        "intersection_for",
        "intersection_for (i = …) …",
        "Its body for each value; what all have in common.",
    ),
    k(
        "let",
        "let (name = value, …) …",
        "Names values for the expression or statement after it.",
    ),
    k(
        "each",
        "each list",
        "In a list: the elements of a list, each on its own.",
    ),
    k("true", "true", "Yes."),
    k("false", "false", "No."),
    k("undef", "undef", "No value."),
];

/// The built-in name `name`.
pub fn builtin(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

/// The parameter names of a signature as written: `cube(size = 1, center)`
/// gives `size`, `center`.
pub fn parameters(signature: &str) -> Vec<String> {
    let Some(open) = signature.find('(') else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in signature[open + 1..].chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 0 => depth -= 1,
            ')' => break,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    out.push(current);
    out.into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}
