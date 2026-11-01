// An angle bracket with screw holes and a gusset.

thickness = 4;
length = 50;
width = 30;
hole_d = 5.5; // [3:0.5:10]
holes = 2; // [1:4]
fillet = true;

include <lib/fasteners.scad>

module plate() {
    difference() {
        cube([length, width, thickness]);
        for (i = [1:holes])
            translate([length * i / (holes + 1), width / 2, 0])
                countersunk(hole_d, thickness);
    }
}

module gusset() {
    translate([thickness, width / 2 + thickness / 2, thickness])
        rotate([90, 0, 0])
            linear_extrude(height = thickness)
                polygon([[0, 0], [length / 3, 0], [0, length / 3]]);
}

union() {
    plate();
    // The upright leg: the plate turned up about the Y axis.
    translate([thickness, 0, 0]) rotate([0, -90, 0]) plate();
    if (fillet) gusset();
}
