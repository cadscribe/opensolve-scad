// A parametric box with a lid that sits beside it, the way the Customizer
// files on Thingiverse are written.

/* [Box] */
// Inside width
width = 60; // [20:200]
// Inside depth
depth = 40; // [20:200]
// Inside height
height = 25; // [10:100]
// Wall thickness
wall = 2; // [1:0.5:5]
// Corner radius
radius = 4; // [0:10]

/* [Lid] */
// Print the lid too
lid = true;
// Gap between lid and box
gap = 0.3; // [0.1, 0.2, 0.3, 0.4]
lid_height = 6;

/* [Hidden] */
$fn = 32;
eps = 0.01;

module rounded_box(size, r) {
    if (r <= 0) {
        cube(size);
    } else {
        hull() {
            for (x = [r, size.x - r], y = [r, size.y - r])
                translate([x, y, 0]) cylinder(r = r, h = size.z);
        }
    }
}

module box() {
    outer = [width + 2 * wall, depth + 2 * wall, height + wall];
    difference() {
        rounded_box(outer, radius);
        translate([wall, wall, wall])
            rounded_box([width, depth, height + eps], max(radius - wall, 0));
    }
}

module lid() {
    outer = [width + 2 * wall, depth + 2 * wall, wall];
    rounded_box(outer, radius);
    // A rim that fits into the box.
    translate([wall + gap, wall + gap, wall - eps])
        difference() {
            rounded_box([width - 2 * gap, depth - 2 * gap, lid_height], max(radius - wall - gap, 0));
            translate([wall, wall, -eps])
                rounded_box([width - 2 * gap - 2 * wall, depth - 2 * gap - 2 * wall, lid_height + 2 * eps],
                            max(radius - 2 * wall - gap, 0));
        }
}

box();
if (lid)
    translate([0, depth + 2 * wall + 10, 0]) lid();

echo(str("box ", width, " x ", depth, " x ", height));
