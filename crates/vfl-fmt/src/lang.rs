//! What the formatter needs to know about a language: how its syntax tree
//! is shaped, where its lines break, how its tokens are spaced and what
//! lines up. The engine (elements, layout, alignment, checks) is shared;
//! [`Vhdl`](vfl_syntax::Vhdl) and [`Verilog`](vfl_verilog_syntax::Verilog)
//! implement this trait. Everything is resolved at compile time.

use std::fmt::Debug;
use std::hash::Hash;

use vfl_config::{Case, FormatConfig, ListLayout, Separator};
use vfl_syntax::SyntaxError;

use crate::align::{Context, Kind};
use crate::elems::Elem;
use crate::layout::{Analysis, Line, Operator};

pub(crate) type Node<L> = rowan::SyntaxNode<L>;
pub(crate) type Token<L> = rowan::SyntaxToken<L>;
pub(crate) type Element<L> = rowan::SyntaxElement<L>;

pub(crate) trait Lang:
    rowan::Language<Kind: Copy + Eq + Hash + Debug> + Sized + 'static
{
    /// What parsing takes besides the text.
    type Std<'a>: Copy;

    const WHITESPACE: Self::Kind;
    const LPAREN: Self::Kind;
    const RPAREN: Self::Kind;
    const SEMI: Self::Kind;

    /// The tree of `text`, and its syntax errors.
    fn parse(text: &str, std: Self::Std<'_>) -> (Node<Self>, Vec<SyntaxError>);

    /// The kind and length of each token of `text`.
    fn lex(text: &str, std: Self::Std<'_>) -> Vec<(Self::Kind, usize)>;

    /// Nodes that start their own line inside a container.
    fn is_item_kind(kind: Self::Kind) -> bool;

    /// Nodes of an item kind that are part of their container's header
    /// rather than items of it.
    fn in_header(_node: &Node<Self>) -> bool {
        false
    }

    /// Nodes whose items are indented one level deeper than the node.
    fn is_container(kind: Self::Kind) -> bool;

    /// Nodes whose items stay at the node's own indentation.
    fn is_flat_container(kind: Self::Kind) -> bool;

    /// Keywords that sit at their container's indentation on their own line.
    fn is_section_keyword(kind: Self::Kind, parent: Self::Kind) -> bool;

    /// Keywords that sit at their construct's indentation when they start a
    /// line, but need not start one.
    fn is_flush_keyword(_kind: Self::Kind, _parent: Self::Kind) -> bool {
        false
    }

    /// Comments, and what is laid out like them.
    fn is_comment(kind: Self::Kind) -> bool;

    /// Tokens after which the line ends.
    fn ends_line(kind: Self::Kind) -> bool;

    /// Tokens that always start a line.
    fn starts_line(_kind: Self::Kind) -> bool {
        false
    }

    /// Tokens whose lines keep the indentation they have in the source.
    fn keeps_column(_kind: Self::Kind) -> bool {
        false
    }

    /// Nodes kept as written, as one piece.
    fn is_chunk(kind: Self::Kind) -> bool;

    /// The case a token is written in.
    fn case_of(kind: Self::Kind, config: &FormatConfig) -> Case;

    /// Whether a space goes between `prev` and `cur` on the same line.
    fn space_between(prev: &Elem<Self>, cur: &Elem<Self>, config: &FormatConfig) -> bool;

    /// Whether element `i` stays on the line of the element before it,
    /// whatever else says.
    fn stays_on_line(_elems: &[Elem<Self>], _i: usize) -> bool {
        false
    }

    /// Whether `elem` is the colon ending the label of a statement.
    fn ends_label(elem: &Elem<Self>) -> bool;

    /// Alternatives of a `case`, whose single simple statement may stay on
    /// the line of the choice.
    fn is_alternative(kind: Self::Kind) -> bool;

    fn is_simple_statement(kind: Self::Kind) -> bool;

    /// Whether statements may hold other statements outside containers
    /// (`if (a) x = 1;`), which [`nests_body`](Lang::nests_body) tells.
    const NESTS_BODIES: bool = false;

    /// Whether `child`, a statement, is the body of `parent`, indented one
    /// level deeper when it starts a line of its own.
    fn nests_body(_parent: Self::Kind, _child: Self::Kind) -> bool {
        false
    }

    /// Values whose continuation lines start under their first value.
    fn is_hanging(_kind: Self::Kind) -> bool {
        false
    }

    /// Whether `token` opens a body that moves in with it when it starts a
    /// line (a VHDL `record`).
    fn opens_body(_token: &Token<Self>, _parent: &Node<Self>) -> bool {
        false
    }

    /// When the `(` `token` opens a list that can be laid out one element
    /// per line: how its elements are laid out.
    fn opens_list(
        token: &Token<Self>,
        parent: &Node<Self>,
        config: &FormatConfig,
    ) -> Option<ListLayout>;

    /// Separators between the elements of a list.
    fn is_separator(kind: Self::Kind) -> bool;

    /// Nodes that may be lists [`opens_list`](Lang::opens_list) accepts.
    fn is_list(kind: Self::Kind) -> bool;

    /// Where the separators of `list` (the parent of the `(` that
    /// [`opens_list`](Lang::opens_list)) go.
    fn separator(list: &Node<Self>, config: &FormatConfig) -> Separator;

    /// Whether the list element starting at `elem` may stay on the line of
    /// the one before it when the list is laid out one element per line.
    fn joins_previous_element(_elem: &Elem<Self>) -> bool {
        false
    }

    /// When `token`, in `parent`, is an operator an expression may be
    /// broken at when its line is too long: how.
    fn breakable_operator(_token: &Token<Self>, _parent: &Node<Self>) -> Option<Operator<Self>> {
        None
    }

    /// Applies the case of [`FormatConfig::declaration_case`] to the
    /// identifiers of `elems` declared in `root`, and to the references to
    /// them.
    fn apply_declaration_case(
        _elems: &mut [Elem<Self>],
        _root: &Node<Self>,
        _config: &FormatConfig,
    ) {
    }

    /// The `[format.blank_lines]` rule for the gap before the line starting
    /// with element `i`, and whether the gap is above the comment lines just
    /// above that line.
    fn blank_line_rule(
        _elems: &[Elem<Self>],
        _an: &Analysis<Self>,
        _i: usize,
        _config: &vfl_config::BlankLinesConfig,
    ) -> Option<(vfl_config::BlankLines, bool)> {
        None
    }

    /// Adds or drops the optional words after `end`.
    fn edit_end_clauses(elems: Vec<Elem<Self>>, _config: &FormatConfig) -> Vec<Elem<Self>> {
        elems
    }

    /// Adds the anchors of `node`, which begins `line`, if it is a construct
    /// that has any. Returns whether it was one.
    fn anchors_of(
        node: &Node<Self>,
        elems: &[Elem<Self>],
        an: &Analysis<Self>,
        line: &Line,
        config: &vfl_config::AlignConfig,
        add: &mut impl FnMut(Kind, Context, usize),
    ) -> bool;

    /// Whether anchors in a node of this kind align together with
    /// `scope = "block"`: a process, an architecture, an interface list...
    fn is_align_block(kind: Self::Kind) -> bool;

    /// An anchor in the middle of a line (a VHDL `when`).
    fn inline_anchor(
        _elem: &Elem<Self>,
        _config: &vfl_config::AlignConfig,
    ) -> Option<(Kind, Node<Self>)> {
        None
    }

    /// The text of a line comment after its marker, for `vfl: off`.
    fn comment_body(elem: &Elem<Self>) -> Option<&str>;

    /// The text of a token as formatting must preserve it.
    fn normalize(kind: Self::Kind, text: &str) -> String;

    /// Whether `token` is an `end` whose optional words the formatter may
    /// change.
    fn starts_end_tail(_token: &Token<Self>) -> bool {
        false
    }

    /// The same, from the token kind alone.
    fn lexical_end_tail(_kind: Self::Kind) -> bool {
        false
    }

    /// Whether a part of the file can be formatted alone when it is cut
    /// inside `node`. `starts_line` tells whether a token starts its line.
    fn cuts_inside(node: &Node<Self>, starts_line: &dyn Fn(Option<Token<Self>>) -> bool) -> bool;
}
