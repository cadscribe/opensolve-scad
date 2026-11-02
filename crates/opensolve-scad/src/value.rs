//! OpenSCAD's values and their operators: what a wrong operand makes is
//! `undef`, as in OpenSCAD.

use std::fmt::Write as _;
use std::rc::Rc;

use crate::ast::Func;
use crate::eval::Frame;

#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    Undef,
    Bool(bool),
    Num(f64),
    Str(Rc<str>),
    List(Rc<Vec<Value>>),
    /// begin, step, end.
    Range(f64, f64, f64),
    /// A function literal and where it was made.
    Func(Rc<Func>, Rc<Frame>),
}

impl PartialEq for Value {
    fn eq(&self, o: &Value) -> bool {
        match (self, o) {
            (Value::Undef, Value::Undef) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Num(a), Value::Num(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            // As OpenSCAD compares ranges: a NaN in one is the same.
            (Value::Range(a, b, c), Value::Range(d, e, f)) => [a, b, c]
                .iter()
                .zip([d, e, f])
                .all(|(x, y)| x == &y || (x.is_nan() && y.is_nan())),
            (Value::Func(a, _), Value::Func(b, _)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// The most values a range or list may have.
pub const MOST: usize = 1_000_000;

impl Value {
    pub fn list(v: Vec<Value>) -> Value {
        Value::List(Rc::new(v))
    }

    pub fn str(s: &str) -> Value {
        Value::Str(s.into())
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Undef => false,
            Value::Bool(b) => *b,
            Value::Num(x) => *x != 0.0 && !x.is_nan(),
            Value::Str(s) => !s.is_empty(),
            Value::List(v) => !v.is_empty(),
            Value::Range(..) | Value::Func(..) => true,
        }
    }

    pub fn num(&self) -> Option<f64> {
        match self {
            Value::Num(x) => Some(*x),
            _ => None,
        }
    }

    /// A vector of numbers, as long as it is.
    pub fn nums(&self) -> Option<Vec<f64>> {
        match self {
            Value::List(v) => v.iter().map(Value::num).collect(),
            _ => None,
        }
    }

    /// A vector of two or three numbers as a point; z is 0 for two.
    pub fn point(&self) -> Option<[f64; 3]> {
        let v = self.nums()?;
        match v.len() {
            2 => Some([v[0], v[1], 0.0]),
            n if n >= 3 => Some([v[0], v[1], v[2]]),
            _ => None,
        }
    }

    /// The values a `for` runs over: those of a list or range, one value
    /// for anything else.
    pub fn items(&self) -> Result<Vec<Value>, String> {
        match self {
            Value::List(v) => Ok(v.to_vec()),
            Value::Range(b, s, e) => {
                let n = range_len(*b, *s, *e);
                if n > MOST {
                    return Err(format!("a range of {n} values is too long"));
                }
                Ok((0..n).map(|i| Value::Num(b + s * i as f64)).collect())
            }
            Value::Str(s) => Ok(s.chars().map(|c| Value::str(&c.to_string())).collect()),
            Value::Undef => Ok(Vec::new()),
            v => Ok(vec![v.clone()]),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Undef => "undef",
            Value::Bool(_) => "bool",
            Value::Num(_) => "number",
            Value::Str(_) => "string",
            Value::List(_) => "vector",
            Value::Range(..) => "range",
            Value::Func(..) => "function",
        }
    }
}

/// How many values a range has, as OpenSCAD counts them.
pub fn range_len(begin: f64, step: f64, end: f64) -> usize {
    if begin.is_nan() || end.is_nan() || step.is_nan() {
        return 0;
    }
    if (step < 0.0 && begin < end) || (step >= 0.0 && begin > end) {
        return 0;
    }
    if begin.total_cmp(&end).is_eq() || step.is_infinite() {
        return 1;
    }
    if begin.is_infinite() || end.is_infinite() || step == 0.0 {
        return usize::MAX;
    }
    let steps = (end - begin) / step;
    if steps >= MOST as f64 {
        usize::MAX
    } else {
        steps as usize + 1
    }
}

/// A number as OpenSCAD writes it: six significant digits, as `%g`.
pub fn number(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let sci = format!("{x:.5e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let trim = |s: &str| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s.to_string()
        }
    };
    if !(-5..6).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mantissa), exp.abs())
    } else {
        trim(&format!("{x:.*}", (5 - exp).max(0) as usize))
    }
}

impl std::fmt::Display for Value {
    /// As `str()` makes it: strings without quotes at the top, with them
    /// inside vectors.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Str(s) => f.write_str(s),
            v => f.write_str(&repr(v)),
        }
    }
}

/// A value as `echo` shows it.
pub fn repr(v: &Value) -> String {
    match v {
        Value::Undef => "undef".into(),
        Value::Bool(b) => b.to_string(),
        Value::Num(x) => number(*x),
        Value::Str(s) => {
            let mut o = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => o.push_str("\\\""),
                    '\\' => o.push_str("\\\\"),
                    '\n' => o.push_str("\\n"),
                    '\t' => o.push_str("\\t"),
                    c => o.push(c),
                }
            }
            o.push('"');
            o
        }
        Value::List(items) => {
            let mut o = String::from("[");
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                o.push_str(&repr(x));
            }
            o.push(']');
            o
        }
        Value::Range(b, s, e) => {
            let mut o = String::new();
            let _ = write!(o, "[{} : {} : {}]", number(*b), number(*s), number(*e));
            o
        }
        Value::Func(func, _) => func.to_string(),
    }
}

fn zip(a: &[Value], b: &[Value], op: &str) -> Value {
    let out: Vec<Value> = a.iter().zip(b).map(|(x, y)| binary(op, x, y)).collect();
    Value::list(out)
}

fn is_matrix(v: &[Value]) -> bool {
    !v.is_empty() && v.iter().all(|r| matches!(r, Value::List(_)))
}

/// How `a` and `b` are ordered: numbers, strings, booleans, and vectors
/// element by element, as C++'s lexicographical compare does it, where
/// elements that cannot be ordered count as the same.
fn compare(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::Equal;
    match (a, b) {
        (Value::Num(x), Value::Num(y)) => x.partial_cmp(y),
        (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (Value::List(x), Value::List(y)) => {
            for (p, q) in x.iter().zip(y.iter()) {
                match compare(p, q) {
                    Some(Equal) | None => {}
                    o => return o,
                }
            }
            Some(x.len().cmp(&y.len()))
        }
        _ => None,
    }
}

/// `a op b`.
pub fn binary(op: &str, a: &Value, b: &Value) -> Value {
    use Value::{Bool, List, Num};
    match op {
        "==" => return Bool(a == b),
        "!=" => return Bool(a != b),
        "&&" => return Bool(a.truthy() && b.truthy()),
        "||" => return Bool(a.truthy() || b.truthy()),
        "<" | "<=" | ">" | ">=" => {
            let order = compare(a, b);
            let Some(o) = order else {
                return Value::Undef;
            };
            return Bool(match op {
                "<" => o.is_lt(),
                "<=" => o.is_le(),
                ">" => o.is_gt(),
                _ => o.is_ge(),
            });
        }
        _ => {}
    }
    match (op, a, b) {
        ("+", Num(x), Num(y)) => Num(x + y),
        ("-", Num(x), Num(y)) => Num(x - y),
        ("*", Num(x), Num(y)) => Num(x * y),
        ("/", Num(x), Num(y)) => Num(x / y),
        ("%", Num(x), Num(y)) => Num(x % y),
        ("^", Num(x), Num(y)) => Num(x.powf(*y)),
        ("+" | "-", List(x), List(y)) => zip(x, y, op),
        ("*" | "/", List(x), Num(_)) => Value::list(x.iter().map(|v| binary(op, v, b)).collect()),
        ("*", Num(_), List(y)) => Value::list(y.iter().map(|v| binary(op, a, v)).collect()),
        ("*", List(x), List(y)) => multiply(x, y),
        _ => Value::Undef,
    }
}

/// Vector times vector (the dot product), matrix times vector, vector
/// times matrix, matrix times matrix.
fn multiply(x: &[Value], y: &[Value]) -> Value {
    match (is_matrix(x), is_matrix(y)) {
        (false, false) => {
            if x.len() != y.len() {
                return Value::Undef;
            }
            let mut sum = 0.0;
            for (a, b) in x.iter().zip(y) {
                match (a, b) {
                    (Value::Num(a), Value::Num(b)) => sum += a * b,
                    _ => return Value::Undef,
                }
            }
            Value::Num(sum)
        }
        (true, false) => Value::list(
            x.iter()
                .map(|row| binary("*", row, &Value::list(y.to_vec())))
                .collect(),
        ),
        (false, true) => {
            let cols = y
                .iter()
                .map(|r| r.nums().map_or(0, |r| r.len()))
                .min()
                .unwrap_or(0);
            let col = |j: usize| Value::list(y.iter().map(|r| item(r, j)).collect());
            let v = Value::list(x.to_vec());
            Value::list((0..cols).map(|j| binary("*", &v, &col(j))).collect())
        }
        (true, true) => {
            let cols = y
                .iter()
                .map(|r| r.nums().map_or(0, |r| r.len()))
                .min()
                .unwrap_or(0);
            let col = |j: usize| Value::list(y.iter().map(|r| item(r, j)).collect());
            Value::list(
                x.iter()
                    .map(|row| Value::list((0..cols).map(|j| binary("*", row, &col(j))).collect()))
                    .collect(),
            )
        }
    }
}

/// Element `i` of a vector, or undef.
pub fn item(v: &Value, i: usize) -> Value {
    match v {
        Value::List(x) => x.get(i).cloned().unwrap_or_default(),
        Value::Str(s) => s
            .chars()
            .nth(i)
            .map_or(Value::Undef, |c| Value::str(&c.to_string())),
        // A range's begin, step and end.
        Value::Range(b, s, e) => match i {
            0 => Value::Num(*b),
            1 => Value::Num(*s),
            2 => Value::Num(*e),
            _ => Value::Undef,
        },
        _ => Value::Undef,
    }
}

pub fn unary(op: &str, a: &Value) -> Value {
    match (op, a) {
        ("!", a) => Value::Bool(!a.truthy()),
        ("-", Value::Num(x)) => Value::Num(-x),
        ("-", Value::List(v)) => Value::list(v.iter().map(|x| unary("-", x)).collect()),
        _ => Value::Undef,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> Value {
        Value::Num(x)
    }

    #[test]
    fn numbers_print_as_in_openscad() {
        let cases = [
            (0.1 + 0.2, "0.3"),
            (1.0 / 3.0, "0.333333"),
            (100.0, "100"),
            (1e6, "1e+06"),
            (123456789.0, "1.23457e+08"),
            (1e-7, "1e-07"),
            (0.0001, "0.0001"),
            (-2.5, "-2.5"),
        ];
        for (x, s) in cases {
            assert_eq!(number(x), s);
        }
    }

    #[test]
    fn vectors_and_matrices_multiply() {
        let v = |x: &[f64]| Value::list(x.iter().map(|x| n(*x)).collect());
        assert_eq!(
            binary("*", &v(&[1.0, 2.0, 3.0]), &v(&[4.0, 5.0, 6.0])),
            n(32.0)
        );
        let m = Value::list(vec![v(&[0.0, -1.0]), v(&[1.0, 0.0])]);
        assert_eq!(binary("*", &m, &v(&[1.0, 0.0])), v(&[0.0, 1.0]));
        assert_eq!(binary("*", &v(&[1.0, 0.0]), &m), v(&[0.0, -1.0]));
        assert_eq!(
            binary("+", &v(&[1.0, 2.0]), &v(&[3.0, 4.0, 5.0])),
            v(&[4.0, 6.0])
        );
        assert_eq!(binary("+", &Value::str("a"), &n(1.0)), Value::Undef);
    }

    #[test]
    fn ranges_count_as_openscad_does() {
        assert_eq!(range_len(0.0, 1.0, 10.0), 11);
        assert_eq!(range_len(0.0, 0.25, 1.0), 5);
        assert_eq!(range_len(5.0, 1.0, 0.0), 0);
        assert_eq!(range_len(3.0, -1.0, 0.0), 4);
    }
}
