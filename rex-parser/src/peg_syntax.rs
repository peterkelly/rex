//! Test-time parser for checked `.peg` grammar files.
//!
//! Rex source is tokenized by the parser crate, but `.peg` files are their own tiny
//! source language with different punctuation and reserved words. This module
//! supplies the `.peg` lexer, grammar, and CST-to-syntax conversion while
//! reusing the same generic PEG interpreter as the Rex parser.

use std::{collections::BTreeSet, fmt};

use rex_ast::span::Span as RexSpan;

use crate::{
    grammar::{Cst, CstNode, Grammar, GrammarParser, Item as GrammarItem, Peg, Terminal},
    peg::{Engine, EngineToken, Failure},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Span {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Span {
    fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) span: Span,
}

impl EngineToken for Token {
    fn is_eof(&self) -> bool {
        matches!(self.kind, TokenKind::Eof)
    }

    fn span(&self) -> RexSpan {
        RexSpan::new(0, self.span.start, 0, self.span.end)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TokenKind {
    Ident(String),
    String(String),
    Comment(String),
    Cut,
    Label,
    Arrow,
    Slash,
    Star,
    Plus,
    Question,
    Amp,
    Bang,
    ParenL,
    ParenR,
    Comma,
    Newline,
    Eof,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum TerminalKind {
    Ident,
    String,
    CommentLine,
    Cut,
    Label,
    Arrow,
    Slash,
    Star,
    Plus,
    Question,
    Amp,
    Bang,
    ParenL,
    ParenR,
    Comma,
    Newline,
    Eof,
}

impl TerminalKind {
    const ALL: &'static [Self] = &[
        Self::Ident,
        Self::String,
        Self::CommentLine,
        Self::Cut,
        Self::Label,
        Self::Arrow,
        Self::Slash,
        Self::Star,
        Self::Plus,
        Self::Question,
        Self::Amp,
        Self::Bang,
        Self::ParenL,
        Self::ParenR,
        Self::Comma,
        Self::Newline,
        Self::Eof,
    ];

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.peg_name() == name)
    }

    fn peg_name(self) -> &'static str {
        match self {
            Self::Ident => "IDENT",
            Self::String => "STRING",
            Self::CommentLine => "COMMENT_LINE",
            Self::Cut => "CUT",
            Self::Label => "LABEL",
            Self::Arrow => "ARROW",
            Self::Slash => "SLASH",
            Self::Star => "STAR",
            Self::Plus => "PLUS",
            Self::Question => "QUESTION",
            Self::Amp => "AMP",
            Self::Bang => "BANG",
            Self::ParenL => "PAREN_L",
            Self::ParenR => "PAREN_R",
            Self::Comma => "COMMA",
            Self::Newline => "NEWLINE",
            Self::Eof => "EOF",
        }
    }
}

impl fmt::Display for TerminalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.peg_name())
    }
}

impl Terminal<Token> for TerminalKind {
    fn label(self) -> &'static str {
        match self {
            TerminalKind::Ident => "identifier",
            TerminalKind::String => "string literal",
            TerminalKind::CommentLine => "comment",
            TerminalKind::Cut => "`cut`",
            TerminalKind::Label => "`label`",
            TerminalKind::Arrow => "`<-`",
            TerminalKind::Slash => "`/`",
            TerminalKind::Star => "`*`",
            TerminalKind::Plus => "`+`",
            TerminalKind::Question => "`?`",
            TerminalKind::Amp => "`&`",
            TerminalKind::Bang => "`!`",
            TerminalKind::ParenL => "`(`",
            TerminalKind::ParenR => "`)`",
            TerminalKind::Comma => "`,`",
            TerminalKind::Newline => "newline",
            TerminalKind::Eof => "EOF",
        }
    }

    fn matches(self, token: &Token) -> bool {
        matches!(
            (self, &token.kind),
            (TerminalKind::Ident, TokenKind::Ident(_))
                | (TerminalKind::String, TokenKind::String(_))
                | (TerminalKind::CommentLine, TokenKind::Comment(_))
                | (TerminalKind::Cut, TokenKind::Cut)
                | (TerminalKind::Label, TokenKind::Label)
                | (TerminalKind::Arrow, TokenKind::Arrow)
                | (TerminalKind::Slash, TokenKind::Slash)
                | (TerminalKind::Star, TokenKind::Star)
                | (TerminalKind::Plus, TokenKind::Plus)
                | (TerminalKind::Question, TokenKind::Question)
                | (TerminalKind::Amp, TokenKind::Amp)
                | (TerminalKind::Bang, TokenKind::Bang)
                | (TerminalKind::ParenL, TokenKind::ParenL)
                | (TerminalKind::ParenR, TokenKind::ParenR)
                | (TerminalKind::Comma, TokenKind::Comma)
                | (TerminalKind::Newline, TokenKind::Newline)
                | (TerminalKind::Eof, TokenKind::Eof)
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum RuleKind {
    Grammar,
    Item,
    Comment,
    Rule,
    Choice,
    Sequence,
    Prefix,
    Postfix,
    Atom,
    CutExpr,
    LabelExpr,
    Name,
    Group,
}

impl RuleKind {
    const ALL: &'static [Self] = &[
        Self::Grammar,
        Self::Item,
        Self::Comment,
        Self::Rule,
        Self::Choice,
        Self::Sequence,
        Self::Prefix,
        Self::Postfix,
        Self::Atom,
        Self::CutExpr,
        Self::LabelExpr,
        Self::Name,
        Self::Group,
    ];

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|rule| rule.name() == name)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Grammar => "Grammar",
            Self::Item => "Item",
            Self::Comment => "Comment",
            Self::Rule => "Rule",
            Self::Choice => "Choice",
            Self::Sequence => "Sequence",
            Self::Prefix => "Prefix",
            Self::Postfix => "Postfix",
            Self::Atom => "Atom",
            Self::CutExpr => "CutExpr",
            Self::LabelExpr => "LabelExpr",
            Self::Name => "Name",
            Self::Group => "Group",
        }
    }
}

impl fmt::Display for RuleKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

type ParserGrammar = Grammar<RuleKind, TerminalKind>;
type ParserPeg = Peg<RuleKind, TerminalKind>;
type ParserCstNode = CstNode<RuleKind, Token>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LexError {
    pub(crate) span: Span,
    pub(crate) message: String,
}

impl LexError {
    fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

pub(crate) fn lex(source: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(source).lex()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SyntaxGrammar {
    pub(crate) items: Vec<SyntaxItem>,
}

impl SyntaxGrammar {
    pub(crate) fn rules(&self) -> impl DoubleEndedIterator<Item = &SyntaxRule> {
        self.items.iter().filter_map(|item| match item {
            SyntaxItem::Rule(rule) => Some(rule),
            SyntaxItem::Comment(_) => None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SyntaxItem {
    Rule(SyntaxRule),
    Comment(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SyntaxRule {
    pub(crate) name: String,
    pub(crate) expression: SyntaxExpr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SyntaxExpr {
    Name(String),
    Seq(Vec<SyntaxExpr>),
    Choice(Vec<SyntaxExpr>),
    Optional(Box<SyntaxExpr>),
    Repeat(Box<SyntaxExpr>),
    Repeat1(Box<SyntaxExpr>),
    And(Box<SyntaxExpr>),
    Not(Box<SyntaxExpr>),
    Cut(Box<SyntaxExpr>),
    Label(String, Box<SyntaxExpr>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParseError {
    pub(crate) span: Span,
    pub(crate) message: String,
}

impl ParseError {
    fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }

    fn from_lex(err: LexError) -> Self {
        Self {
            span: err.span,
            message: err.message,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GrammarLoadError {
    message: String,
}

impl GrammarLoadError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub(crate) fn parse(source: &str) -> Result<SyntaxGrammar, ParseError> {
    let tokens = lex(source).map_err(ParseError::from_lex)?;
    let (eof, tokens) = tokens
        .split_last()
        .expect("PEG syntax lexer always emits EOF");
    let grammar = parser_grammar();
    let mut engine = Engine::new(tokens, eof.clone());
    let mut parser = GrammarParser::new(&grammar, &mut engine);
    let cst = parser.parse_start().map_err(parse_error_from_failure)?;

    Ok(syntax_grammar_from_cst(&cst))
}

// Self-description of the accepted `.peg` syntax. The test below parses this
// file through the generic PEG interpreter and compares it with
// `peg_syntax_grammar()`, so parser-grammar changes must update the checked
// grammar specification as well.
pub(crate) const PEG_PEG_GRAMMAR: &str = include_str!("peg.peg");

pub(crate) fn peg_syntax_grammar() -> SyntaxGrammar {
    use TerminalKind as T;

    SyntaxGrammar {
        items: vec![
            SyntaxItem::Rule(syntax_rule(
                "Grammar",
                seq([
                    rep(terminal(T::Newline)),
                    name("Item"),
                    rep(seq([rep1(terminal(T::Newline)), name("Item")])),
                    rep(terminal(T::Newline)),
                    terminal(T::Eof),
                ]),
            )),
            SyntaxItem::Rule(syntax_rule("Item", choice([name("Comment"), name("Rule")]))),
            SyntaxItem::Rule(syntax_rule(
                "Comment",
                seq([
                    terminal(T::CommentLine),
                    rep(seq([terminal(T::Newline), terminal(T::CommentLine)])),
                ]),
            )),
            SyntaxItem::Rule(syntax_rule(
                "Rule",
                seq([terminal(T::Ident), terminal(T::Arrow), name("Choice")]),
            )),
            SyntaxItem::Rule(syntax_rule(
                "Choice",
                seq([
                    name("Sequence"),
                    rep(seq([terminal(T::Slash), name("Sequence")])),
                ]),
            )),
            SyntaxItem::Rule(syntax_rule("Sequence", rep1(name("Prefix")))),
            SyntaxItem::Rule(syntax_rule(
                "Prefix",
                choice([
                    seq([terminal(T::Amp), name("Prefix")]),
                    seq([terminal(T::Bang), name("Prefix")]),
                    name("Postfix"),
                ]),
            )),
            SyntaxItem::Rule(syntax_rule(
                "Postfix",
                seq([
                    name("Atom"),
                    rep(choice([
                        terminal(T::Question),
                        terminal(T::Star),
                        terminal(T::Plus),
                    ])),
                ]),
            )),
            SyntaxItem::Rule(syntax_rule(
                "Atom",
                choice([
                    name("CutExpr"),
                    name("LabelExpr"),
                    name("Name"),
                    name("Group"),
                ]),
            )),
            SyntaxItem::Rule(syntax_rule(
                "CutExpr",
                seq([
                    terminal(T::Cut),
                    terminal(T::ParenL),
                    name("Choice"),
                    terminal(T::ParenR),
                ]),
            )),
            SyntaxItem::Rule(syntax_rule(
                "LabelExpr",
                seq([
                    terminal(T::Label),
                    terminal(T::ParenL),
                    terminal(T::String),
                    terminal(T::Comma),
                    name("Choice"),
                    terminal(T::ParenR),
                ]),
            )),
            SyntaxItem::Rule(syntax_rule("Name", terminal(T::Ident))),
            SyntaxItem::Rule(syntax_rule(
                "Group",
                seq([terminal(T::ParenL), name("Choice"), terminal(T::ParenR)]),
            )),
        ],
    }
}

fn syntax_rule(name: &str, expression: SyntaxExpr) -> SyntaxRule {
    SyntaxRule {
        name: name.to_string(),
        expression,
    }
}

fn name(name: &str) -> SyntaxExpr {
    SyntaxExpr::Name(name.to_string())
}

fn terminal(kind: TerminalKind) -> SyntaxExpr {
    name(kind.peg_name())
}

fn seq(items: impl IntoIterator<Item = SyntaxExpr>) -> SyntaxExpr {
    let mut items = items.into_iter().collect::<Vec<_>>();

    match items.len() {
        0 => unreachable!("PEG syntax grammar should not contain empty sequences"),
        1 => items.remove(0),
        _ => SyntaxExpr::Seq(items),
    }
}

fn choice(items: impl IntoIterator<Item = SyntaxExpr>) -> SyntaxExpr {
    let mut items = items.into_iter().collect::<Vec<_>>();

    match items.len() {
        0 => unreachable!("PEG syntax grammar should not contain empty choices"),
        1 => items.remove(0),
        _ => SyntaxExpr::Choice(items),
    }
}

fn rep(item: SyntaxExpr) -> SyntaxExpr {
    SyntaxExpr::Repeat(Box::new(item))
}

fn rep1(item: SyntaxExpr) -> SyntaxExpr {
    SyntaxExpr::Repeat1(Box::new(item))
}

fn parser_grammar_from_syntax(syntax: &SyntaxGrammar) -> Result<ParserGrammar, GrammarLoadError> {
    let first_rule = syntax
        .rules()
        .next()
        .ok_or_else(|| GrammarLoadError::new("expected rule definition"))?;
    let start = resolve_rule_name(&first_rule.name)?;
    let expected = parser_grammar();

    if start != expected.start() {
        return Err(GrammarLoadError::new(format!(
            "expected start rule `{}`, found `{}`",
            expected.start(),
            start
        )));
    }

    let mut seen = BTreeSet::new();
    let mut items = Vec::with_capacity(syntax.items.len());

    for item in &syntax.items {
        match item {
            SyntaxItem::Rule(syntax_rule) => {
                let rule = resolve_rule_name(&syntax_rule.name)?;
                if !seen.insert(rule) {
                    return Err(GrammarLoadError::new(format!(
                        "duplicate rule definition `{}`",
                        rule
                    )));
                }

                items.push(GrammarItem::Rule(
                    rule,
                    resolve_expr(&syntax_rule.expression)?,
                ));
            }
            SyntaxItem::Comment(comment) => {
                items.push(GrammarItem::Comment(comment.clone()));
            }
        }
    }

    for (expected_rule, _) in expected.rules() {
        if !seen.contains(&expected_rule) {
            return Err(GrammarLoadError::new(format!(
                "missing rule definition `{}`",
                expected_rule
            )));
        }
    }

    Ok(Grammar::from_items(start, items))
}

fn resolve_rule_name(name: &str) -> Result<RuleKind, GrammarLoadError> {
    RuleKind::from_name(name)
        .ok_or_else(|| GrammarLoadError::new(format!("unknown rule name `{name}`")))
}

fn resolve_expr(expression: &SyntaxExpr) -> Result<ParserPeg, GrammarLoadError> {
    match expression {
        SyntaxExpr::Name(name) => resolve_symbol(name),
        SyntaxExpr::Seq(items) => resolve_sequence(items),
        SyntaxExpr::Choice(items) => resolve_choice(items),
        SyntaxExpr::Optional(item) => Ok(Peg::Optional(Box::new(resolve_expr(item)?))),
        SyntaxExpr::Repeat(item) => Ok(Peg::Repeat(Box::new(resolve_expr(item)?))),
        SyntaxExpr::Repeat1(item) => Ok(Peg::Repeat1(Box::new(resolve_expr(item)?))),
        SyntaxExpr::And(item) => Ok(Peg::And(Box::new(resolve_expr(item)?))),
        SyntaxExpr::Not(item) => Ok(Peg::Not(Box::new(resolve_expr(item)?))),
        SyntaxExpr::Cut(item) => Ok(Peg::Cut(Box::new(resolve_expr(item)?))),
        SyntaxExpr::Label(message, item) => {
            Ok(Peg::Label(message.clone(), Box::new(resolve_expr(item)?)))
        }
    }
}

fn resolve_symbol(name: &str) -> Result<ParserPeg, GrammarLoadError> {
    match (RuleKind::from_name(name), TerminalKind::from_name(name)) {
        (Some(_), Some(_)) => Err(GrammarLoadError::new(format!(
            "ambiguous grammar symbol `{name}` resolves as both rule and token"
        ))),
        (Some(rule), None) => Ok(Peg::Rule(rule)),
        (None, Some(kind)) => Ok(Peg::Token(kind)),
        (None, None) => Err(GrammarLoadError::new(format!(
            "unknown grammar symbol `{name}`"
        ))),
    }
}

fn resolve_sequence(items: &[SyntaxExpr]) -> Result<ParserPeg, GrammarLoadError> {
    let mut resolved = Vec::new();

    for item in items {
        match resolve_expr(item)? {
            Peg::Seq(mut nested) => resolved.append(&mut nested),
            item => resolved.push(item),
        }
    }

    Ok(match resolved.len() {
        0 => unreachable!("parser never produces empty sequences"),
        1 => resolved.remove(0),
        _ => Peg::Seq(resolved),
    })
}

fn resolve_choice(items: &[SyntaxExpr]) -> Result<ParserPeg, GrammarLoadError> {
    let mut resolved = Vec::new();

    for item in items {
        match resolve_expr(item)? {
            Peg::Choice(mut nested) => resolved.append(&mut nested),
            item => resolved.push(item),
        }
    }

    Ok(match resolved.len() {
        0 => unreachable!("parser never produces empty choices"),
        1 => resolved.remove(0),
        _ => Peg::Choice(resolved),
    })
}

fn parser_grammar() -> ParserGrammar {
    use RuleKind as R;
    use TerminalKind as T;

    Grammar::new(
        R::Grammar,
        [
            (
                R::Grammar,
                p_seq([
                    p_rep(p_tok(T::Newline)),
                    p_rule(R::Item),
                    p_rep(p_seq([p_rep1(p_tok(T::Newline)), p_rule(R::Item)])),
                    p_rep(p_tok(T::Newline)),
                    p_tok(T::Eof),
                ]),
            ),
            (R::Item, p_choice([p_rule(R::Comment), p_rule(R::Rule)])),
            (
                R::Comment,
                p_seq([
                    p_tok(T::CommentLine),
                    p_rep(p_seq([p_tok(T::Newline), p_tok(T::CommentLine)])),
                ]),
            ),
            (
                R::Rule,
                p_seq([p_tok(T::Ident), p_tok(T::Arrow), p_rule(R::Choice)]),
            ),
            (
                R::Choice,
                p_seq([
                    p_rule(R::Sequence),
                    p_rep(p_seq([p_tok(T::Slash), p_rule(R::Sequence)])),
                ]),
            ),
            (R::Sequence, p_rep1(p_rule(R::Prefix))),
            (
                R::Prefix,
                p_choice([
                    p_seq([p_tok(T::Amp), p_rule(R::Prefix)]),
                    p_seq([p_tok(T::Bang), p_rule(R::Prefix)]),
                    p_rule(R::Postfix),
                ]),
            ),
            (
                R::Postfix,
                p_seq([
                    p_rule(R::Atom),
                    p_rep(p_choice([
                        p_tok(T::Question),
                        p_tok(T::Star),
                        p_tok(T::Plus),
                    ])),
                ]),
            ),
            (
                R::Atom,
                p_choice([
                    p_rule(R::CutExpr),
                    p_rule(R::LabelExpr),
                    p_rule(R::Name),
                    p_rule(R::Group),
                ]),
            ),
            (
                R::CutExpr,
                p_seq([
                    p_tok(T::Cut),
                    p_tok(T::ParenL),
                    p_rule(R::Choice),
                    p_tok(T::ParenR),
                ]),
            ),
            (
                R::LabelExpr,
                p_seq([
                    p_tok(T::Label),
                    p_tok(T::ParenL),
                    p_tok(T::String),
                    p_tok(T::Comma),
                    p_rule(R::Choice),
                    p_tok(T::ParenR),
                ]),
            ),
            (R::Name, p_tok(T::Ident)),
            (
                R::Group,
                p_seq([p_tok(T::ParenL), p_rule(R::Choice), p_tok(T::ParenR)]),
            ),
        ],
    )
}

fn p_tok(kind: TerminalKind) -> ParserPeg {
    Peg::Token(kind)
}

fn p_rule(rule: RuleKind) -> ParserPeg {
    Peg::Rule(rule)
}

fn p_seq(items: impl IntoIterator<Item = ParserPeg>) -> ParserPeg {
    Peg::Seq(items.into_iter().collect())
}

fn p_choice(items: impl IntoIterator<Item = ParserPeg>) -> ParserPeg {
    Peg::Choice(items.into_iter().collect())
}

fn p_rep(item: ParserPeg) -> ParserPeg {
    Peg::Repeat(Box::new(item))
}

fn p_rep1(item: ParserPeg) -> ParserPeg {
    Peg::Repeat1(Box::new(item))
}

fn syntax_grammar_from_cst(node: &ParserCstNode) -> SyntaxGrammar {
    debug_assert_eq!(node.rule, RuleKind::Grammar);
    SyntaxGrammar {
        items: child_rules(node, RuleKind::Item)
            .map(syntax_item_from_cst)
            .collect(),
    }
}

fn syntax_item_from_cst(node: &ParserCstNode) -> SyntaxItem {
    debug_assert_eq!(node.rule, RuleKind::Item);

    if let Some(comment) = first_rule(node, RuleKind::Comment) {
        SyntaxItem::Comment(comment_from_cst(comment))
    } else {
        SyntaxItem::Rule(syntax_rule_from_cst(expect_rule(node, RuleKind::Rule)))
    }
}

fn comment_from_cst(node: &ParserCstNode) -> String {
    debug_assert_eq!(node.rule, RuleKind::Comment);

    node.children
        .iter()
        .filter_map(|child| match child {
            Cst::Token(Token {
                kind: TokenKind::Comment(line),
                ..
            }) => Some(line.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn syntax_rule_from_cst(node: &ParserCstNode) -> SyntaxRule {
    debug_assert_eq!(node.rule, RuleKind::Rule);
    SyntaxRule {
        name: first_ident(node),
        expression: choice_from_cst(expect_rule(node, RuleKind::Choice)),
    }
}

fn choice_from_cst(node: &ParserCstNode) -> SyntaxExpr {
    debug_assert_eq!(node.rule, RuleKind::Choice);
    let alternatives = child_rules(node, RuleKind::Sequence)
        .map(sequence_from_cst)
        .collect::<Vec<_>>();

    choice(alternatives)
}

fn sequence_from_cst(node: &ParserCstNode) -> SyntaxExpr {
    debug_assert_eq!(node.rule, RuleKind::Sequence);
    let items = child_rules(node, RuleKind::Prefix)
        .map(prefix_from_cst)
        .collect::<Vec<_>>();

    seq(items)
}

fn prefix_from_cst(node: &ParserCstNode) -> SyntaxExpr {
    debug_assert_eq!(node.rule, RuleKind::Prefix);

    if has_token(node, |kind| matches!(kind, TokenKind::Amp)) {
        SyntaxExpr::And(Box::new(prefix_from_cst(expect_rule(
            node,
            RuleKind::Prefix,
        ))))
    } else if has_token(node, |kind| matches!(kind, TokenKind::Bang)) {
        SyntaxExpr::Not(Box::new(prefix_from_cst(expect_rule(
            node,
            RuleKind::Prefix,
        ))))
    } else {
        postfix_from_cst(expect_rule(node, RuleKind::Postfix))
    }
}

fn postfix_from_cst(node: &ParserCstNode) -> SyntaxExpr {
    debug_assert_eq!(node.rule, RuleKind::Postfix);
    let mut expression = atom_from_cst(expect_rule(node, RuleKind::Atom));

    for child in &node.children {
        expression = match child {
            Cst::Token(Token {
                kind: TokenKind::Question,
                ..
            }) => SyntaxExpr::Optional(Box::new(expression)),
            Cst::Token(Token {
                kind: TokenKind::Star,
                ..
            }) => SyntaxExpr::Repeat(Box::new(expression)),
            Cst::Token(Token {
                kind: TokenKind::Plus,
                ..
            }) => SyntaxExpr::Repeat1(Box::new(expression)),
            _ => expression,
        };
    }

    expression
}

fn atom_from_cst(node: &ParserCstNode) -> SyntaxExpr {
    debug_assert_eq!(node.rule, RuleKind::Atom);

    if let Some(child) = first_rule(node, RuleKind::CutExpr) {
        SyntaxExpr::Cut(Box::new(choice_from_cst(expect_rule(
            child,
            RuleKind::Choice,
        ))))
    } else if let Some(child) = first_rule(node, RuleKind::LabelExpr) {
        SyntaxExpr::Label(
            first_string(child),
            Box::new(choice_from_cst(expect_rule(child, RuleKind::Choice))),
        )
    } else if let Some(child) = first_rule(node, RuleKind::Name) {
        SyntaxExpr::Name(first_ident(child))
    } else {
        choice_from_cst(expect_rule(
            expect_rule(node, RuleKind::Group),
            RuleKind::Choice,
        ))
    }
}

fn child_rules(
    node: &ParserCstNode,
    rule: RuleKind,
) -> impl DoubleEndedIterator<Item = &ParserCstNode> {
    node.children.iter().filter_map(move |child| match child {
        Cst::Node(child) if child.rule == rule => Some(child.as_ref()),
        _ => None,
    })
}

fn first_rule(node: &ParserCstNode, rule: RuleKind) -> Option<&ParserCstNode> {
    child_rules(node, rule).next()
}

fn expect_rule(node: &ParserCstNode, rule: RuleKind) -> &ParserCstNode {
    first_rule(node, rule).expect("PEG syntax CST matched the grammar")
}

fn first_ident(node: &ParserCstNode) -> String {
    node.children
        .iter()
        .find_map(|child| match child {
            Cst::Token(Token {
                kind: TokenKind::Ident(name),
                ..
            }) => Some(name.clone()),
            _ => None,
        })
        .expect("PEG syntax CST contains an identifier")
}

fn first_string(node: &ParserCstNode) -> String {
    node.children
        .iter()
        .find_map(|child| match child {
            Cst::Token(Token {
                kind: TokenKind::String(value),
                ..
            }) => Some(value.clone()),
            _ => None,
        })
        .expect("PEG syntax CST contains a string literal")
}

fn has_token(node: &ParserCstNode, predicate: impl Fn(&TokenKind) -> bool) -> bool {
    node.children.iter().any(|child| match child {
        Cst::Token(token) => predicate(&token.kind),
        _ => false,
    })
}

fn parse_error_from_failure(failure: Failure) -> ParseError {
    let message = if is_expression_start_failure(&failure) {
        "expected expression".to_string()
    } else if failure.expected.len() == 1 {
        format!(
            "expected {}",
            failure.expected.iter().next().expect("checked len")
        )
    } else {
        format!(
            "expected one of {}",
            failure
                .expected
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    ParseError::new(
        Span::new(failure.span.begin.column, failure.span.end.column),
        message,
    )
}

fn is_expression_start_failure(failure: &Failure) -> bool {
    const EXPRESSION_STARTS: &[&str] = &["`&`", "`!`", "`(`", "`cut`", "`label`", "identifier"];

    failure.expected.len() > 1
        && failure
            .expected
            .iter()
            .all(|expected| EXPRESSION_STARTS.contains(&expected.as_str()))
}

struct Lexer<'source> {
    source: &'source str,
    cursor: usize,
    line_has_code: bool,
    tokens: Vec<Token>,
}

impl<'source> Lexer<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source,
            cursor: 0,
            line_has_code: false,
            tokens: Vec::new(),
        }
    }

    fn lex(mut self) -> Result<Vec<Token>, LexError> {
        while let Some((start, ch)) = self.peek() {
            match ch {
                ' ' | '\t' => {
                    self.bump();
                }
                '\r' | '\n' => {
                    let end = self.bump_newline();
                    self.push(TokenKind::Newline, start, end);
                    self.line_has_code = false;
                }
                '#' => {
                    if self.line_has_code {
                        self.skip_comment();
                    } else {
                        let token = self.comment(start);
                        self.tokens.push(token);
                        self.line_has_code = true;
                    }
                }
                '<' => {
                    self.bump();
                    if self.consume('-') {
                        self.push(TokenKind::Arrow, start, self.cursor);
                    } else {
                        return Err(LexError::new(
                            Span::new(start, self.cursor),
                            "expected `<-`",
                        ));
                    }
                }
                '/' => {
                    self.bump();
                    self.push(TokenKind::Slash, start, self.cursor);
                }
                '*' => {
                    self.bump();
                    self.push(TokenKind::Star, start, self.cursor);
                }
                '+' => {
                    self.bump();
                    self.push(TokenKind::Plus, start, self.cursor);
                }
                '?' => {
                    self.bump();
                    self.push(TokenKind::Question, start, self.cursor);
                }
                '&' => {
                    self.bump();
                    self.push(TokenKind::Amp, start, self.cursor);
                }
                '!' => {
                    self.bump();
                    self.push(TokenKind::Bang, start, self.cursor);
                }
                '(' => {
                    self.bump();
                    self.push(TokenKind::ParenL, start, self.cursor);
                }
                ')' => {
                    self.bump();
                    self.push(TokenKind::ParenR, start, self.cursor);
                }
                ',' => {
                    self.bump();
                    self.push(TokenKind::Comma, start, self.cursor);
                }
                '"' => {
                    let token = self.string_literal(start)?;
                    self.tokens.push(token);
                }
                _ if is_ident_start(ch) => {
                    let token = self.identifier(start);
                    self.tokens.push(token);
                }
                _ => {
                    self.bump();
                    return Err(LexError::new(
                        Span::new(start, self.cursor),
                        format!("unexpected character {ch:?}"),
                    ));
                }
            }
        }

        self.push(TokenKind::Eof, self.cursor, self.cursor);
        Ok(self.tokens)
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        if !matches!(kind, TokenKind::Newline) {
            self.line_has_code = true;
        }
        self.tokens.push(Token {
            kind,
            span: Span::new(start, end),
        });
    }

    fn identifier(&mut self, start: usize) -> Token {
        self.bump();
        while let Some((_, ch)) = self.peek()
            && is_ident_continue(ch)
        {
            self.bump();
        }

        let text = &self.source[start..self.cursor];
        // These are PEG syntax constructors, not grammar symbols. Reserving
        // them lets `peg.peg` describe function forms with ordinary tokens
        // instead of relying on parser-side string comparisons.
        let kind = match text {
            "cut" => TokenKind::Cut,
            "label" => TokenKind::Label,
            _ => TokenKind::Ident(text.to_string()),
        };

        Token {
            kind,
            span: Span::new(start, self.cursor),
        }
    }

    fn comment(&mut self, start: usize) -> Token {
        self.bump();
        let text_start = if self.peek().is_some_and(|(_, ch)| ch == ' ') {
            self.bump();
            self.cursor
        } else {
            self.cursor
        };

        while let Some((_, ch)) = self.peek() {
            if ch == '\r' || ch == '\n' {
                break;
            }
            self.bump();
        }

        Token {
            kind: TokenKind::Comment(self.source[text_start..self.cursor].to_string()),
            span: Span::new(start, self.cursor),
        }
    }

    fn string_literal(&mut self, start: usize) -> Result<Token, LexError> {
        self.bump();
        let mut value = String::new();

        loop {
            let Some((ch_start, ch)) = self.peek() else {
                return Err(LexError::new(
                    Span::new(start, self.cursor),
                    "unterminated string literal",
                ));
            };

            match ch {
                '"' => {
                    self.bump();
                    return Ok(Token {
                        kind: TokenKind::String(value),
                        span: Span::new(start, self.cursor),
                    });
                }
                '\\' => {
                    self.bump();
                    value.push(self.escape_sequence(ch_start)?);
                }
                '\r' | '\n' => {
                    return Err(LexError::new(
                        Span::new(start, ch_start),
                        "unterminated string literal",
                    ));
                }
                _ if ch.is_control() => {
                    self.bump();
                    return Err(LexError::new(
                        Span::new(ch_start, self.cursor),
                        "control character in string literal",
                    ));
                }
                _ => {
                    self.bump();
                    value.push(ch);
                }
            }
        }
    }

    fn escape_sequence(&mut self, slash_start: usize) -> Result<char, LexError> {
        let Some((escape_start, ch)) = self.peek() else {
            return Err(LexError::new(
                Span::new(slash_start, self.cursor),
                "unterminated escape sequence",
            ));
        };

        self.bump();
        match ch {
            '"' => Ok('"'),
            '\\' => Ok('\\'),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            '0' => Ok('\0'),
            'u' => self.unicode_escape(slash_start),
            _ => Err(LexError::new(
                Span::new(escape_start, self.cursor),
                format!("unsupported escape sequence `\\{ch}`"),
            )),
        }
    }

    fn unicode_escape(&mut self, slash_start: usize) -> Result<char, LexError> {
        if !self.consume('{') {
            return Err(LexError::new(
                Span::new(slash_start, self.cursor),
                "expected `{` after `\\u`",
            ));
        }

        let digits_start = self.cursor;
        let mut value = 0_u32;
        let mut digits = 0;

        while let Some((_, ch)) = self.peek() {
            if ch == '}' {
                break;
            }

            let Some(digit) = ch.to_digit(16) else {
                self.bump();
                return Err(LexError::new(
                    Span::new(digits_start, self.cursor),
                    "expected hexadecimal digit in unicode escape",
                ));
            };

            digits += 1;
            if digits > 6 {
                self.bump();
                return Err(LexError::new(
                    Span::new(digits_start, self.cursor),
                    "unicode escape has more than 6 digits",
                ));
            }

            value = (value << 4) | digit;
            self.bump();
        }

        if digits == 0 {
            return Err(LexError::new(
                Span::new(digits_start, self.cursor),
                "unicode escape requires at least one digit",
            ));
        }

        if !self.consume('}') {
            return Err(LexError::new(
                Span::new(slash_start, self.cursor),
                "unterminated unicode escape",
            ));
        }

        char::from_u32(value).ok_or_else(|| {
            LexError::new(
                Span::new(slash_start, self.cursor),
                "unicode escape is not a valid scalar value",
            )
        })
    }

    fn skip_comment(&mut self) {
        while let Some((_, ch)) = self.peek() {
            if ch == '\r' || ch == '\n' {
                break;
            }
            self.bump();
        }
    }

    fn bump_newline(&mut self) -> usize {
        let Some((_, ch)) = self.peek() else {
            return self.cursor;
        };

        self.bump();
        if ch == '\r' {
            self.consume('\n');
        }
        self.cursor
    }

    fn consume(&mut self, expected: char) -> bool {
        if self.peek().is_some_and(|(_, ch)| ch == expected) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<(usize, char)> {
        self.source[self.cursor..]
            .chars()
            .next()
            .map(|ch| (self.cursor, ch))
    }

    fn bump(&mut self) -> Option<(usize, char)> {
        let (start, ch) = self.peek()?;
        self.cursor += ch.len_utf8();
        Some((start, ch))
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Result<Vec<TokenKind>, LexError> {
        lex(source).map(|tokens| tokens.into_iter().map(|token| token.kind).collect())
    }

    #[test]
    fn terminal_names_are_upper_snake_case() {
        assert_eq!(TerminalKind::ParenL.to_string(), "PAREN_L");
        assert_eq!(
            TerminalKind::from_name("PAREN_L"),
            Some(TerminalKind::ParenL)
        );
        assert_eq!(TerminalKind::from_name("ParenL"), None);
    }

    #[test]
    fn lexes_canonical_peg_syntax() {
        assert_eq!(
            kinds("Program <- Decl* Expr? EOF\n").unwrap(),
            vec![
                TokenKind::Ident("Program".to_string()),
                TokenKind::Arrow,
                TokenKind::Ident("Decl".to_string()),
                TokenKind::Star,
                TokenKind::Ident("Expr".to_string()),
                TokenKind::Question,
                TokenKind::Ident("EOF".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_function_forms_and_lookahead() {
        assert_eq!(
            kinds("Rule <- cut(label(\"x\", &(IDENT / INT)))\n").unwrap(),
            vec![
                TokenKind::Ident("Rule".to_string()),
                TokenKind::Arrow,
                TokenKind::Cut,
                TokenKind::ParenL,
                TokenKind::Label,
                TokenKind::ParenL,
                TokenKind::String("x".to_string()),
                TokenKind::Comma,
                TokenKind::Amp,
                TokenKind::ParenL,
                TokenKind::Ident("IDENT".to_string()),
                TokenKind::Slash,
                TokenKind::Ident("INT".to_string()),
                TokenKind::ParenR,
                TokenKind::ParenR,
                TokenKind::ParenR,
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn skips_spaces_tabs_and_comments_but_preserves_newlines() {
        assert_eq!(
            kinds("  A\t<- B # comment\n# whole-line comment\nC <- !D\n").unwrap(),
            vec![
                TokenKind::Ident("A".to_string()),
                TokenKind::Arrow,
                TokenKind::Ident("B".to_string()),
                TokenKind::Newline,
                TokenKind::Comment("whole-line comment".to_string()),
                TokenKind::Newline,
                TokenKind::Ident("C".to_string()),
                TokenKind::Arrow,
                TokenKind::Bang,
                TokenKind::Ident("D".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn decodes_string_escapes() {
        assert_eq!(
            kinds("\"quote: \\\" slash: \\\\ newline: \\n unicode: \\u{3bb}\"").unwrap(),
            vec![
                TokenKind::String("quote: \" slash: \\ newline: \n unicode: λ".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn reports_bad_characters() {
        let err = lex("A <- 'bad'").unwrap_err();

        assert_eq!(err.span, Span::new(5, 6));
        assert!(err.message.contains("unexpected character"));
    }

    #[test]
    fn reports_unterminated_strings() {
        let err = lex("A <- \"bad").unwrap_err();

        assert_eq!(err.span, Span::new(5, 9));
        assert_eq!(err.message, "unterminated string literal");
    }

    #[test]
    fn reports_invalid_unicode_escape() {
        let err = lex("\"\\u{110000}\"").unwrap_err();

        assert_eq!(err.message, "unicode escape is not a valid scalar value");
    }

    #[test]
    fn parses_sequence_choice_prefix_postfix_and_grouping() {
        assert_eq!(
            parse("Start <- (IDENT / INT) (STRING BOOL)? (&FLOAT)* &QUESTION?\n").unwrap(),
            SyntaxGrammar {
                items: vec![SyntaxItem::Rule(SyntaxRule {
                    name: "Start".to_string(),
                    expression: SyntaxExpr::Seq(vec![
                        SyntaxExpr::Choice(vec![name("IDENT"), name("INT")]),
                        SyntaxExpr::Optional(Box::new(SyntaxExpr::Seq(vec![
                            name("STRING"),
                            name("BOOL")
                        ]))),
                        SyntaxExpr::Repeat(Box::new(SyntaxExpr::And(Box::new(name("FLOAT"))))),
                        SyntaxExpr::And(Box::new(SyntaxExpr::Optional(Box::new(name("QUESTION"))))),
                    ]),
                })]
            }
        );
    }

    #[test]
    fn parses_cut_and_label_function_forms() {
        assert_eq!(
            parse("Start <- cut(label(\"expected \\\"thing\\\"\\n\", IDENT SEMI_COLON))\n")
                .unwrap(),
            SyntaxGrammar {
                items: vec![SyntaxItem::Rule(SyntaxRule {
                    name: "Start".to_string(),
                    expression: SyntaxExpr::Cut(Box::new(SyntaxExpr::Label(
                        "expected \"thing\"\n".to_string(),
                        Box::new(SyntaxExpr::Seq(vec![name("IDENT"), name("SEMI_COLON")]))
                    ))),
                })]
            }
        );
    }

    #[test]
    fn parses_blank_lines_and_optional_final_newline() {
        assert_eq!(
            parse("\n\nStart <- A\n\nTerm <- B").unwrap(),
            SyntaxGrammar {
                items: vec![
                    SyntaxItem::Rule(SyntaxRule {
                        name: "Start".to_string(),
                        expression: name("A"),
                    }),
                    SyntaxItem::Rule(SyntaxRule {
                        name: "Term".to_string(),
                        expression: name("B"),
                    }),
                ]
            }
        );
    }

    #[test]
    fn parses_comment_blocks_without_losing_text() {
        assert_eq!(
            parse("# Heading\n#   detail\n\nStart <- A\n").unwrap(),
            SyntaxGrammar {
                items: vec![
                    SyntaxItem::Comment("Heading\n  detail".to_string()),
                    SyntaxItem::Rule(SyntaxRule {
                        name: "Start".to_string(),
                        expression: name("A"),
                    }),
                ]
            }
        );
    }

    #[test]
    fn parses_checked_in_rex_grammar_syntax() {
        let grammar = parse(crate::rex::REX_PEG_GRAMMAR).unwrap();
        let rules = grammar.rules().collect::<Vec<_>>();

        assert_eq!(rules.len(), 90);
        assert_eq!(rules.first().unwrap().name, "CompilationUnit");
        assert_eq!(rules.last().unwrap().name, "ValueName");
    }

    #[test]
    fn parses_checked_in_peg_grammar_syntax() {
        let grammar = parse(PEG_PEG_GRAMMAR).unwrap();
        let resolved = parser_grammar_from_syntax(&grammar).unwrap();

        assert_eq!(grammar, peg_syntax_grammar());
        assert_eq!(resolved, parser_grammar());
        assert_eq!(
            PEG_PEG_GRAMMAR,
            crate::grammar::grammar_to_string(&parser_grammar()),
            "checked-in peg.peg must be regenerated from parser_grammar()"
        );
        assert_eq!(
            crate::grammar::grammar_to_string(&resolved),
            PEG_PEG_GRAMMAR,
            "loaded parser grammar must render back to canonical peg.peg text"
        );
        let rules = grammar.rules().collect::<Vec<_>>();
        assert_eq!(rules.len(), 13);
        assert_eq!(rules.first().unwrap().name, "Grammar");
        assert_eq!(rules.last().unwrap().name, "Group");
    }

    #[test]
    fn peg_grammar_resolution_rejects_unknown_symbols() {
        let source = PEG_PEG_GRAMMAR.replacen("Grammar   <- NEWLINE*", "Grammar   <- Missing*", 1);
        let grammar = parse(&source).unwrap();
        let err = parser_grammar_from_syntax(&grammar).unwrap_err();

        assert_eq!(err.message, "unknown grammar symbol `Missing`");
    }

    #[test]
    fn peg_grammar_resolution_rejects_duplicate_rules() {
        let source = format!("{PEG_PEG_GRAMMAR}Grammar <- EOF\n");
        let grammar = parse(&source).unwrap();
        let err = parser_grammar_from_syntax(&grammar).unwrap_err();

        assert_eq!(err.message, "duplicate rule definition `Grammar`");
    }

    #[test]
    fn peg_grammar_resolution_rejects_missing_rules() {
        let source = PEG_PEG_GRAMMAR.replace("Group     <- PAREN_L Choice PAREN_R\n", "");
        let grammar = parse(&source).unwrap();
        let err = parser_grammar_from_syntax(&grammar).unwrap_err();

        assert_eq!(err.message, "missing rule definition `Group`");
    }

    #[test]
    fn rejects_missing_arrow() {
        let err = parse("Start IDENT\n").unwrap_err();

        assert_eq!(err.message, "expected `<-`");
    }

    #[test]
    fn rejects_empty_expressions() {
        let err = parse("Start <- \n").unwrap_err();

        assert_eq!(err.message, "expected expression");
    }

    #[test]
    fn rejects_missing_label_comma() {
        let err = parse("Start <- label(\"message\" IDENT)\n").unwrap_err();

        assert_eq!(err.message, "expected `,`");
    }

    #[test]
    fn rejects_reserved_function_name_without_call() {
        let err = parse("Start <- cut\n").unwrap_err();

        assert_eq!(err.message, "expected `(`");
    }

    fn name(name: &str) -> SyntaxExpr {
        super::name(name)
    }
}
