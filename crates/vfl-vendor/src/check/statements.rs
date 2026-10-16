//! The checks of statement files: Lattice Diamond preferences (`.lpf`) and
//! Gowin physical constraints (`.cst`).

use super::*;
use crate::stmt::{self, Statement};

impl Checker<'_> {
    pub(super) fn statements(&mut self) {
        let keywords = self.kind.statements();
        let parsed = stmt::parse(self.text, |w| {
            keywords.iter().any(|k| k.eq_ignore_ascii_case(w))
        });
        for error in parsed.errors {
            self.push(SYNTAX, error.range, error.message);
        }
        for statement in &parsed.statements {
            let first = statement.words[0].text(self.text);
            let Some(keyword) = keywords.iter().find(|k| k.eq_ignore_ascii_case(first)) else {
                let message = match near(first, keywords.iter().copied()) {
                    Some(meant) => format!("unknown statement `{first}`; did you mean `{meant}`?"),
                    None if self.cx.strict_commands => format!("unknown statement `{first}`"),
                    None => continue,
                };
                self.push(UNKNOWN_COMMAND, statement.words[0].range, message);
                continue;
            };
            match *keyword {
                "LOCATE" => self.lpf_locate(statement),
                "IOBUF" => self.lpf_iobuf(statement),
                "FREQUENCY" | "PERIOD" => self.lpf_frequency(statement),
                "IO_LOC" => self.cst_io_loc(statement),
                "IO_PORT" => self.cst_io_port(statement),
                _ => {}
            }
        }
    }

    /// The text of word `i`, upper-case, if it is there.
    fn upper(&self, statement: &Statement, i: usize) -> Option<String> {
        statement
            .words
            .get(i)
            .map(|w| w.text(self.text).to_ascii_uppercase())
    }

    /// `LOCATE COMP "port" SITE "A1";`
    fn lpf_locate(&mut self, statement: &Statement) {
        let text = self.text;
        let words = &statement.words;
        if words.len() < 5 {
            self.push(
                MISSING_ARGUMENT,
                statement.range,
                "`LOCATE` needs an object and where to place it",
            );
            return;
        }
        if self.upper(statement, 1).as_deref() != Some("COMP")
            || self.upper(statement, 3).as_deref() != Some("SITE")
        {
            return;
        }
        let site = words[4].text(text).to_ascii_uppercase();
        if !is_pin(&site) {
            return;
        }
        let comp = words[2];
        self.port_item(comp.inner);
        let port = comp.text(text).to_owned();
        self.assign(
            format!("LOCATE COMP {port}"),
            words[4].range,
            &format!("the site of {port}"),
            site.clone(),
        );
        self.pin(site, words[4].range, port);
    }

    /// `IOBUF PORT "port" IO_TYPE=LVCMOS33 ...;`, also `ALLPORTS` and
    /// `GROUP`.
    fn lpf_iobuf(&mut self, statement: &Statement) {
        let text = self.text;
        let words = &statement.words;
        let target = self.upper(statement, 1);
        let (object, attributes) = match target.as_deref() {
            Some("PORT" | "GROUP") if words.len() >= 3 => (Some(words[2]), &words[3..]),
            Some("ALLPORTS") => (None, &words[2..]),
            Some("PORT" | "GROUP") => {
                self.push(
                    MISSING_ARGUMENT,
                    statement.range,
                    "`IOBUF` needs the name of the port or group",
                );
                return;
            }
            _ => return,
        };
        let port = object.filter(|_| target.as_deref() == Some("PORT"));
        if let Some(port) = port {
            self.port_item(port.inner);
        }
        let owner = match object {
            Some(object) => format!("{} {}", target.unwrap_or_default(), object.text(text)),
            None => "ALLPORTS".to_owned(),
        };
        self.attributes("IOBUF", &owner, attributes, known::LPF_IOBUF);
    }

    /// `FREQUENCY PORT "clk" 100 MHZ;`, or a frequency for every clock.
    fn lpf_frequency(&mut self, statement: &Statement) {
        let words = &statement.words;
        let target = self.upper(statement, 1);
        if !matches!(target.as_deref(), Some("PORT" | "NET")) {
            return;
        }
        if words.len() < 4 {
            let keyword = words[0].text(self.text).to_ascii_uppercase();
            self.push(
                MISSING_ARGUMENT,
                statement.range,
                format!("`{keyword}` needs a name and a value"),
            );
            return;
        }
        if target.as_deref() == Some("PORT") {
            self.port_item(words[2].inner);
        }
    }

    /// `IO_LOC "port" A1;`, or `A1,B1` for a differential pair.
    fn cst_io_loc(&mut self, statement: &Statement) {
        let text = self.text;
        let words = &statement.words;
        if words.len() < 3 {
            self.push(
                MISSING_ARGUMENT,
                statement.range,
                "`IO_LOC` needs a port and a pin",
            );
            return;
        }
        let object = words[1];
        self.port_item(object.inner);
        let port = object.text(text).to_owned();
        let location = words[2..]
            .iter()
            .map(|w| w.text(text))
            .collect::<Vec<_>>()
            .join(",")
            .to_ascii_uppercase();
        self.assign(
            format!("IO_LOC {port}"),
            words[2].range,
            &format!("the pin of {port}"),
            location.clone(),
        );
        for word in &words[2..] {
            let mut start = word.inner.start();
            for pin in word.text(text).split(',') {
                let at = TextRange::at(start, (pin.len() as u32).into());
                start += vfl_syntax::TextSize::from(pin.len() as u32 + 1);
                let pin = pin.trim();
                if !pin.is_empty() {
                    self.pin(pin.to_ascii_uppercase(), at, port.clone());
                }
            }
        }
    }

    /// `IO_PORT "port" IO_TYPE=LVCMOS33 PULL_MODE=UP;`
    fn cst_io_port(&mut self, statement: &Statement) {
        let words = &statement.words;
        if words.len() < 3 {
            self.push(
                MISSING_ARGUMENT,
                statement.range,
                "`IO_PORT` needs a port and attributes",
            );
            return;
        }
        let object = words[1];
        self.port_item(object.inner);
        let owner = object.text(self.text).to_owned();
        self.attributes("IO_PORT", &owner, &words[2..], known::CST_IO_PORT);
    }

    /// Checks `KEY=VALUE` attributes of `owner` against `known` names, and
    /// for duplicates.
    fn attributes(
        &mut self,
        statement: &str,
        owner: &str,
        attributes: &[stmt::Word],
        known: &[&str],
    ) {
        let text = self.text;
        for word in attributes {
            let Some((key, value)) = word.pair(text) else {
                continue;
            };
            let upper = key.to_ascii_uppercase();
            let key_range = TextRange::at(word.range.start(), (key.len() as u32).into());
            if !known.contains(&upper.as_str()) {
                if let Some(meant) = near(&upper, known.iter().copied()) {
                    self.push(
                        MISSPELLED_NAME,
                        key_range,
                        format!("`{statement}` has no attribute `{key}`; did you mean `{meant}`?"),
                    );
                }
                continue;
            }
            self.assign(
                format!("{statement} {owner} {upper}"),
                word.range,
                &format!("{upper} of {owner}"),
                value.to_owned(),
            );
        }
    }
}
