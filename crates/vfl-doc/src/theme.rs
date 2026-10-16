//! Themes: the colors and fonts of the pages and the diagrams.
//!
//! Diagrams are drawn with the style sheet of the built-in theme. A
//! configured theme swaps that style sheet, and the colors of the HTML
//! pages, for its own when the files are written ([`Theme::apply`]), so
//! drawing needs no theme and the built-in one costs nothing.

use std::borrow::Cow;
use std::sync::LazyLock;

use vfl_config::{DocTheme, ThemeColors, ThemeMode};

/// The font of the page text when none is configured.
const SANS: &str = "-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif";
/// The font of code and diagrams when none is configured.
const MONO: &str =
    "ui-monospace,SFMono-Regular,Menlo,Consolas,'DejaVu Sans Mono','Liberation Mono',monospace";

/// The colors of one theme.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub background: String,
    pub foreground: String,
    pub muted: String,
    pub border: String,
    pub surface: String,
    pub code: String,
    pub accent: String,
    pub highlight: String,
    pub stroke: String,
    pub edge: String,
    pub dashed: String,
    pub grid: String,
    pub value: String,
    pub error: String,
    /// WaveDrom data fills: `=` (and `2`), then `3` to `9`.
    pub wave: [String; 8],
}

fn palette(colors: [&str; 14], wave: [&str; 8]) -> Palette {
    let [
        background,
        foreground,
        muted,
        border,
        surface,
        code,
        accent,
        highlight,
        stroke,
        edge,
        dashed,
        grid,
        value,
        error,
    ] = colors.map(str::to_owned);
    Palette {
        background,
        foreground,
        muted,
        border,
        surface,
        code,
        accent,
        highlight,
        stroke,
        edge,
        dashed,
        grid,
        value,
        error,
        wave: wave.map(str::to_owned),
    }
}

impl Palette {
    /// The built-in light colors.
    pub fn light() -> Palette {
        palette(
            [
                "#ffffff", "#1f2328", "#59636e", "#d1d9e0", "#f6f8fa", "#eff2f5", "#0969da",
                "#ddf4ff", "#424a53", "#57606a", "#8c959f", "#d0d7de", "#0550ae", "#cf222e",
            ],
            [
                "#ffffff", "#fff8c5", "#ffebe9", "#ddf4ff", "#dafbe1", "#fbefff", "#fff1e5",
                "#eaeef2",
            ],
        )
    }

    /// The built-in dark colors.
    pub fn dark() -> Palette {
        palette(
            [
                "#0d1117", "#f0f6fc", "#9198a1", "#3d444d", "#151b23", "#262c36", "#4493f8",
                "#0c2d6b", "#9198a1", "#9198a1", "#656c76", "#3d444d", "#79c0ff", "#ff7b72",
            ],
            [
                "#151b23", "#3b2e00", "#4c1d1d", "#0c2d6b", "#0f3a1d", "#3a1d4c", "#4a2a10",
                "#262c36",
            ],
        )
    }

    /// These colors with the configured ones in their place.
    fn with(mut self, colors: &ThemeColors) -> Palette {
        let slots = [
            (&mut self.background, &colors.background),
            (&mut self.foreground, &colors.foreground),
            (&mut self.muted, &colors.muted),
            (&mut self.border, &colors.border),
            (&mut self.surface, &colors.surface),
            (&mut self.code, &colors.code),
            (&mut self.accent, &colors.accent),
            (&mut self.highlight, &colors.highlight),
            (&mut self.stroke, &colors.stroke),
            (&mut self.edge, &colors.edge),
            (&mut self.dashed, &colors.dashed),
            (&mut self.grid, &colors.grid),
            (&mut self.value, &colors.value),
            (&mut self.error, &colors.error),
        ];
        for (slot, color) in slots {
            if let Some(color) = color {
                *slot = color.0.clone();
            }
        }
        if let Some(wave) = &colors.wave {
            // Fewer colors than data kinds: they repeat.
            if !wave.is_empty() {
                for (i, slot) in self.wave.iter_mut().enumerate() {
                    *slot = wave[i % wave.len()].0.clone();
                }
            }
        }
        self
    }

    /// The CSS variables of the pages.
    fn page_vars(&self) -> String {
        format!(
            "--bg:{};--fg:{};--muted:{};--line:{};--side:{};--code:{};--link:{}",
            self.background,
            self.foreground,
            self.muted,
            self.border,
            self.surface,
            self.code,
            self.accent
        )
    }

    /// The diagram rules of these colors; with `shapes`, also the widths
    /// and dashes of the lines, which the dark rules of a light and dark
    /// style sheet leave to the light ones.
    fn svg_rules(&self, shapes: bool, mono: &str) -> String {
        let p = self;
        let w = |extra: &'static str| if shapes { extra } else { "" };
        let mut out = String::new();
        let mut rule = |selector: &str, body: String| {
            out.push_str(selector);
            out.push('{');
            out.push_str(&body);
            out.push('}');
        };
        rule(
            ".vfl-box",
            format!(
                "fill:{};stroke:{}{}",
                p.surface,
                p.stroke,
                w(";stroke-width:1.5")
            ),
        );
        rule(
            ".vfl-head",
            format!(
                "fill:{};stroke:{}{}",
                p.highlight,
                p.stroke,
                w(";stroke-width:1.5")
            ),
        );
        rule(
            ".vfl-ext",
            format!(
                "fill:{};stroke:{}{}",
                p.background,
                p.dashed,
                w(";stroke-width:1.2;stroke-dasharray:4 3")
            ),
        );
        rule(
            ".vfl-sep",
            format!(
                "stroke:{}{}",
                p.dashed,
                w(";stroke-width:1;stroke-dasharray:3 3")
            ),
        );
        rule(
            ".vfl-pin",
            format!(
                "stroke:{}{};fill:{}",
                p.stroke,
                w(";stroke-width:1.5"),
                p.stroke
            ),
        );
        rule(
            ".vfl-edge",
            format!("stroke:{}{}", p.edge, w(";stroke-width:1.2;fill:none")),
        );
        rule(".vfl-arrow", format!("fill:{}", p.edge));
        rule(
            ".vfl-wire",
            format!("stroke:{}{}", p.accent, w(";stroke-width:1.3;fill:none")),
        );
        rule(".vfl-dot", format!("fill:{}", p.accent));
        rule(
            ".vfl-term",
            format!(
                "fill:{};stroke:{}{}",
                p.highlight,
                p.stroke,
                w(";stroke-width:1.2")
            ),
        );
        if shapes {
            rule(".vfl-small", "font-size:10px".into());
        }
        rule(".vfl-actual", format!("fill:{}", p.value));
        rule(
            ".vfl-cross",
            format!("stroke:{}{}", p.error, w(";stroke-width:1.5;fill:none")),
        );
        rule(".vfl-cross-text", format!("fill:{}", p.error));
        rule(
            ".vfl-grid",
            format!(
                "stroke:{}{}",
                p.grid,
                w(";stroke-width:0.8;stroke-dasharray:2 3")
            ),
        );
        rule(
            ".vfl-wave",
            format!("stroke:{}{}", p.stroke, w(";stroke-width:1.3;fill:none")),
        );
        if shapes {
            rule(".vfl-wave-x", "fill:url(#vfl-hatch)".into());
        }
        let data = ["d0", "d2", "d3", "d4", "d5", "d6", "d7", "d8", "d9"];
        for (i, d) in data.iter().enumerate() {
            rule(
                &format!(".vfl-wave-{d}"),
                format!("fill:{}", p.wave[i.saturating_sub(1)]),
            );
        }
        rule(
            ".vfl-gap",
            format!(
                "fill:{};stroke:{}{}",
                p.background,
                p.stroke,
                w(";stroke-width:1")
            ),
        );
        rule(
            ".vfl-state",
            format!(
                "fill:{};stroke:{}{}",
                p.surface,
                p.stroke,
                w(";stroke-width:1.5")
            ),
        );
        rule(
            ".vfl-initial",
            format!(
                "fill:{};stroke:{}{}",
                p.highlight,
                p.accent,
                w(";stroke-width:2")
            ),
        );
        rule(".vfl-label-bg", format!("fill:{}", p.background));
        if shapes {
            rule(
                "text",
                format!("font-family:{mono};font-size:12px;fill:{}", p.foreground),
            );
            rule(".vfl-title", "font-weight:bold".into());
        } else {
            rule("text", format!("fill:{}", p.foreground));
        }
        rule(".vfl-muted", format!("fill:{}", p.muted));
        if shapes {
            rule("a:hover text", "text-decoration:underline".into());
        }
        out
    }
}

/// A resolved theme: the built-in one with the configured changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub mode: ThemeMode,
    pub light: Palette,
    pub dark: Palette,
    pub font: String,
    pub mono_font: String,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            mode: ThemeMode::Auto,
            light: Palette::light(),
            dark: Palette::dark(),
            font: SANS.into(),
            mono_font: MONO.into(),
        }
    }
}

const DARK_MEDIA: &str = "@media (prefers-color-scheme:dark)";

/// The style sheet of the built-in theme, which every diagram is drawn with.
pub(crate) static DEFAULT_SVG_STYLE: LazyLock<String> =
    LazyLock::new(|| Theme::default().svg_style());
/// The colors and fonts of the pages of the built-in theme.
pub(crate) static DEFAULT_PAGE_VARS: LazyLock<String> =
    LazyLock::new(|| Theme::default().page_style());

impl Theme {
    /// The built-in theme changed as `config` says.
    pub fn new(config: &DocTheme) -> Theme {
        let base = Theme::default();
        Theme {
            mode: config.mode,
            light: base.light.with(&config.light),
            dark: base.dark.with(&config.dark),
            font: config.font.as_ref().map_or(base.font, |f| f.0.clone()),
            mono_font: config
                .mono_font
                .as_ref()
                .map_or(base.mono_font, |f| f.0.clone()),
        }
    }

    /// The style sheet of the diagrams.
    pub fn svg_style(&self) -> String {
        match self.mode {
            ThemeMode::Light => self.light.svg_rules(true, &self.mono_font),
            ThemeMode::Dark => self.dark.svg_rules(true, &self.mono_font),
            ThemeMode::Auto => format!(
                "{}{DARK_MEDIA}{{{}}}",
                self.light.svg_rules(true, &self.mono_font),
                self.dark.svg_rules(false, &self.mono_font)
            ),
        }
    }

    /// The rules of the page style sheet that set its colors and fonts.
    pub fn page_style(&self) -> String {
        let fonts = format!(";--font:{};--mono:{}", self.font, self.mono_font);
        match self.mode {
            ThemeMode::Light => {
                format!(
                    ":root{{color-scheme:light;{}{fonts}}}",
                    self.light.page_vars()
                )
            }
            ThemeMode::Dark => {
                format!(
                    ":root{{color-scheme:dark;{}{fonts}}}",
                    self.dark.page_vars()
                )
            }
            ThemeMode::Auto => format!(
                ":root{{color-scheme:light dark;{}{fonts}}}{DARK_MEDIA}{{:root{{{}}}}}",
                self.light.page_vars(),
                self.dark.page_vars()
            ),
        }
    }

    /// `text`, a page or a diagram drawn with the built-in theme, in this
    /// theme.
    pub fn apply<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if *self == Theme::default() {
            return Cow::Borrowed(text);
        }
        let svg = format!("<style>{}</style>", *DEFAULT_SVG_STYLE);
        let page = format!("<style>{}", *DEFAULT_PAGE_VARS);
        if !text.contains(&svg) && !text.contains(&page) {
            return Cow::Borrowed(text);
        }
        Cow::Owned(
            text.replace(&svg, &format!("<style>{}</style>", self.svg_style()))
                .replace(&page, &format!("<style>{}", self.page_style())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(toml: &str) -> Theme {
        let config = vfl_config::Config::from_toml_str(toml).unwrap();
        Theme::new(&config.doc.theme)
    }

    #[test]
    fn the_default_theme_changes_nothing() {
        let svg = crate::svg::Canvas::new().finish("t", 1.0);
        assert!(matches!(Theme::default().apply(&svg), Cow::Borrowed(_)));
        assert_eq!(theme(""), Theme::default());
    }

    #[test]
    fn a_fixed_mode_has_one_set_of_colors() {
        let dark = theme("[doc.theme]\nmode = \"dark\"\n");
        let style = dark.svg_style();
        assert!(!style.contains("prefers-color-scheme"), "{style}");
        assert!(style.contains(".vfl-box{fill:#151b23;stroke:#9198a1;stroke-width:1.5}"));
        let page = dark.page_style();
        assert!(
            page.starts_with(":root{color-scheme:dark;--bg:#0d1117;"),
            "{page}"
        );
        assert!(!page.contains("@media"), "{page}");
    }

    #[test]
    fn configured_colors_and_fonts_replace_the_built_in_ones() {
        let t = theme(
            "[doc.theme]\nmono_font = \"'Fira Code', monospace\"\nfont = \"Inter, sans-serif\"\n\
             [doc.theme.light]\naccent = \"#ff00ff\"\nwave = [\"red\", \"rgb(0, 1, 2)\"]\n\
             [doc.theme.dark]\nbackground = \"black\"\n",
        );
        assert_eq!(t.light.accent, "#ff00ff");
        assert_eq!(t.light.wave[2], "red");
        assert_eq!(t.light.wave[3], "rgb(0, 1, 2)");
        assert_eq!(t.dark.background, "black");
        assert_eq!(t.dark.accent, Palette::dark().accent);
        let svg = crate::svg::Canvas::new().finish("t", 1.0);
        let themed = t.apply(&svg);
        assert!(themed.contains(".vfl-wire{stroke:#ff00ff;"), "{themed}");
        assert!(
            themed.contains("font-family:'Fira Code', monospace;"),
            "{themed}"
        );
        assert!(themed.contains(".vfl-label-bg{fill:black}"), "{themed}");
        assert!(!themed.contains(DEFAULT_SVG_STYLE.as_str()));
        assert!(t.page_style().contains("--font:Inter, sans-serif;"));
    }

    #[test]
    fn values_that_could_end_the_style_sheet_are_refused() {
        for bad in ["red;}", "</style>", "a{", "x&y", ""] {
            let toml = format!("[doc.theme.light]\naccent = {bad:?}\n");
            assert!(vfl_config::Config::from_toml_str(&toml).is_err(), "{bad}");
        }
        assert!(vfl_config::Config::from_toml_str("[doc.theme]\nmode = \"sepia\"\n").is_err());
    }
}
