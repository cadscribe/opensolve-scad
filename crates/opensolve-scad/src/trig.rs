//! OpenSCAD's trigonometry in degrees, as its `degree_trig.cc` computes
//! it: exact at multiples of 30° and 45°, and the inverse functions
//! exact where a whole number of degrees is the answer. Libraries count
//! on it: BOSL2 cuts an arc into `ceil(angle / step)` pieces, and an angle
//! of 90.00000000000001° is one piece more.

#![allow(clippy::float_cmp, clippy::excessive_precision)]

const RAD2DEG: f64 = 57.295_779_513_082_320_876_7;
const DEG2RAD: f64 = 0.017_453_292_519_943_295_769;
const SQRT3: f64 = 1.732_050_807_568_877_193_18;
const SQRT3_4: f64 = 0.866_025_403_784_438_596_59;
const SQRT1_3: f64 = 0.577_350_269_189_625_731_06;
const HUGE: f64 = (1u64 << 26) as f64 * 360.0 * (1u64 << 26) as f64;

/// `x` brought into [0, 360), NaN if it is too large to say.
fn turn(x: f64) -> Option<f64> {
    if (0.0..360.0).contains(&x) {
        return Some(x);
    }
    if x < HUGE && x > -HUGE {
        return Some(x - 360.0 * (x / 360.0).floor());
    }
    None
}

pub fn sin(x: f64) -> f64 {
    let Some(mut x) = turn(x) else {
        return f64::NAN;
    };
    let oppose = x >= 180.0;
    if oppose {
        x -= 180.0;
    }
    if x > 90.0 {
        x = 180.0 - x;
    }
    let s = if x < 45.0 {
        if x == 30.0 { 0.5 } else { (x * DEG2RAD).sin() }
    } else if x == 45.0 {
        std::f64::consts::FRAC_1_SQRT_2
    } else if x == 60.0 {
        SQRT3_4
    } else {
        ((90.0 - x) * DEG2RAD).cos()
    };
    if oppose { -s } else { s }
}

pub fn cos(x: f64) -> f64 {
    let Some(mut x) = turn(x) else {
        return f64::NAN;
    };
    let mut oppose = x >= 180.0;
    if oppose {
        x -= 180.0;
    }
    if x > 90.0 {
        x = 180.0 - x;
        oppose = !oppose;
    }
    let c = if x > 45.0 {
        if x == 60.0 {
            0.5
        } else {
            ((90.0 - x) * DEG2RAD).sin()
        }
    } else if x == 45.0 {
        std::f64::consts::FRAC_1_SQRT_2
    } else if x == 30.0 {
        SQRT3_4
    } else {
        (x * DEG2RAD).cos()
    };
    if oppose { -c } else { c }
}

pub fn tan(x: f64) -> f64 {
    let cycles = (x / 180.0).floor();
    let mut x = if (0.0..180.0).contains(&x) {
        x
    } else if x < HUGE && x > -HUGE {
        x - 180.0 * cycles
    } else {
        return f64::NAN;
    };
    let odd = cycles.rem_euclid(2.0) != 0.0;
    let oppose = x > 90.0;
    if oppose {
        x = 180.0 - x;
    }
    let t = if x == 0.0 {
        if odd { -0.0 } else { 0.0 }
    } else if x == 30.0 {
        SQRT1_3
    } else if x == 45.0 {
        1.0
    } else if x == 60.0 {
        SQRT3
    } else if x == 90.0 {
        if odd {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        }
    } else {
        (x * DEG2RAD).tan()
    };
    if oppose { -t } else { t }
}

pub fn asin(x: f64) -> f64 {
    let d = x.asin() * RAD2DEG;
    let whole = d.round();
    if sin(whole) == x { whole } else { d }
}

pub fn acos(x: f64) -> f64 {
    let d = x.acos() * RAD2DEG;
    let whole = d.round();
    if cos(whole) == x { whole } else { d }
}

pub fn atan(x: f64) -> f64 {
    let d = x.atan() * RAD2DEG;
    let whole = d.round();
    if tan(whole) == x { whole } else { d }
}

pub fn atan2(y: f64, x: f64) -> f64 {
    let d = y.atan2(x) * RAD2DEG;
    let whole = d.round();
    if (d - whole).abs() < 3.0e-14 {
        whole
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_where_openscad_is() {
        assert_eq!(acos(-0.5), 120.0);
        assert_eq!(asin(0.5), 30.0);
        assert_eq!(atan(1.0), 45.0);
        assert_eq!(atan2(1.0, 1.0), 45.0);
        assert_eq!(cos(60.0), 0.5);
        assert_eq!(sin(-150.0), -0.5);
        assert_eq!(tan(45.0), 1.0);
        assert_eq!(tan(90.0), f64::INFINITY);
        assert!((asin(0.3) - 17.457_603_123_722_1).abs() < 1e-9);
    }
}
