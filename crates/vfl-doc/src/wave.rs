//! WaveDrom timing diagrams, written in doc comments as fenced
//! ```` ```wavedrom ```` blocks and drawn as SVG without JavaScript.
//!
//! The common subset of WaveJSON is supported: `signal` with lanes
//! (`name`, `wave`, `data`), spacers (`{}`) and groups (`["name", ...]`),
//! `head` and `foot` text, and `config.hscale`. Wave characters: `0 1 l h
//! L H` levels, `p n P N` clocks, `x` unknown, `z` high impedance, `= 2-9`
//! data, `u d` pulled levels, `.` to extend and `|` for a gap.

use std::fmt::Write;

use crate::svg::{Canvas, cut, width};

/// A JSON5 value: what WaveJSON is written in (unquoted keys, single
/// quotes, trailing commas and comments are allowed).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn skip(&mut self) {
        loop {
            while self.at < self.text.len() && self.text[self.at].is_ascii_whitespace() {
                self.at += 1;
            }
            if self.text[self.at..].starts_with(b"//") {
                while self.at < self.text.len() && self.text[self.at] != b'\n' {
                    self.at += 1;
                }
            } else if self.text[self.at..].starts_with(b"/*") {
                self.at += 2;
                while self.at < self.text.len() && !self.text[self.at..].starts_with(b"*/") {
                    self.at += 1;
                }
                self.at = (self.at + 2).min(self.text.len());
            } else {
                return;
            }
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.text.get(self.at).copied()
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > 64 {
            return Err("nested too deeply".into());
        }
        match self.peek().ok_or("unexpected end")? {
            b'{' => {
                self.at += 1;
                let mut fields = Vec::new();
                loop {
                    match self.peek().ok_or("unclosed `{`")? {
                        b'}' => {
                            self.at += 1;
                            return Ok(Value::Object(fields));
                        }
                        b',' => self.at += 1,
                        _ => {
                            let key = self.key()?;
                            if self.peek() != Some(b':') {
                                return Err(format!("expected `:` after `{key}`"));
                            }
                            self.at += 1;
                            let value = self.value(depth + 1)?;
                            fields.push((key, value));
                        }
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                loop {
                    match self.peek().ok_or("unclosed `[`")? {
                        b']' => {
                            self.at += 1;
                            return Ok(Value::Array(items));
                        }
                        b',' => self.at += 1,
                        _ => items.push(self.value(depth + 1)?),
                    }
                }
            }
            b'"' | b'\'' => Ok(Value::Str(self.string()?)),
            c if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() => {
                let start = self.at;
                while self.at < self.text.len()
                    && (self.text[self.at].is_ascii_alphanumeric()
                        || matches!(self.text[self.at], b'.' | b'-' | b'+'))
                {
                    self.at += 1;
                }
                let s = std::str::from_utf8(&self.text[start..self.at]).unwrap_or("");
                s.parse()
                    .map(Value::Number)
                    .map_err(|_| format!("bad number `{s}`"))
            }
            _ => {
                let word = self.key()?;
                match word.as_str() {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    "null" => Ok(Value::Null),
                    _ => Err(format!("unexpected `{word}`")),
                }
            }
        }
    }

    fn key(&mut self) -> Result<String, String> {
        match self.peek() {
            Some(b'"' | b'\'') => self.string(),
            _ => {
                let start = self.at;
                while self.at < self.text.len()
                    && (self.text[self.at].is_ascii_alphanumeric()
                        || matches!(self.text[self.at], b'_' | b'$'))
                {
                    self.at += 1;
                }
                if start == self.at {
                    return Err(format!(
                        "unexpected `{}`",
                        self.text.get(self.at).map_or(' ', |&c| c as char)
                    ));
                }
                Ok(String::from_utf8_lossy(&self.text[start..self.at]).into_owned())
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        let quote = self.text[self.at];
        self.at += 1;
        let mut out = Vec::new();
        while self.at < self.text.len() {
            let c = self.text[self.at];
            self.at += 1;
            if c == quote {
                return Ok(String::from_utf8_lossy(&out).into_owned());
            }
            if c == b'\\' && self.at < self.text.len() {
                let e = self.text[self.at];
                self.at += 1;
                out.push(match e {
                    b'n' => b'\n',
                    b't' => b'\t',
                    other => other,
                });
            } else {
                out.push(c);
            }
        }
        Err("unclosed string".into())
    }
}

/// Parses JSON5 text.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut parser = Parser {
        text: text.as_bytes(),
        at: 0,
    };
    let value = parser.value(0)?;
    if parser.peek().is_some() {
        return Err("text after the end".into());
    }
    Ok(value)
}

/// One row of the diagram.
enum Row {
    Lane {
        name: String,
        wave: String,
        data: Vec<String>,
        indent: usize,
    },
    Group {
        name: String,
        indent: usize,
    },
    Spacer,
}

fn rows(signal: &[Value], indent: usize, out: &mut Vec<Row>) {
    for item in signal {
        match item {
            Value::Object(_) => match item.get("wave").and_then(Value::as_str) {
                Some(wave) => {
                    let data = match item.get("data") {
                        Some(Value::Array(items)) => items
                            .iter()
                            .map(|v| match v {
                                Value::Str(s) => s.clone(),
                                Value::Number(n) => format!("{n}"),
                                _ => String::new(),
                            })
                            .collect(),
                        Some(Value::Str(s)) => s.split_whitespace().map(str::to_owned).collect(),
                        _ => Vec::new(),
                    };
                    out.push(Row::Lane {
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        wave: wave.to_owned(),
                        data,
                        indent,
                    });
                }
                None => out.push(Row::Spacer),
            },
            Value::Array(items) => {
                let (name, rest) = match items.first() {
                    Some(Value::Str(name)) => (name.clone(), &items[1..]),
                    _ => (String::new(), &items[..]),
                };
                if !name.is_empty() {
                    out.push(Row::Group {
                        name,
                        indent,
                    });
                }
                rows(rest, indent + 1, out);
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    High,
    Low,
    Pos,
    Neg,
    Unknown,
    Z,
    /// A data value, with its colour (0 for `=`, else 2 to 9).
    Data(u8),
}

fn state(c: char) -> Option<State> {
    Some(match c {
        '1' | 'h' | 'H' | 'u' => State::High,
        '0' | 'l' | 'L' | 'd' => State::Low,
        'p' | 'P' => State::Pos,
        'n' | 'N' => State::Neg,
        'x' => State::Unknown,
        'z' => State::Z,
        '=' => State::Data(0),
        '2'..='9' => State::Data(c as u8 - b'0'),
        _ => return None,
    })
}

const STYLE: &str = "<defs><pattern id=\"vfl-hatch\" width=\"6\" height=\"6\" patternUnits=\"userSpaceOnUse\" patternTransform=\"rotate(45)\"><line class=\"vfl-sep\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"6\"/></pattern></defs>";

/// Draws WaveJSON text; `Err` with the reason when it cannot be read.
pub fn draw(source: &str) -> Result<String, String> {
    let value = parse(source)?;
    let Some(Value::Array(signal)) = value.get("signal") else {
        return Err("no `signal` list".into());
    };
    let hscale = match value.get("config").and_then(|c| c.get("hscale")) {
        Some(Value::Number(n)) if *n >= 0.5 && *n <= 8.0 => *n,
        _ => 1.0,
    };
    let text_of = |key: &str| -> Option<String> {
        match value.get(key)?.get("text")? {
            Value::Str(s) => Some(s.clone()),
            _ => None,
        }
    };
    let mut list = Vec::new();
    rows(signal, 0, &mut list);
    const LANE: f64 = 30.0;
    const SLANT: f64 = 3.0;
    let period = 40.0 * hscale;
    let names_w = list
        .iter()
        .map(|r| match r {
            Row::Lane { name, indent, .. } | Row::Group { name, indent } => {
                width(&cut(name, 32)) + *indent as f64 * 10.0
            }
            Row::Spacer => 0.0,
        })
        .fold(0.0, f64::max)
        + 14.0;
    let cycles = list
        .iter()
        .map(|r| match r {
            Row::Lane { wave, .. } => wave.chars().count(),
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    let x0 = names_w;
    let mut c = Canvas::new();
    c.raw(STYLE);
    let mut y = 0.0;
    if let Some(head) = text_of("head") {
        c.text(x0 + cycles as f64 * period / 2.0, 14.0, &cut(&head, 80), "middle", "vfl-title");
        y += 26.0;
    }
    let top = y;
    // Grid.
    for i in 0..=cycles {
        let x = x0 + i as f64 * period;
        let bottom = top + list.len() as f64 * LANE;
        c.line(x, top, x, bottom, "vfl-grid");
    }
    for row in &list {
        match row {
            Row::Spacer => {}
            Row::Group { name, indent } => {
                c.text(
                    *indent as f64 * 10.0,
                    y + 19.0,
                    &cut(name, 32),
                    "start",
                    "vfl-title",
                );
            }
            Row::Lane {
                name,
                wave,
                data,
                indent,
            } => {
                let _ = indent;
                c.text(x0 - 8.0, y + 19.0, &cut(name, 32), "end", "");
                lane(&mut c, wave, data, x0, y, period, SLANT);
            }
        }
        y += LANE;
    }
    if let Some(foot) = text_of("foot") {
        c.text(
            x0 + cycles as f64 * period / 2.0,
            y + 16.0,
            &cut(&foot, 80),
            "middle",
            "vfl-muted",
        );
    }
    c.extend(0.0, 0.0, x0 + cycles as f64 * period, y);
    Ok(c.finish("Timing diagram", 8.0))
}

fn lane(c: &mut Canvas, wave: &str, data: &[String], x0: f64, y: f64, period: f64, slant: f64) {
    let (high, low) = (y + 5.0, y + 25.0);
    let mid = (high + low) / 2.0;
    let chars: Vec<char> = wave.chars().collect();
    // Segments: (state, start cycle, end cycle), a new one at each wave
    // character but `.`, `|` and a level that repeats.
    let mut segments: Vec<(State, usize, usize)> = Vec::new();
    let mut gaps = Vec::new();
    for (i, &ch) in chars.iter().enumerate() {
        match (ch, state(ch), segments.last_mut()) {
            ('.', _, Some(last)) => last.2 = i + 1,
            ('|', _, Some(last)) => {
                last.2 = i + 1;
                gaps.push(i);
            }
            (_, Some(s @ (State::High | State::Low | State::Z | State::Unknown)), Some(last))
                if last.0 == s =>
            {
                last.2 = i + 1;
            }
            (_, Some(s), _) => segments.push((s, i, i + 1)),
            _ => {}
        }
    }
    let mut data_index = 0;
    let mut path = String::new();
    let mut previous: Option<State> = None;
    for &(s, from, to) in &segments {
        let xa = x0 + from as f64 * period;
        let xb = x0 + to as f64 * period;
        match s {
            State::High | State::Low => {
                let level = if s == State::High { high } else { low };
                let start = match previous {
                    Some(State::High) => high,
                    Some(State::Low) => low,
                    Some(State::Pos) => low,
                    Some(State::Neg) => high,
                    None => level,
                    _ => mid,
                };
                let _ = write!(
                    path,
                    "M{xa:.1},{start:.1}L{:.1},{level:.1}H{xb:.1}",
                    xa + slant
                );
            }
            State::Pos | State::Neg => {
                for k in from..to {
                    let xs = x0 + k as f64 * period;
                    let half = xs + period / 2.0;
                    let end = xs + period;
                    let (first, second) = if s == State::Pos {
                        (high, low)
                    } else {
                        (low, high)
                    };
                    let _ = write!(
                        path,
                        "M{xs:.1},{second:.1}V{first:.1}H{half:.1}V{second:.1}H{end:.1}"
                    );
                }
            }
            State::Z => {
                let _ = write!(path, "M{xa:.1},{mid:.1}H{xb:.1}");
            }
            State::Unknown | State::Data(_) => {
                let shape = format!(
                    "M{xa:.1},{mid:.1}L{:.1},{high:.1}H{:.1}L{xb:.1},{mid:.1}L{:.1},{low:.1}H{:.1}Z",
                    xa + slant,
                    xb - slant,
                    xb - slant,
                    xa + slant
                );
                let class = match s {
                    State::Unknown => "vfl-wave vfl-wave-x".to_owned(),
                    State::Data(d) => format!("vfl-wave vfl-wave-d{d}"),
                    _ => unreachable!(),
                };
                c.raw(&format!(r#"<path class="{class}" d="{shape}"/>"#));
                if let State::Data(_) = s {
                    let room = ((xb - xa - 2.0 * slant) / 7.25).floor().max(1.0) as usize;
                    if let Some(label) = data.get(data_index) {
                        c.text((xa + xb) / 2.0, mid + 4.0, &cut(label, room), "middle", "");
                    }
                    data_index += 1;
                }
            }
        }
        previous = Some(s);
    }
    if !path.is_empty() {
        c.raw(&format!(r#"<path class="vfl-wave" d="{path}"/>"#));
    }
    for g in gaps {
        let x = x0 + g as f64 * period + period / 2.0;
        c.raw(&format!(
            r#"<path class="vfl-gap" d="M{:.1},{:.1}l4,{:.1}h3l-4,{:.1}z"/>"#,
            x - 4.0,
            high - 2.0,
            low - high + 4.0,
            -(low - high + 4.0)
        ));
    }
    c.extend(x0, high, x0 + segments.last().map_or(0, |s| s.2) as f64 * period, low);
}
