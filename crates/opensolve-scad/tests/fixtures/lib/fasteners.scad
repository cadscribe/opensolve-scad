// Holes for screws, used by bracket.scad.

// A hole of diameter `d` through a plate `t` thick, countersunk at the top.
module countersunk(d, t, $fn = 24) {
    translate([0, 0, -1]) cylinder(d = d, h = t + 2);
    translate([0, 0, t - d / 2]) cylinder(d1 = d, d2 = 2 * d, h = d / 2 + 0.01);
}
