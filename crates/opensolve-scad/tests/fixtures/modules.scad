// Modules that pass children along, recursion and list comprehensions:
// a gear-like wheel on a stepped shaft, and a small tree of branches.

use <lib/shapes.scad>

teeth = 12; // [6:40]
module_size = 2;
shaft = [[0, 6], [8, 4], [14, 3]]; // [z, radius] steps

function pitch_radius(n, m) = n * m / 2;
function tooth_points(n, r, h) = [
    for (i = [0:n - 1], k = [0:3])
        let (a = 360 * (i + k / 4) / n, rr = (k == 1 || k == 2) ? r + h : r - h)
            [rr * cos(a), rr * sin(a)]
];

module arrayed(n, radius) {
    for (i = [0:n - 1]) rotate([0, 0, i * 360 / n]) translate([radius, 0, 0]) children();
}

module stepped(steps, i = 0) {
    if (i < len(steps)) {
        z0 = steps[i][0];
        z1 = i + 1 < len(steps) ? steps[i + 1][0] : z0 + 6;
        translate([0, 0, z0]) cylinder(r = steps[i][1], h = z1 - z0, $fn = 24);
        stepped(steps, i + 1);
    }
}

module branch(depth, size) {
    cube([size, size, size * 3]);
    if (depth > 0)
        translate([0, 0, size * 3])
            for (side = [-1, 1])
                rotate([0, side * 30, 0]) branch(depth - 1, size * 0.7);
}

r = pitch_radius(teeth, module_size);
difference() {
    linear_extrude(height = 5) polygon(tooth_points(teeth, r, module_size));
    translate([0, 0, -1]) cylinder(r = 2, h = 7, $fn = 16);
    arrayed(4, r / 2) cylinder(r = 1.5, h = 20, center = true, $fn = 12);
}
translate([40, 0, 0]) stepped(shaft);
translate([-40, 0, 0]) branch(3, 4);
translate([0, 40, 0]) rounded_plate([20, 10, 2], 2);

echo(teeth = teeth, r = r, points = len(tooth_points(teeth, r, module_size)));
