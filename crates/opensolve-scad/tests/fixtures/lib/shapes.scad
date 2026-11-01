// Shapes for modules.scad, brought in by `use`: only its modules and
// functions count, not what it makes itself.

corner_fn = 16;

module rounded_plate(size, r) {
    linear_extrude(height = size.z)
        offset_square([size.x, size.y], r);
}

module offset_square(size, r) {
    hull() for (x = [r, size.x - r], y = [r, size.y - r]) translate([x, y]) circle(r, $fn = corner_fn);
}

// Made when the file is opened alone, not when used.
rounded_plate([100, 100, 100], 5);
