//! OpenSCAD's built-in functions. Angles are degrees; what a function gets
//! that it cannot take makes `undef`, as in OpenSCAD.

use std::rc::Rc;

use crate::geom::{cos_deg, sin_deg};
use crate::value::{Value, item};

/// Arguments as given: by position, or named.
pub type Args = [(Option<Rc<str>>, Value)];

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
        "tan" => one(|x| {
            let c = cos_deg(x);
            if c == 0.0 {
                f64::INFINITY
            } else {
                sin_deg(x) / c
            }
        }),
        "asin" => one(|x| x.asin().to_degrees()),
        "acos" => one(|x| x.acos().to_degrees()),
        "atan" => one(|x| x.atan().to_degrees()),
        "atan2" => Some(match (num(args, 0), num(args, 1)) {
            (Some(y), Some(x)) => Value::Num(y.atan2(x).to_degrees()),
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
        "rands" => Some(rands(args)),
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

/// `search(match, in, num_returns_per_match = 1, index_col_num = 0)`, for
/// what is commonly asked: a value in a list (or a column of it), or each
/// character of a string in a string or list.
fn search(args: &Args) -> Value {
    let (Some(what), Some(within)) = (
        arg(args, "match_value", 0),
        arg(args, "string_or_vector", 1),
    ) else {
        return Value::Undef;
    };
    let returns = arg(args, "num_returns_per_match", 2)
        .and_then(Value::num)
        .unwrap_or(1.0) as usize;
    let col = arg(args, "index_col_num", 3)
        .and_then(Value::num)
        .unwrap_or(0.0) as usize;
    let haystack: Vec<Value> = match within {
        Value::Str(s) => s.chars().map(|c| Value::str(&c.to_string())).collect(),
        Value::List(v) => v
            .iter()
            .map(|x| match x {
                Value::List(_) => item(x, col),
                x => x.clone(),
            })
            .collect(),
        _ => return Value::Undef,
    };
    let find = |needle: &Value| -> Vec<Value> {
        let hits = haystack
            .iter()
            .enumerate()
            .filter(|(_, h)| *h == needle)
            .map(|(i, _)| Value::Num(i as f64));
        if returns == 0 {
            hits.collect()
        } else {
            hits.take(returns).collect()
        }
    };
    match what {
        Value::Num(_) => Value::list(find(what)),
        Value::Str(s) if matches!(within, Value::Str(_)) || s.chars().count() != 1 => {
            let per: Vec<Value> = s
                .chars()
                .map(|c| {
                    let hits = find(&Value::str(&c.to_string()));
                    if returns == 1 {
                        hits.into_iter()
                            .next()
                            .unwrap_or_else(|| Value::list(Vec::new()))
                    } else {
                        Value::list(hits)
                    }
                })
                .collect();
            Value::list(per)
        }
        Value::Str(_) => Value::list(find(what)),
        Value::List(needles) => Value::list(
            needles
                .iter()
                .map(|n| {
                    let hits = find(n);
                    if returns == 1 {
                        hits.into_iter()
                            .next()
                            .unwrap_or_else(|| Value::list(Vec::new()))
                    } else {
                        Value::list(hits)
                    }
                })
                .collect(),
        ),
        _ => Value::Undef,
    }
}

/// Random numbers, from a seed if one is given; not OpenSCAD's sequence.
fn rands(args: &Args) -> Value {
    let (Some(lo), Some(hi), Some(n)) = (num(args, 0), num(args, 1), num(args, 2)) else {
        return Value::Undef;
    };
    let mut state = num(args, 3).map_or(0x2545_f491_4f6c_dd1d, |s| s.to_bits() | 1);
    let n = (n.max(0.0) as usize).min(crate::value::MOST);
    let out = (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let u = (state >> 11) as f64 / (1u64 << 53) as f64;
            Value::Num(lo + (hi - lo) * u)
        })
        .collect();
    Value::list(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> (Option<Rc<str>>, Value) {
        (None, Value::Num(x))
    }

    fn v(x: &[f64]) -> (Option<Rc<str>>, Value) {
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
