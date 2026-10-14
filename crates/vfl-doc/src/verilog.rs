//! Verilog and SystemVerilog modules in the documentation: each module is
//! documented like an entity, with its parameters as generics, its ports
//! and the modules it instantiates.

use vfl_syntax::LineIndex;
use vfl_verilog_syntax::summary::{Direction, Module};

use crate::model::{Architecture, Entity, Instance, InstanceKind, Interface};

/// The modules of a Verilog file, as entities of `library`. `path` is the
/// file relative to the project root.
pub fn entities(modules: &[Module], text: &str, path: &str, library: &str) -> Vec<Entity> {
    let lines = LineIndex::new(text);
    modules
        .iter()
        .map(|m| {
            let line = |offset| lines.line_col(text, offset).line + 1;
            let instances: Vec<Instance> = m
                .instances
                .iter()
                .map(|i| Instance {
                    label: i.name.clone(),
                    scope: Vec::new(),
                    kind: InstanceKind::Module,
                    library: None,
                    unit: i.module.clone(),
                    architecture: None,
                    generic_map: i.parameters.clone(),
                    // `.*` names no port.
                    port_map: i
                        .connections
                        .iter()
                        .filter(|(port, _)| port != "*")
                        .cloned()
                        .collect(),
                    description: String::new(),
                })
                .collect();
            Entity {
                page: String::new(),
                name: m.name.clone(),
                library: library.to_owned(),
                file: path.to_owned(),
                line: line(m.name_range.start()),
                description: m.description.clone(),
                generics: m
                    .parameters
                    .iter()
                    .map(|p| Interface {
                        name: p.name.clone(),
                        mode: String::new(),
                        ty: p.ty.clone(),
                        default: p.default.clone(),
                        description: p.description.clone(),
                    })
                    .collect(),
                ports: m
                    .ports
                    .iter()
                    .map(|p| Interface {
                        name: p.name.clone(),
                        mode: match p.direction {
                            Some(Direction::Input) => "in",
                            Some(Direction::Output) => "out",
                            Some(Direction::Inout | Direction::Ref) | None => "inout",
                        }
                        .to_owned(),
                        ty: p.ty.clone(),
                        default: p.default.clone(),
                        description: p.description.clone(),
                    })
                    .collect(),
                architectures: if instances.is_empty() {
                    Vec::new()
                } else {
                    vec![Architecture {
                        name: m.name.clone(),
                        file: path.to_owned(),
                        line: line(m.name_range.start()),
                        instances,
                        ..Architecture::default()
                    }]
                },
                uses: Vec::new(),
                verilog: true,
            }
        })
        .collect()
}
