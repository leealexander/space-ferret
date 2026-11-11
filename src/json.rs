//! Tiny JSON writer (keeps the binary dependency-free apart from rayon).
//! Objects/arrays whose members are all scalars are written on one line,
//! which keeps the output compact but still easy to read.
use std::fmt::Write;

pub enum J {
    Null,
    B(bool),
    U(u64),
    I(i64),
    /// float written with one decimal (percentages)
    F(f64),
    S(String),
    A(Vec<J>),
    O(Vec<(&'static str, J)>),
}

impl J {
    pub fn s(v: impl Into<String>) -> J {
        J::S(v.into())
    }

    pub fn opt_i(v: Option<i64>) -> J {
        v.map_or(J::Null, J::I)
    }

    fn scalar(&self) -> bool {
        match self {
            J::A(v) => v.is_empty(),
            J::O(v) => v.is_empty(),
            _ => true,
        }
    }

    pub fn render(&self) -> String {
        let mut o = String::with_capacity(32 * 1024);
        self.write(&mut o, 0);
        o.push('\n');
        o
    }

    fn write(&self, o: &mut String, ind: usize) {
        match self {
            J::Null => o.push_str("null"),
            J::B(b) => o.push_str(if *b { "true" } else { "false" }),
            J::U(n) => {
                let _ = write!(o, "{n}");
            }
            J::I(n) => {
                let _ = write!(o, "{n}");
            }
            J::F(f) => {
                if f.is_finite() {
                    let _ = write!(o, "{f:.1}");
                } else {
                    o.push('0');
                }
            }
            J::S(s) => esc(o, s),
            J::A(v) => {
                if v.iter().all(J::scalar) {
                    o.push('[');
                    for (i, x) in v.iter().enumerate() {
                        if i > 0 {
                            o.push_str(", ");
                        }
                        x.write(o, ind);
                    }
                    o.push(']');
                    return;
                }
                o.push_str("[\n");
                for (i, x) in v.iter().enumerate() {
                    pad(o, ind + 1);
                    x.write(o, ind + 1);
                    if i + 1 < v.len() {
                        o.push(',');
                    }
                    o.push('\n');
                }
                pad(o, ind);
                o.push(']');
            }
            J::O(kv) => {
                if kv.iter().all(|(_, x)| x.scalar()) {
                    o.push('{');
                    for (i, (k, x)) in kv.iter().enumerate() {
                        if i > 0 {
                            o.push_str(", ");
                        }
                        esc(o, k);
                        o.push_str(": ");
                        x.write(o, ind);
                    }
                    o.push('}');
                    return;
                }
                o.push_str("{\n");
                for (i, (k, x)) in kv.iter().enumerate() {
                    pad(o, ind + 1);
                    esc(o, k);
                    o.push_str(": ");
                    x.write(o, ind + 1);
                    if i + 1 < kv.len() {
                        o.push(',');
                    }
                    o.push('\n');
                }
                pad(o, ind);
                o.push('}');
            }
        }
    }
}

fn pad(o: &mut String, n: usize) {
    for _ in 0..n {
        o.push_str("  ");
    }
}

fn esc(o: &mut String, s: &str) {
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
}
