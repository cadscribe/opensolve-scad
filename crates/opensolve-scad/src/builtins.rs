//! OpenSCAD's built-in functions. Angles are degrees; what a function gets
//! that it cannot take makes `undef`, as in OpenSCAD.

use std::sync::Arc;

use crate::geom::{cos_deg, sin_deg};
use crate::trig;
use crate::value::Value;

/// Arguments as given: by position, or named.
pub type Args = [(Option<Arc<str>>, Value)];

fn arg<'a>(args: &'a Args, name: &str, at: usize) -> Option<&'a Value> {
    args.iter()
        .find(|(n, _)| n.as_deref() == Some(name))
        .or_else(|| args.iter().filter(|(n, _)| n.is_none()).nth(at))
        .map(|(_, v)| v)
}

fn num(args: &Args, at: usize) -> Option<f64> {
    args.iter()
        .filter(|(n, _)| n.is_none())
        .nth(at)
        .and_then(|(_, v)| v.num())
}

/// The names of the built-in functions.
pub const NAMES: &[&str] = &[
    "abs",
    "sign",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "floor",
    "ceil",
    "round",
    "sqrt",
    "exp",
    "ln",
    "log",
    "pow",
    "min",
    "max",
    "norm",
    "cross",
    "len",
    "concat",
    "lookup",
    "str",
    "chr",
    "ord",
    "search",
    "rands",
    "version",
    "version_num",
    "is_undef",
    "is_bool",
    "is_num",
    "is_string",
    "is_list",
    "is_function",
    "parent_module",
];

/// Whether `name` is a built-in function.
pub fn is_builtin(name: &str) -> bool {
    NAMES.contains(&name)
}

/// The value of built-in function `name`, `None` if there is none of that
/// name.
pub fn call(name: &str, args: &Args) -> Option<Value> {
    let one = |f: fn(f64) -> f64| Some(num(args, 0).map_or(Value::Undef, |x| Value::Num(f(x))));
    match name {
        "abs" => one(f64::abs),
        "sign" => one(|x| {
            if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            }
        }),
        "sin" => one(sin_deg),
        "cos" => one(cos_deg),
        "tan" => one(trig::tan),
        "asin" => one(trig::asin),
        "acos" => one(trig::acos),
        "atan" => one(trig::atan),
        "atan2" => Some(match (num(args, 0), num(args, 1)) {
            (Some(y), Some(x)) => Value::Num(trig::atan2(y, x)),
            _ => Value::Undef,
        }),
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        // Halves away from zero, as C's round.
        "round" => one(f64::round),
        "sqrt" => one(f64::sqrt),
        "exp" => one(f64::exp),
        "ln" => one(f64::ln),
        "log" => Some(match (num(args, 0), num(args, 1)) {
            (Some(b), Some(x)) => Value::Num(x.ln() / b.ln()),
            (Some(x), None) => Value::Num(x.log10()),
            _ => Value::Undef,
        }),
        "pow" => Some(match (num(args, 0), num(args, 1)) {
            (Some(b), Some(e)) => Value::Num(b.powf(e)),
            _ => Value::Undef,
        }),
        "min" | "max" => Some(extreme(args, name == "max")),
        "norm" => Some(
            arg(args, "v", 0)
                .and_then(Value::nums)
                .map_or(Value::Undef, |v| {
                    Value::Num(v.iter().map(|x| x * x).sum::<f64>().sqrt())
                }),
        ),
        "cross" => Some(cross(args)),
        "len" => Some(match arg(args, "v", 0) {
            Some(Value::List(v)) => Value::Num(v.len() as f64),
            Some(Value::Str(s)) => Value::Num(s.chars().count() as f64),
            _ => Value::Undef,
        }),
        "concat" => {
            let mut out = Vec::new();
            for (_, v) in args {
                match v {
                    Value::List(x) => out.extend(x.iter().cloned()),
                    v => out.push(v.clone()),
                }
            }
            Some(Value::list(out))
        }
        "lookup" => Some(lookup(args)),
        "str" => Some(Value::str(
            &args.iter().map(|(_, v)| v.to_string()).collect::<String>(),
        )),
        "chr" => {
            let mut s = String::new();
            for (_, v) in args {
                chars(v, &mut s);
            }
            Some(Value::str(&s))
        }
        "ord" => Some(match arg(args, "c", 0) {
            Some(Value::Str(s)) if s.chars().count() == 1 => {
                Value::Num(f64::from(s.chars().next().map_or(0, u32::from)))
            }
            _ => Value::Undef,
        }),
        "search" => Some(search(args)),
        "version" => Some(Value::list(vec![
            Value::Num(2021.0),
            Value::Num(1.0),
            Value::Num(0.0),
        ])),
        "version_num" => Some(Value::Num(20210100.0)),
        "is_undef" => Some(Value::Bool(matches!(
            arg(args, "", 0),
            Some(Value::Undef) | None
        ))),
        "is_bool" => Some(Value::Bool(matches!(
            arg(args, "", 0),
            Some(Value::Bool(_))
        ))),
        "is_num" => Some(Value::Bool(
            matches!(arg(args, "", 0), Some(Value::Num(x)) if !x.is_nan()),
        )),
        "is_string" => Some(Value::Bool(matches!(arg(args, "", 0), Some(Value::Str(_))))),
        "is_list" => Some(Value::Bool(matches!(
            arg(args, "", 0),
            Some(Value::List(_))
        ))),
        "is_function" => Some(Value::Bool(matches!(
            arg(args, "", 0),
            Some(Value::Func(..))
        ))),
        _ => None,
    }
}

fn chars(v: &Value, s: &mut String) {
    match v {
        Value::Num(x) => {
            if let Some(c) = char::from_u32(*x as u32).filter(|_| *x >= 1.0) {
                s.push(c);
            }
        }
        Value::List(_) | Value::Range(..) => {
            for x in v.items().unwrap_or_default() {
                chars(&x, s);
            }
        }
        _ => {}
    }
}

/// `min`/`max` of numbers, or of one vector of numbers.
fn extreme(args: &Args, max: bool) -> Value {
    let values: Vec<f64> = match args {
        [(_, Value::List(v))] => match v.iter().map(Value::num).collect() {
            Some(v) => v,
            None => return Value::Undef,
        },
        _ => match args.iter().map(|(_, v)| v.num()).collect() {
            Some(v) => v,
            None => return Value::Undef,
        },
    };
    let pick = |a: f64, b: f64| if max { a.max(b) } else { a.min(b) };
    values
        .into_iter()
        .reduce(pick)
        .map_or(Value::Undef, Value::Num)
}

fn cross(args: &Args) -> Value {
    let (Some(a), Some(b)) = (
        arg(args, "", 0).and_then(Value::nums),
        arg(args, "", 1).and_then(Value::nums),
    ) else {
        return Value::Undef;
    };
    match (a.len(), b.len()) {
        (2, 2) => Value::Num(a[0] * b[1] - a[1] * b[0]),
        (3, 3) => Value::list(
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
            .map(Value::Num)
            .to_vec(),
        ),
        _ => Value::Undef,
    }
}

/// The value at `key` in a table of `[key, value]` rows, in between linear.
fn lookup(args: &Args) -> Value {
    let (Some(k), Some(Value::List(table))) = (num(args, 0), arg(args, "", 1)) else {
        return Value::Undef;
    };
    let mut rows: Vec<(f64, f64)> = table
        .iter()
        .filter_map(|r| {
            let r = r.nums()?;
            Some((*r.first()?, *r.get(1)?))
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return Value::Undef;
    };
    if k <= first.0 {
        return Value::Num(first.1);
    }
    if k >= last.0 {
        return Value::Num(last.1);
    }
    let at = rows.windows(2).find(|w| w[0].0 <= k && k <= w[1].0);
    match at {
        Some([lo, hi]) if hi.0 > lo.0 => {
            Value::Num(lo.1 + (k - lo.0) / (hi.0 - lo.0) * (hi.1 - lo.1))
        }
        Some([lo, _]) => Value::Num(lo.1),
        _ => Value::Undef,
    }
}

/// `search(match, in, num_returns_per_match = 1, index_col_num = 0)`, as
/// OpenSCAD's: a number, each element of a vector, or each character of a
/// string looked for in a list (an element, or its column) or a string.
fn search(args: &Args) -> Value {
    let (Some(what), Some(within)) = (
        arg(args, "match_value", 0),
        arg(args, "string_or_vector", 1),
    ) else {
        return Value::Undef;
    };
    let returns = arg(args, "num_returns_per_match", 2)
        .and_then(Value::num)
        .unwrap_or(1.0)
        .max(0.0) as usize;
    let col = arg(args, "index_col_num", 3)
        .and_then(Value::num)
        .unwrap_or(0.0)
        .max(0.0) as usize;
    let table: &[Value] = match within {
        Value::List(v) => v,
        _ => &[],
    };
    let hit = |find: &Value, element: &Value| {
        (col == 0 && find == element)
            || matches!(element, Value::List(e) if e.get(col).is_some_and(|x| x == find))
    };
    // The places where `is` holds, as many as asked for (0: all).
    let places = |is: &dyn Fn(usize) -> bool, n: usize| -> Vec<Value> {
        let mut found = Vec::new();
        for j in (0..n).filter(|j| is(*j)) {
            found.push(Value::Num(j as f64));
            if returns != 0 && found.len() >= returns {
                break;
            }
        }
        found
    };
    match what {
        Value::Num(_) | Value::Bool(_) | Value::Undef => {
            Value::list(places(&|j| hit(what, &table[j]), table.len()))
        }
        Value::Str(s) => {
            let glyphs: Vec<Option<char>> = match within {
                Value::Str(t) => t.chars().map(Some).collect(),
                _ => table
                    .iter()
                    .map(|e| match e {
                        Value::List(e) => e.get(col).and_then(|x| x.to_string().chars().next()),
                        _ => None,
                    })
                    .collect(),
            };
            let mut out = Vec::new();
            for c in s.chars() {
                let found = places(&|j| glyphs[j] == Some(c), glyphs.len());
                // One asked for and none found: nothing for this character.
                if returns == 1 {
                    out.extend(found);
                } else {
                    out.push(Value::list(found));
                }
            }
            Value::list(out)
        }
        Value::List(needles) => {
            let mut out = Vec::new();
            for n in needles.iter() {
                let found = places(&|j| hit(n, &table[j]), table.len());
                match (returns, found.first()) {
                    (1, Some(first)) => out.push(first.clone()),
                    _ => out.push(Value::list(found)),
                }
            }
            Value::list(out)
        }
        _ => Value::Undef,
    }
}

/// The Mersenne Twister of C++'s `std::mt19937`, which OpenSCAD's `rands`
/// draws from: with a seed, the same numbers as OpenSCAD.
#[derive(Clone, Debug)]
pub struct Mt19937 {
    state: [u32; 624],
    at: usize,
}

impl Mt19937 {
    pub fn new(seed: u32) -> Mt19937 {
        let mut state = [0u32; 624];
        state[0] = seed;
        for i in 1..624 {
            let prev = state[i - 1];
            state[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        Mt19937 { state, at: 624 }
    }

    pub fn next_u32(&mut self) -> u32 {
        if self.at >= 624 {
            for i in 0..624 {
                let y = (self.state[i] & 0x8000_0000) | (self.state[(i + 1) % 624] & 0x7fff_ffff);
                let mut v = self.state[(i + 397) % 624] ^ (y >> 1);
                if y & 1 != 0 {
                    v ^= 0x9908_b0df;
                }
                self.state[i] = v;
            }
            self.at = 0;
        }
        let mut y = self.state[self.at];
        self.at += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `std::uniform_real_distribution<double>(lo, hi)`: two draws make
    /// 53 bits (`generate_canonical`).
    pub fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        let a = f64::from(self.next_u32());
        let b = f64::from(self.next_u32());
        let r = 4_294_967_296.0;
        let mut u = (a + b * r) / (r * r);
        if u >= 1.0 {
            u = 1.0 - f64::EPSILON / 2.0;
        }
        lo + (hi - lo) * u
    }
}

/// `rands(min, max, count, seed)`: with a seed, `rng` is seeded anew, as
/// OpenSCAD does.
pub fn rands(args: &Args, rng: &mut Mt19937) -> Value {
    let (Some(lo), Some(hi), Some(n)) = (num(args, 0), num(args, 1), num(args, 2)) else {
        return Value::Undef;
    };
    if let Some(seed) = num(args, 3) {
        *rng = Mt19937::new(seed as i64 as u32);
    }
    let (lo, hi) = if hi < lo { (hi, lo) } else { (lo, hi) };
    let n = (n.round().max(0.0) as usize).min(crate::value::MOST);
    Value::list((0..n).map(|_| Value::Num(rng.uniform(lo, hi))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> (Option<Arc<str>>, Value) {
        (None, Value::Num(x))
    }

    fn v(x: &[f64]) -> (Option<Arc<str>>, Value) {
        (
            None,
            Value::list(x.iter().map(|x| Value::Num(*x)).collect()),
        )
    }

    #[test]
    fn math_in_degrees() {
        assert_eq!(call("sin", &[n(30.0)]), Some(Value::Num(0.5)));
        assert_eq!(call("atan2", &[n(1.0), n(1.0)]), Some(Value::Num(45.0)));
        assert_eq!(call("round", &[n(-2.5)]), Some(Value::Num(-3.0)));
        assert_eq!(call("max", &[v(&[1.0, 7.0, 3.0])]), Some(Value::Num(7.0)));
        assert_eq!(call("min", &[n(4.0), n(2.0)]), Some(Value::Num(2.0)));
        assert_eq!(call("norm", &[v(&[3.0, 4.0])]), Some(Value::Num(5.0)));
        assert_eq!(call("log", &[n(1000.0)]), Some(Value::Num(3.0)));
        assert_eq!(call("nothing", &[]), None);
    }

    #[test]
    fn lists_and_strings() {
        let table = (
            None,
            Value::list(vec![
                v(&[0.0, 0.0]).1,
                v(&[10.0, 100.0]).1,
                v(&[20.0, 0.0]).1,
            ]),
        );
        assert_eq!(
            call("lookup", &[n(5.0), table.clone()]),
            Some(Value::Num(50.0))
        );
        assert_eq!(call("lookup", &[n(30.0), table]), Some(Value::Num(0.0)));
        let s = (None, Value::str("a"));
        assert_eq!(
            call("str", &[s, n(1.5), v(&[1.0, 2.0])]),
            Some(Value::str("a1.5[1, 2]"))
        );
        assert_eq!(call("chr", &[v(&[72.0, 105.0])]), Some(Value::str("Hi")));
        assert_eq!(
            call("ord", &[(None, Value::str("A"))]),
            Some(Value::Num(65.0))
        );
        assert_eq!(
            call("search", &[n(3.0), v(&[1.0, 3.0, 3.0])]),
            Some(Value::list(vec![Value::Num(1.0)]))
        );
        assert_eq!(call("len", &[v(&[1.0, 2.0])]), Some(Value::Num(2.0)));
        assert_eq!(
            call("concat", &[v(&[1.0]), n(2.0)]),
            Some(Value::list(vec![Value::Num(1.0), Value::Num(2.0)]))
        );
    }
}
