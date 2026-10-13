//! SVG diagrams: entity symbols, the instantiation tree and state machines.
//!
//! Text is set in a monospace font so its width can be computed without
//! font metrics. Every diagram carries its own style sheet, with a dark
//! variant, so the files stand alone and also work inline in HTML.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::fsm::Fsm;
use crate::hierarchy::Node;
use crate::model::{Direction, Entity, Interface};

const FONT_SIZE: f64 = 12.0;
/// The advance of one character of a 12px monospace font.
const CHAR_WIDTH: f64 = 7.25;
const STYLE: &str = "\
.vfl-box{fill:#f6f8fa;stroke:#424a53;stroke-width:1.5}\
.vfl-head{fill:#ddf4ff;stroke:#424a53;stroke-width:1.5}\
.vfl-ext{fill:#ffffff;stroke:#8c959f;stroke-width:1.2;stroke-dasharray:4 3}\
.vfl-sep{stroke:#8c959f;stroke-width:1;stroke-dasharray:3 3}\
.vfl-pin{stroke:#424a53;stroke-width:1.5;fill:#424a53}\
.vfl-edge{stroke:#57606a;stroke-width:1.2;fill:none}\
.vfl-arrow{fill:#57606a}\
.vfl-wire{stroke:#0969da;stroke-width:1.3;fill:none}\
.vfl-dot{fill:#0969da}\
.vfl-term{fill:#ddf4ff;stroke:#424a53;stroke-width:1.2}\
.vfl-small{font-size:10px}\
.vfl-actual{fill:#0550ae}\
.vfl-state{fill:#f6f8fa;stroke:#424a53;stroke-width:1.5}\
.vfl-initial{fill:#ddf4ff;stroke:#0969da;stroke-width:2}\
.vfl-label-bg{fill:#ffffff;fill-opacity:0.85}\
text{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,'DejaVu Sans Mono','Liberation Mono',monospace;font-size:12px;fill:#1f2328}\
.vfl-title{font-weight:bold}\
.vfl-muted{fill:#59636e}\
a:hover text{text-decoration:underline}\
@media (prefers-color-scheme:dark){\
.vfl-box{fill:#151b23;stroke:#9198a1}\
.vfl-head{fill:#0c2d6b;stroke:#9198a1}\
.vfl-ext{fill:#0d1117;stroke:#656c76}\
.vfl-pin{stroke:#9198a1;fill:#9198a1}\
.vfl-edge{stroke:#9198a1}\
.vfl-arrow{fill:#9198a1}\
.vfl-wire{stroke:#4493f8}\
.vfl-dot{fill:#4493f8}\
.vfl-term{fill:#0c2d6b;stroke:#9198a1}\
.vfl-state{fill:#151b23;stroke:#9198a1}\
.vfl-initial{fill:#0c2d6b;stroke:#4493f8}\
.vfl-label-bg{fill:#0d1117}\
text{fill:#f0f6fc}\
.vfl-actual{fill:#79c0ff}\
.vfl-muted{fill:#9198a1}}";

/// The most arrows of a state diagram that get their conditions written.
const MAX_FSM_LABELS: usize = 10;

/// The longest text a diagram shows in one piece; longer texts are cut.
const MAX_TEXT: usize = 48;

pub(crate) fn width(text: &str) -> f64 {
    text.chars().count() as f64 * CHAR_WIDTH
}

/// Escapes text for XML.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

pub(crate) fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// An SVG document under construction, tracking its extent.
pub(crate) struct Canvas {
    body: String,
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl Canvas {
    pub(crate) fn new() -> Self {
        Canvas {
            body: String::new(),
            min_x: f64::MAX,
            min_y: f64::MAX,
            max_x: f64::MIN,
            max_y: f64::MIN,
        }
    }

    pub(crate) fn extend(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        self.min_x = self.min_x.min(x0.min(x1));
        self.min_y = self.min_y.min(y0.min(y1));
        self.max_x = self.max_x.max(x0.max(x1));
        self.max_y = self.max_y.max(y0.max(y1));
    }

    pub(crate) fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, class: &str) {
        self.extend(x, y, x + w, y + h);
        let _ = write!(
            self.body,
            r#"<rect class="{class}" x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="3"/>"#
        );
    }

    pub(crate) fn line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, class: &str) {
        self.extend(x0, y0, x1, y1);
        let _ = write!(
            self.body,
            r#"<line class="{class}" x1="{x0:.1}" y1="{y0:.1}" x2="{x1:.1}" y2="{y1:.1}"/>"#
        );
    }

    /// Text with its baseline at `y`; `anchor` is `start`, `middle` or `end`.
    pub(crate) fn text(&mut self, x: f64, y: f64, text: &str, anchor: &str, class: &str) {
        let w = width(text);
        let x0 = match anchor {
            "middle" => x - w / 2.0,
            "end" => x - w,
            _ => x,
        };
        self.extend(x0, y - FONT_SIZE, x0 + w, y + 4.0);
        let class = if class.is_empty() {
            String::new()
        } else {
            format!(r#" class="{class}""#)
        };
        let _ = write!(
            self.body,
            r#"<text x="{x:.1}" y="{y:.1}" text-anchor="{anchor}"{class}>{}</text>"#,
            escape(text)
        );
    }

    pub(crate) fn raw(&mut self, text: &str) {
        self.body.push_str(text);
    }

    pub(crate) fn finish(self, title: &str, margin: f64) -> String {
        let (x, y) = (self.min_x - margin, self.min_y - margin);
        let w = (self.max_x - self.min_x + 2.0 * margin).max(1.0);
        let h = (self.max_y - self.min_y + 2.0 * margin).max(1.0);
        format!(
            concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" "#,
                r#"class="vfl-svg" width="{w:.0}" height="{h:.0}" viewBox="{x:.1} {y:.1} {w:.1} {h:.1}" role="img">"#,
                "<title>{title}</title><style>{style}</style>",
                r#"<defs><marker id="vfl-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path class="vfl-arrow" d="M0,0L10,5L0,10z"/></marker></defs>"#,
                "{body}</svg>\n"
            ),
            w = w,
            h = h,
            x = x,
            y = y,
            title = escape(title),
            style = STYLE,
            body = self.body,
        )
    }
}

/// Options of the entity symbol.
#[derive(Debug, Clone, Copy)]
pub struct SymbolOptions {
    pub port_types: bool,
    pub generics: bool,
}

/// The block symbol of an entity: inputs on the left, outputs and
/// bidirectional ports on the right, generics under the name.
pub fn symbol(entity: &Entity, options: SymbolOptions) -> String {
    symbol_of(&entity.name, &entity.generics, &entity.ports, options)
}

pub(crate) fn symbol_of(
    name: &str,
    generics: &[Interface],
    ports: &[Interface],
    options: SymbolOptions,
) -> String {
    let generic_lines: Vec<(String, bool)> = if options.generics {
        generics
            .iter()
            .map(|g| {
                let mut line = format!("{} : {}", g.name, g.ty);
                if let Some(default) = &g.default {
                    let _ = write!(line, " := {default}");
                }
                (line, true)
            })
            .collect()
    } else {
        Vec::new()
    };
    let outside = |port: &Interface| {
        options
            .port_types
            .then(|| (port.ty.clone(), true))
            .filter(|(ty, _)| !ty.is_empty())
    };
    draw_symbol(name, None, &generic_lines, ports, &outside).finish(name, 8.0)
}

/// What a symbol writes beyond the end of a pin, and whether it is muted.
pub(crate) type PinText<'a> = &'a dyn Fn(&Interface) -> Option<(String, bool)>;

/// A block with a title (and a subtitle), lines of generics, and a pin per
/// port: inputs on the left, the others on the right. `outside` gives the
/// text beyond each pin: the type in a symbol, the actual in a port map.
pub(crate) fn draw_symbol(
    title: &str,
    subtitle: Option<&str>,
    generic_lines: &[(String, bool)],
    ports: &[Interface],
    outside: PinText,
) -> Canvas {
    const PAD: f64 = 10.0;
    const ROW: f64 = 18.0;
    const PIN: f64 = 24.0;
    let head = if subtitle.is_some() { 38.0 } else { 26.0 };
    let left: Vec<&Interface> = ports
        .iter()
        .filter(|p| p.direction() == Direction::In)
        .collect();
    let right: Vec<&Interface> = ports
        .iter()
        .filter(|p| p.direction() != Direction::In)
        .collect();
    let generic_lines: Vec<(String, bool)> = generic_lines
        .iter()
        .map(|(l, muted)| (cut(l, MAX_TEXT), *muted))
        .collect();
    let name_w = |list: &[&Interface]| {
        list.iter()
            .map(|p| width(&cut(&p.name, MAX_TEXT)))
            .fold(0.0, f64::max)
    };
    let inner = (name_w(&left) + name_w(&right) + 4.0 * PAD)
        .max(width(&cut(title, MAX_TEXT)) + 2.0 * PAD)
        .max(subtitle.map_or(0.0, |s| width(&cut(s, MAX_TEXT))) + 2.0 * PAD)
        .max(
            generic_lines
                .iter()
                .map(|(l, _)| width(l))
                .fold(0.0, f64::max)
                + 2.0 * PAD,
        )
        .max(120.0);
    let rows = left.len().max(right.len()).max(1) as f64;
    let generics_h = if generic_lines.is_empty() {
        0.0
    } else {
        generic_lines.len() as f64 * ROW + PAD
    };
    let body_h = generics_h + rows * ROW + PAD * 1.5;

    let mut c = Canvas::new();
    let (x, y) = (0.0, 0.0);
    c.rect(x, y, inner, head + body_h, "vfl-box");
    c.rect(x, y, inner, head, "vfl-head");
    c.text(
        x + inner / 2.0,
        y + 17.0,
        &cut(title, MAX_TEXT),
        "middle",
        "vfl-title",
    );
    if let Some(subtitle) = subtitle {
        c.text(
            x + inner / 2.0,
            y + 31.0,
            &cut(subtitle, MAX_TEXT),
            "middle",
            "vfl-muted",
        );
    }
    let mut row_y = y + head;
    if !generic_lines.is_empty() {
        for (i, (line, muted)) in generic_lines.iter().enumerate() {
            c.text(
                x + PAD,
                row_y + 14.0 + i as f64 * ROW,
                line,
                "start",
                if *muted { "vfl-muted" } else { "" },
            );
        }
        row_y += generics_h;
        c.line(
            x,
            row_y - PAD / 2.0,
            x + inner,
            row_y - PAD / 2.0,
            "vfl-sep",
        );
    }
    row_y += PAD / 2.0;
    let class = |muted: bool| if muted { "vfl-muted" } else { "vfl-actual" };
    for (i, port) in left.iter().enumerate() {
        let py = row_y + i as f64 * ROW + ROW / 2.0;
        c.line(x - PIN, py, x, py, "vfl-pin");
        // An arrow into the box.
        c.raw(&format!(
            r#"<path class="vfl-pin" d="M{:.1},{:.1}l-6,-4v8z"/>"#,
            x - 1.0,
            py
        ));
        c.text(x + 6.0, py + 4.0, &cut(&port.name, MAX_TEXT), "start", "");
        if let Some((text, muted)) = outside(port) {
            c.text(
                x - PIN - 4.0,
                py + 4.0,
                &cut(&text, MAX_TEXT),
                "end",
                class(muted),
            );
        }
    }
    let rx = x + inner;
    for (i, port) in right.iter().enumerate() {
        let py = row_y + i as f64 * ROW + ROW / 2.0;
        c.line(rx, py, rx + PIN, py, "vfl-pin");
        match port.direction() {
            Direction::InOut => c.raw(&format!(
                r#"<path class="vfl-pin" d="M{:.1},{:.1}l5,-4l5,4l-5,4z"/>"#,
                rx + PIN - 10.0,
                py
            )),
            _ => c.raw(&format!(
                r#"<path class="vfl-pin" d="M{:.1},{:.1}l-6,-4v8z"/>"#,
                rx + PIN,
                py
            )),
        }
        let label = if port.mode == "buffer" || port.mode == "linkage" || port.mode == "view" {
            format!("{} ({})", port.name, port.mode)
        } else {
            port.name.clone()
        };
        c.text(rx - 6.0, py + 4.0, &cut(&label, MAX_TEXT), "end", "");
        if let Some((text, muted)) = outside(port) {
            c.text(
                rx + PIN + 4.0,
                py + 4.0,
                &cut(&text, MAX_TEXT),
                "start",
                class(muted),
            );
        }
    }
    // Room for the pins of an entity without types.
    c.extend(x - PIN - 2.0, y, rx + PIN + 2.0, y);
    c
}

/// The instantiation tree, drawn as an indented outline. `link` gives the
/// page of a documented entity, for clickable boxes.
pub fn hierarchy(roots: &[Node], link: Option<&dyn Fn(usize) -> String>) -> String {
    const ROW: f64 = 30.0;
    const BOX: f64 = 22.0;
    const INDENT: f64 = 28.0;
    let mut c = Canvas::new();
    let mut y = 0.0;
    fn draw(
        c: &mut Canvas,
        node: &Node,
        depth: usize,
        y: &mut f64,
        link: Option<&dyn Fn(usize) -> String>,
    ) -> (f64, f64) {
        let x = depth as f64 * INDENT;
        let top = *y;
        let mut text = match &node.label {
            Some(label) => format!("{label} : {}", node.unit),
            None => node.unit.clone(),
        };
        let arch = node
            .architecture
            .as_ref()
            .map(|a| format!(" ({a})"))
            .unwrap_or_default();
        let note = if node.recursive {
            " (recursive)"
        } else if node.repeated {
            " (see above)"
        } else if node.entity.is_none() {
            " (external)"
        } else {
            ""
        };
        text = cut(&text, MAX_TEXT);
        let suffix = format!("{arch}{note}");
        let w = width(&text) + width(&suffix) + 16.0;
        let class = if node.entity.is_some() {
            "vfl-box"
        } else {
            "vfl-ext"
        };
        let href = node.entity.and_then(|e| link.map(|l| l(e)));
        if let Some(href) = &href {
            c.raw(&format!(r#"<a href="{}">"#, escape(href)));
        }
        c.rect(x, top, w, BOX, class);
        c.text(x + 8.0, top + 15.0, &text, "start", "");
        if !suffix.is_empty() {
            c.text(
                x + 8.0 + width(&text),
                top + 15.0,
                &suffix,
                "start",
                "vfl-muted",
            );
        }
        if href.is_some() {
            c.raw("</a>");
        }
        *y += ROW;
        for child in &node.children {
            let (child_x, child_mid) = draw(c, child, depth + 1, y, link);
            let stem = x + 12.0;
            c.raw(&format!(
                r#"<path class="vfl-edge" d="M{stem:.1},{:.1}V{child_mid:.1}H{child_x:.1}"/>"#,
                top + BOX
            ));
        }
        (x, top + BOX / 2.0)
    }
    for root in roots {
        draw(&mut c, root, 0, &mut y, link);
        y += 8.0;
    }
    if roots.is_empty() {
        c.text(0.0, 14.0, "(no entities)", "start", "vfl-muted");
    }
    c.finish("Design hierarchy", 6.0)
}

/// A state diagram: states on a circle, transitions as arrows labelled
/// with their conditions.
pub fn fsm(machine: &Fsm) -> String {
    use std::f64::consts::PI;
    let n = machine.states.len().max(1);
    let r = machine
        .states
        .iter()
        .map(|s| width(&cut(s, 24)) / 2.0 + 10.0)
        .fold(24.0, f64::max);
    let index = |state: &str| machine.states.iter().position(|s| s == state);

    // Transitions between the same states share one arrow.
    let mut edges: BTreeMap<(usize, usize), Vec<Option<String>>> = BTreeMap::new();
    for t in &machine.transitions {
        if let (Some(a), Some(b)) = (index(&t.from), index(&t.to)) {
            let conditions = edges.entry((a, b)).or_default();
            if !conditions.contains(&t.condition) {
                conditions.push(t.condition.clone());
            }
        }
    }
    // Past a few labelled arrows the labels overlap more than they help:
    // the transition table of the page lists the conditions.
    let labelled = edges
        .values()
        .filter(|c| c.iter().any(Option::is_some))
        .count();
    let show_labels = labelled <= MAX_FSM_LABELS;
    let widest = if show_labels {
        edges
            .values()
            .flatten()
            .flatten()
            .map(|c| width(&cut(c, 28)))
            .fold(0.0, f64::max)
    } else {
        0.0
    };
    // Room between neighbouring states for the labels of their arrows.
    let ring = if n <= 1 {
        0.0
    } else {
        ((2.0 * r + 90.0 + 0.8 * widest) * n as f64 / (2.0 * PI)).max(r + 60.0 + widest / 2.0)
    };
    let position = |i: usize| -> (f64, f64) {
        if n == 2 {
            return (i as f64 * (2.0 * ring), 0.0);
        }
        let angle = -PI / 2.0 + 2.0 * PI * i as f64 / n as f64;
        (ring * angle.cos(), ring * angle.sin())
    };
    let center = if n == 2 { (ring, 0.0) } else { (0.0, 0.0) };
    let mut c = Canvas::new();
    let mut labels = Canvas::new();
    // A label centred at (x, y), or beside it on the side of the normal
    // (nx, ny) so that it clears its arrow.
    let label = |labels: &mut Canvas,
                 x: f64,
                 y: f64,
                 normal: Option<(f64, f64)>,
                 conditions: &[Option<String>]| {
        if !show_labels {
            return;
        }
        let mut lines: Vec<String> = conditions.iter().flatten().map(|c| cut(c, 28)).collect();
        if lines.len() > 3 {
            lines.truncate(3);
            lines.push("…".into());
        }
        let h = lines.len() as f64 * 14.0;
        let w = lines.iter().map(|l| width(l)).fold(0.0, f64::max);
        if lines.is_empty() {
            return;
        }
        let (x, y) = match normal {
            Some((nx, ny)) => {
                let reach = (w / 2.0 + 2.0) * nx.abs() + (h / 2.0 + 2.0) * ny.abs() + 2.0;
                (x + nx * reach, y + ny * reach)
            }
            None => (x, y),
        };
        labels.rect(
            x - w / 2.0 - 2.0,
            y - h / 2.0 - 2.0,
            w + 4.0,
            h + 4.0,
            "vfl-label-bg",
        );
        for (i, line) in lines.iter().enumerate() {
            labels.text(
                x,
                y - h / 2.0 + 11.0 + i as f64 * 14.0,
                line,
                "middle",
                "vfl-muted",
            );
        }
    };
    for (&(a, b), conditions) in &edges {
        let (ax, ay) = position(a);
        let (bx, by) = position(b);
        if a == b {
            // A loop on the outer side of the state.
            let (ox, oy) = (ax - center.0, ay - center.1);
            let len = (ox * ox + oy * oy).sqrt();
            let angle = if len < 1e-6 { -PI / 2.0 } else { oy.atan2(ox) };
            let p = |t: f64, d: f64| (ax + d * t.cos(), ay + d * t.sin());
            let (sx, sy) = p(angle - 0.45, r);
            let (ex, ey) = p(angle + 0.45, r);
            let (c1x, c1y) = p(angle - 0.6, r * 2.4);
            let (c2x, c2y) = p(angle + 0.6, r * 2.4);
            c.extend(c1x, c1y, c2x, c2y);
            c.raw(&format!(
                r#"<path class="vfl-edge" marker-end="url(#vfl-arrow)" d="M{sx:.1},{sy:.1}C{c1x:.1},{c1y:.1} {c2x:.1},{c2y:.1} {ex:.1},{ey:.1}"/>"#
            ));
            let (lx, ly) = p(angle, r * 2.3);
            label(&mut labels, lx, ly, None, conditions);
            continue;
        }
        let (dx, dy) = (bx - ax, by - ay);
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let (ux, uy) = (dx / len, dy / len);
        // Bend both arrows of a pair, to opposite sides.
        let bend = if edges.contains_key(&(b, a)) {
            32.0
        } else {
            0.0
        };
        let (mx, my) = ((ax + bx) / 2.0 - uy * bend, (ay + by) / 2.0 + ux * bend);
        let towards = |px: f64, py: f64, qx: f64, qy: f64| {
            let (vx, vy) = (qx - px, qy - py);
            let l = (vx * vx + vy * vy).sqrt().max(1e-6);
            (px + vx / l * r, py + vy / l * (r + 0.0))
        };
        let (sx, sy) = towards(ax, ay, mx, my);
        let (ex, ey) = towards(bx, by, mx, my);
        let (ex, ey) = (ex - (ex - bx) * 0.04, ey - (ey - by) * 0.04);
        c.raw(&format!(
            r#"<path class="vfl-edge" marker-end="url(#vfl-arrow)" d="M{sx:.1},{sy:.1}Q{mx:.1},{my:.1} {ex:.1},{ey:.1}"/>"#
        ));
        let (lx, ly) = (
            0.25 * sx + 0.5 * mx + 0.25 * ex,
            0.25 * sy + 0.5 * my + 0.25 * ey,
        );
        // A bent arrow's label goes on the outer side of its bend.
        let normal = (bend > 0.0).then_some((-uy, ux));
        label(&mut labels, lx, ly, normal, conditions);
    }
    for (i, state) in machine.states.iter().enumerate() {
        let (x, y) = position(i);
        let initial = machine.initial.as_deref() == Some(state.as_str());
        c.extend(x - r, y - r, x + r, y + r);
        let class = if initial { "vfl-initial" } else { "vfl-state" };
        c.raw(&format!(
            r#"<circle class="{class}" cx="{x:.1}" cy="{y:.1}" r="{r:.1}"/>"#
        ));
        c.text(
            x,
            y + 4.0,
            &cut(state, 24),
            "middle",
            if initial { "vfl-title" } else { "" },
        );
        if initial {
            // The entry arrow, from outside the ring.
            let (ox, oy) = (x - center.0, y - center.1);
            let len = (ox * ox + oy * oy).sqrt();
            let (ux, uy) = if len < 1e-6 {
                (0.0, -1.0)
            } else {
                (ox / len, oy / len)
            };
            // Enter beside a self-loop, not through it.
            let (ux, uy) = (ux * 0.6 - uy * 0.8, uy * 0.6 + ux * 0.8);
            let (sx, sy) = (x + ux * (r + 30.0), y + uy * (r + 30.0));
            let (ex, ey) = (x + ux * (r + 2.0), y + uy * (r + 2.0));
            c.extend(sx - 5.0, sy - 5.0, sx + 5.0, sy + 5.0);
            c.raw(&format!(
                r#"<circle class="vfl-pin" cx="{sx:.1}" cy="{sy:.1}" r="4"/><path class="vfl-edge" marker-end="url(#vfl-arrow)" d="M{sx:.1},{sy:.1}L{ex:.1},{ey:.1}"/>"#
            ));
        }
    }
    if !labels.body.is_empty() {
        c.extend(labels.min_x, labels.min_y, labels.max_x, labels.max_y);
    }
    c.body.push_str(&labels.body);
    c.finish(&format!("State machine {}", machine.signal), 10.0)
}
