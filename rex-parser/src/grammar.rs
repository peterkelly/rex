use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

use rex_ast::Span;

use crate::{
    lexer::Token,
    peg::{Engine, EngineToken, Failure, FailureTracker, Mark, MemoEntry, TokenIndex, span_at},
};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum TokenKind {
    As,
    Case,
    Class,
    Declare,
    Else,
    Fn,
    If,
    Import,
    Is,
    Instance,
    Match,
    Pub,
    Type,
    Then,
    With,
    Where,
    Div,
    Dot,
    Gt,
    Lt,
    Le,
    Mul,
    Sub,
    ArrowR,
    Assign,
    BackSlash,
    BraceL,
    BraceR,
    BracketL,
    BracketR,
    Colon,
    ColonColon,
    Comma,
    DotDot,
    In,
    Let,
    Rec,
    ParenL,
    ParenR,
    Pipe,
    Question,
    SemiColon,
    Bool,
    Float,
    Int,
    String,
    Ident,
    ValueOperator,
    BinaryOperator,
    Eof,
}

impl TokenKind {
    #[cfg(test)]
    pub(crate) const ALL: &'static [Self] = &[
        Self::As,
        Self::Case,
        Self::Class,
        Self::Declare,
        Self::Else,
        Self::Fn,
        Self::If,
        Self::Import,
        Self::Is,
        Self::Instance,
        Self::Match,
        Self::Pub,
        Self::Type,
        Self::Then,
        Self::With,
        Self::Where,
        Self::Div,
        Self::Dot,
        Self::Gt,
        Self::Lt,
        Self::Le,
        Self::Mul,
        Self::Sub,
        Self::ArrowR,
        Self::Assign,
        Self::BackSlash,
        Self::BraceL,
        Self::BraceR,
        Self::BracketL,
        Self::BracketR,
        Self::Colon,
        Self::ColonColon,
        Self::Comma,
        Self::DotDot,
        Self::In,
        Self::Let,
        Self::Rec,
        Self::ParenL,
        Self::ParenR,
        Self::Pipe,
        Self::Question,
        Self::SemiColon,
        Self::Bool,
        Self::Float,
        Self::Int,
        Self::String,
        Self::Ident,
        Self::ValueOperator,
        Self::BinaryOperator,
        Self::Eof,
    ];

    #[cfg(test)]
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.peg_name() == name)
    }

    pub(crate) fn matches(self, token: &Token) -> bool {
        match self {
            TokenKind::As => matches!(token, Token::As(..)),
            TokenKind::Case => matches!(token, Token::Case(..)),
            TokenKind::Class => matches!(token, Token::Class(..)),
            TokenKind::Declare => matches!(token, Token::Declare(..)),
            TokenKind::Else => matches!(token, Token::Else(..)),
            TokenKind::Fn => matches!(token, Token::Fn(..)),
            TokenKind::If => matches!(token, Token::If(..)),
            TokenKind::Import => matches!(token, Token::Import(..)),
            TokenKind::Is => matches!(token, Token::Is(..)),
            TokenKind::Instance => matches!(token, Token::Instance(..)),
            TokenKind::Match => matches!(token, Token::Match(..)),
            TokenKind::Pub => matches!(token, Token::Pub(..)),
            TokenKind::Type => matches!(token, Token::Type(..)),
            TokenKind::Then => matches!(token, Token::Then(..)),
            TokenKind::With => matches!(token, Token::With(..)),
            TokenKind::Where => matches!(token, Token::Where(..)),
            TokenKind::Div => matches!(token, Token::Div(..)),
            TokenKind::Dot => matches!(token, Token::Dot(..)),
            TokenKind::Gt => matches!(token, Token::Gt(..)),
            TokenKind::Lt => matches!(token, Token::Lt(..)),
            TokenKind::Le => matches!(token, Token::Le(..)),
            TokenKind::Mul => matches!(token, Token::Mul(..)),
            TokenKind::Sub => matches!(token, Token::Sub(..)),
            TokenKind::ArrowR => matches!(token, Token::ArrowR(..)),
            TokenKind::Assign => matches!(token, Token::Assign(..)),
            TokenKind::BackSlash => matches!(token, Token::BackSlash(..)),
            TokenKind::BraceL => matches!(token, Token::BraceL(..)),
            TokenKind::BraceR => matches!(token, Token::BraceR(..)),
            TokenKind::BracketL => matches!(token, Token::BracketL(..)),
            TokenKind::BracketR => matches!(token, Token::BracketR(..)),
            TokenKind::Colon => matches!(token, Token::Colon(..)),
            TokenKind::ColonColon => matches!(token, Token::ColonColon(..)),
            TokenKind::Comma => matches!(token, Token::Comma(..)),
            TokenKind::DotDot => matches!(token, Token::DotDot(..)),
            TokenKind::In => matches!(token, Token::In(..)),
            TokenKind::Let => matches!(token, Token::Let(..)),
            TokenKind::Rec => matches!(token, Token::Rec(..)),
            TokenKind::ParenL => matches!(token, Token::ParenL(..)),
            TokenKind::ParenR => matches!(token, Token::ParenR(..)),
            TokenKind::Pipe => matches!(token, Token::Pipe(..)),
            TokenKind::Question => matches!(token, Token::Question(..)),
            TokenKind::SemiColon => matches!(token, Token::SemiColon(..)),
            TokenKind::Bool => matches!(token, Token::Bool(..)),
            TokenKind::Float => matches!(token, Token::Float(..)),
            TokenKind::Int => matches!(token, Token::Int(..)),
            TokenKind::String => matches!(token, Token::String(..)),
            TokenKind::Ident => matches!(token, Token::Ident(..)),
            TokenKind::ValueOperator => operator_token_name(token).is_some(),
            TokenKind::BinaryOperator => binary_operator_token_name(token).is_some(),
            TokenKind::Eof => matches!(token, Token::Eof(..)),
        }
    }

    fn label(self) -> &'static str {
        match self {
            TokenKind::As => "`as`",
            TokenKind::Case => "`case`",
            TokenKind::Class => "`class`",
            TokenKind::Declare => "`declare`",
            TokenKind::Else => "`else`",
            TokenKind::Fn => "`fn`",
            TokenKind::If => "`if`",
            TokenKind::Import => "`import`",
            TokenKind::Is => "`is`",
            TokenKind::Instance => "`instance`",
            TokenKind::Match => "`match`",
            TokenKind::Pub => "`pub`",
            TokenKind::Type => "`type`",
            TokenKind::Then => "`then`",
            TokenKind::With => "`with`",
            TokenKind::Where => "`where`",
            TokenKind::Div => "`/`",
            TokenKind::Dot => "`.`",
            TokenKind::Gt => "`>`",
            TokenKind::Lt => "`<`",
            TokenKind::Le => "`<=`",
            TokenKind::Mul => "`*`",
            TokenKind::Sub => "`-`",
            TokenKind::ArrowR => "`->`",
            TokenKind::Assign => "`=`",
            TokenKind::BackSlash => "`\\`",
            TokenKind::BraceL => "`{`",
            TokenKind::BraceR => "`}`",
            TokenKind::BracketL => "`[`",
            TokenKind::BracketR => "`]`",
            TokenKind::Colon => "`:`",
            TokenKind::ColonColon => "`::`",
            TokenKind::Comma => "`,`",
            TokenKind::DotDot => "`..`",
            TokenKind::In => "`in`",
            TokenKind::Let => "`let`",
            TokenKind::Rec => "`rec`",
            TokenKind::ParenL => "`(`",
            TokenKind::ParenR => "`)`",
            TokenKind::Pipe => "`|`",
            TokenKind::Question => "`?`",
            TokenKind::SemiColon => "`;`",
            TokenKind::Bool => "bool",
            TokenKind::Float => "float",
            TokenKind::Int => "int",
            TokenKind::String => "string",
            TokenKind::Ident => "identifier",
            TokenKind::ValueOperator => "operator name",
            TokenKind::BinaryOperator => "binary operator",
            TokenKind::Eof => "EOF",
        }
    }

    fn peg_name(self) -> &'static str {
        match self {
            TokenKind::As => "AS",
            TokenKind::Case => "CASE",
            TokenKind::Class => "CLASS",
            TokenKind::Declare => "DECLARE",
            TokenKind::Else => "ELSE",
            TokenKind::Fn => "FN",
            TokenKind::If => "IF",
            TokenKind::Import => "IMPORT",
            TokenKind::Is => "IS",
            TokenKind::Instance => "INSTANCE",
            TokenKind::Match => "MATCH",
            TokenKind::Pub => "PUB",
            TokenKind::Type => "TYPE",
            TokenKind::Then => "THEN",
            TokenKind::With => "WITH",
            TokenKind::Where => "WHERE",
            TokenKind::Div => "DIV",
            TokenKind::Dot => "DOT",
            TokenKind::Gt => "GT",
            TokenKind::Lt => "LT",
            TokenKind::Le => "LE",
            TokenKind::Mul => "MUL",
            TokenKind::Sub => "SUB",
            TokenKind::ArrowR => "ARROW_R",
            TokenKind::Assign => "ASSIGN",
            TokenKind::BackSlash => "BACK_SLASH",
            TokenKind::BraceL => "BRACE_L",
            TokenKind::BraceR => "BRACE_R",
            TokenKind::BracketL => "BRACKET_L",
            TokenKind::BracketR => "BRACKET_R",
            TokenKind::Colon => "COLON",
            TokenKind::ColonColon => "COLON_COLON",
            TokenKind::Comma => "COMMA",
            TokenKind::DotDot => "DOT_DOT",
            TokenKind::In => "IN",
            TokenKind::Let => "LET",
            TokenKind::Rec => "REC",
            TokenKind::ParenL => "PAREN_L",
            TokenKind::ParenR => "PAREN_R",
            TokenKind::Pipe => "PIPE",
            TokenKind::Question => "QUESTION",
            TokenKind::SemiColon => "SEMI_COLON",
            TokenKind::Bool => "BOOL",
            TokenKind::Float => "FLOAT",
            TokenKind::Int => "INT",
            TokenKind::String => "STRING",
            TokenKind::Ident => "IDENT",
            TokenKind::ValueOperator => "VALUE_OPERATOR",
            TokenKind::BinaryOperator => "BINARY_OPERATOR",
            TokenKind::Eof => "EOF",
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.peg_name())
    }
}

// A grammar terminal is separate from a concrete token. This lets the same
// iterative PEG interpreter run over Rex lexer tokens and over the test-only
// `.peg` lexer tokens without teaching either lexer about the other.
pub(crate) trait Terminal<T>: Copy {
    fn label(self) -> &'static str;
    fn matches(self, token: &T) -> bool;
}

impl Terminal<Token> for TokenKind {
    fn label(self) -> &'static str {
        self.label()
    }

    fn matches(self, token: &Token) -> bool {
        self.matches(token)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Peg<R, K = TokenKind> {
    Token(K),
    Rule(R),
    Seq(Vec<Peg<R, K>>),
    Choice(Vec<Peg<R, K>>),
    Optional(Box<Peg<R, K>>),
    Repeat(Box<Peg<R, K>>),
    Repeat1(Box<Peg<R, K>>),
    And(Box<Peg<R, K>>),
    Not(Box<Peg<R, K>>),
    Label(String, Box<Peg<R, K>>),
    Cut(Box<Peg<R, K>>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Grammar<R, K = TokenKind> {
    start: R,
    items: Vec<Item<R, K>>,
    rule_indexes: BTreeMap<R, usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Item<R, K = TokenKind> {
    Rule(R, Peg<R, K>),
    Comment(String),
}

impl<R, K> Grammar<R, K>
where
    R: Copy + Ord,
{
    #[cfg(test)]
    pub(crate) fn new(start: R, rules: impl IntoIterator<Item = (R, Peg<R, K>)>) -> Self {
        Self::from_items(
            start,
            rules
                .into_iter()
                .map(|(rule, expression)| Item::Rule(rule, expression)),
        )
    }

    pub(crate) fn from_items(start: R, items: impl IntoIterator<Item = Item<R, K>>) -> Self {
        let items = items.into_iter().collect::<Vec<_>>();
        let mut rule_indexes = BTreeMap::new();

        for (index, item) in items.iter().enumerate() {
            if let Item::Rule(rule, _) = item {
                rule_indexes.insert(*rule, index);
            }
        }

        Self {
            start,
            items,
            rule_indexes,
        }
    }

    pub(crate) fn start(&self) -> R {
        self.start
    }

    pub(crate) fn expression(&self, rule: R) -> Option<&Peg<R, K>> {
        let index = *self.rule_indexes.get(&rule)?;
        match self.items.get(index)? {
            Item::Rule(_, expression) => Some(expression),
            Item::Comment(_) => unreachable!("rule index points at a rule item"),
        }
    }

    #[cfg(test)]
    pub(crate) fn items(&self) -> impl DoubleEndedIterator<Item = &Item<R, K>> {
        self.items.iter()
    }

    #[cfg(test)]
    pub(crate) fn rules(&self) -> impl DoubleEndedIterator<Item = (R, &Peg<R, K>)> + '_ {
        self.items.iter().filter_map(|item| match item {
            Item::Rule(rule, expression) => Some((*rule, expression)),
            Item::Comment(_) => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cst<R, T = Token> {
    Node(Arc<CstNode<R, T>>),
    Token(T),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CstNode<R, T = Token> {
    pub(crate) rule: R,
    pub(crate) span: Span,
    pub(crate) children: Vec<Cst<R, T>>,
}

impl<R, T> Drop for CstNode<R, T> {
    fn drop(&mut self) {
        let mut stack = std::mem::take(&mut self.children);
        while let Some(child) = stack.pop() {
            if let Cst::Node(mut node) = child
                && let Some(node) = Arc::get_mut(&mut node)
            {
                stack.extend(std::mem::take(&mut node.children));
            }
        }
    }
}

enum Work<'peg, R, K, T> {
    Eval(&'peg Peg<R, K>),
    Ready(Result<Vec<Cst<R, T>>, Failure>),
    PushAndEval(Frame<'peg, R, K, T>, &'peg Peg<R, K>),
}

enum Frame<'peg, R, K, T> {
    Rule {
        rule: R,
        start: TokenIndex,
        mark: Mark,
    },
    Seq {
        items: &'peg [Peg<R, K>],
        next: usize,
        children: Vec<Cst<R, T>>,
        mark: Mark,
    },
    Choice {
        alternatives: &'peg [Peg<R, K>],
        next: usize,
        mark: Mark,
        failures: FailureTracker,
        failure: Option<Failure>,
    },
    Optional {
        mark: Mark,
        failures: FailureTracker,
    },
    Repeat {
        item: &'peg Peg<R, K>,
        children: Vec<Cst<R, T>>,
        start: TokenIndex,
        mark: Mark,
        failures: FailureTracker,
    },
    Repeat1 {
        item: &'peg Peg<R, K>,
    },
    And {
        mark: Mark,
        failures: FailureTracker,
    },
    Not {
        mark: Mark,
        failures: FailureTracker,
    },
    Label {
        label: &'peg str,
    },
    Cut,
}

type CstMemo<R, T> = BTreeMap<(R, TokenIndex), MemoEntry<Arc<CstNode<R, T>>>>;

pub(crate) struct GrammarParser<'grammar, 'engine, 'input, R, K = TokenKind, T = Token>
where
    T: EngineToken,
{
    grammar: &'grammar Grammar<R, K>,
    engine: &'engine mut Engine<'input, T>,
    memo: CstMemo<R, T>,
}

impl<'grammar, 'engine, 'input, R, K, T> GrammarParser<'grammar, 'engine, 'input, R, K, T>
where
    R: Copy + Ord,
    K: Copy + Terminal<T>,
    T: EngineToken,
{
    pub(crate) fn new(
        grammar: &'grammar Grammar<R, K>,
        engine: &'engine mut Engine<'input, T>,
    ) -> Self {
        Self {
            grammar,
            engine,
            memo: BTreeMap::new(),
        }
    }

    pub(crate) fn parse_start(&mut self) -> Result<Arc<CstNode<R, T>>, Failure> {
        self.parse_rule(self.grammar.start())
    }

    pub(crate) fn parse_rule(&mut self, rule: R) -> Result<Arc<CstNode<R, T>>, Failure> {
        let root: Peg<R, K> = Peg::Rule(rule);
        let mut children = self.parse_expr_iterative(&root)?;
        match children.pop() {
            Some(Cst::Node(node)) if children.is_empty() => Ok(node),
            _ => Err(self.engine.fail("grammar rule")),
        }
    }

    fn parse_expr_iterative(&mut self, root: &Peg<R, K>) -> Result<Vec<Cst<R, T>>, Failure> {
        let mut work = Work::Eval(root);
        let mut frames = Vec::new();

        loop {
            match work {
                Work::PushAndEval(frame, expr) => {
                    frames.push(frame);
                    work = Work::Eval(expr);
                }
                Work::Eval(expr) => match expr {
                    Peg::Token(kind) => {
                        let expected = kind.label();
                        let result = self
                            .engine
                            .expect(expected, |token| kind.matches(token))
                            .map(|token| vec![Cst::Token(token)]);
                        work = Work::Ready(result);
                    }
                    Peg::Rule(rule) => {
                        let start = self.engine.pos();
                        if let Some(entry) = self.memo.get(&(*rule, start)).cloned() {
                            match entry {
                                MemoEntry::Success { value, next } => {
                                    self.engine.restore_pos(next);
                                    work = Work::Ready(Ok(vec![Cst::Node(value)]));
                                }
                                MemoEntry::Failure(failure) => {
                                    self.engine.restore_pos(start);
                                    self.engine.record_failure(failure.clone());
                                    work = Work::Ready(Err(failure));
                                }
                            }
                            continue;
                        }

                        let mark = self.engine.mark();
                        let Some(rule_expr) = self.grammar.expression(*rule) else {
                            let failure = self.engine.fail("grammar rule");
                            self.memo
                                .insert((*rule, start), MemoEntry::Failure(failure.clone()));
                            work = Work::Ready(Err(failure));
                            continue;
                        };

                        frames.push(Frame::Rule {
                            rule: *rule,
                            start,
                            mark,
                        });
                        work = Work::Eval(rule_expr);
                    }
                    Peg::Seq(items) => {
                        if items.is_empty() {
                            work = Work::Ready(Ok(Vec::new()));
                        } else {
                            frames.push(Frame::Seq {
                                items,
                                next: 1,
                                children: Vec::new(),
                                mark: self.engine.mark(),
                            });
                            work = Work::Eval(&items[0]);
                        }
                    }
                    Peg::Choice(alternatives) => {
                        if alternatives.is_empty() {
                            work = Work::Ready(Err(self.engine.fail("alternative")));
                        } else {
                            let mark = self.engine.mark();
                            let failures = self.engine.failure_checkpoint();
                            frames.push(Frame::Choice {
                                alternatives,
                                next: 1,
                                mark,
                                failures: failures.clone(),
                                failure: None,
                            });
                            self.engine.restore(mark);
                            self.engine.restore_failures(failures);
                            work = Work::Eval(&alternatives[0]);
                        }
                    }
                    Peg::Optional(item) => {
                        frames.push(Frame::Optional {
                            mark: self.engine.mark(),
                            failures: self.engine.failure_checkpoint(),
                        });
                        work = Work::Eval(item);
                    }
                    Peg::Repeat(item) => {
                        let mark = self.engine.mark();
                        let failures = self.engine.failure_checkpoint();
                        let start = self.engine.pos();
                        frames.push(Frame::Repeat {
                            item,
                            children: Vec::new(),
                            start,
                            mark,
                            failures,
                        });
                        work = Work::Eval(item);
                    }
                    Peg::Repeat1(item) => {
                        frames.push(Frame::Repeat1 { item });
                        work = Work::Eval(item);
                    }
                    Peg::And(item) => {
                        frames.push(Frame::And {
                            mark: self.engine.mark(),
                            failures: self.engine.failure_checkpoint(),
                        });
                        work = Work::Eval(item);
                    }
                    Peg::Not(item) => {
                        frames.push(Frame::Not {
                            mark: self.engine.mark(),
                            failures: self.engine.failure_checkpoint(),
                        });
                        work = Work::Eval(item);
                    }
                    Peg::Label(label, item) => {
                        frames.push(Frame::Label { label });
                        work = Work::Eval(item);
                    }
                    Peg::Cut(item) => {
                        frames.push(Frame::Cut);
                        work = Work::Eval(item);
                    }
                },
                Work::Ready(result) => {
                    let Some(frame) = frames.pop() else {
                        return result;
                    };
                    work = self.apply_frame(frame, result);
                }
            }
        }
    }

    fn apply_frame<'peg>(
        &mut self,
        frame: Frame<'peg, R, K, T>,
        result: Result<Vec<Cst<R, T>>, Failure>,
    ) -> Work<'peg, R, K, T> {
        match frame {
            Frame::Rule { rule, start, mark } => match result {
                Ok(children) => {
                    let next = self.engine.pos();
                    let node = Arc::new(CstNode {
                        rule,
                        span: self.span_from_positions(start, next),
                        children,
                    });
                    self.memo.insert(
                        (rule, start),
                        MemoEntry::Success {
                            value: node.clone(),
                            next,
                        },
                    );
                    Work::Ready(Ok(vec![Cst::Node(node)]))
                }
                Err(failure) => {
                    self.engine.restore(mark);
                    self.memo
                        .insert((rule, start), MemoEntry::Failure(failure.clone()));
                    Work::Ready(Err(failure))
                }
            },
            Frame::Seq {
                items,
                next,
                mut children,
                mark,
            } => match result {
                Ok(mut new_children) => {
                    children.append(&mut new_children);
                    if let Some(item) = items.get(next) {
                        Work::PushAndEval(
                            Frame::Seq {
                                items,
                                next: next + 1,
                                children,
                                mark,
                            },
                            item,
                        )
                    } else {
                        Work::Ready(Ok(children))
                    }
                }
                Err(failure) => {
                    self.engine.restore(mark);
                    Work::Ready(Err(failure))
                }
            },
            Frame::Choice {
                alternatives,
                next,
                mark,
                failures,
                mut failure,
            } => match result {
                Ok(children) => Work::Ready(Ok(children)),
                Err(err) if err.committed => Work::Ready(Err(err)),
                Err(err) => {
                    merge_failure(&mut failure, err);
                    if let Some(alternative) = alternatives.get(next) {
                        self.engine.restore(mark);
                        self.engine.restore_failures(failures.clone());
                        Work::PushAndEval(
                            Frame::Choice {
                                alternatives,
                                next: next + 1,
                                mark,
                                failures,
                                failure,
                            },
                            alternative,
                        )
                    } else {
                        self.engine.restore(mark);
                        self.engine.restore_failures(failures);
                        let failure = failure.unwrap_or_else(|| self.engine.fail("alternative"));
                        self.engine.record_failure(failure.clone());
                        Work::Ready(Err(failure))
                    }
                }
            },
            Frame::Optional { mark, failures } => match result {
                Ok(children) => Work::Ready(Ok(children)),
                Err(err) if err.committed => Work::Ready(Err(err)),
                Err(_) => {
                    self.engine.restore(mark);
                    self.engine.restore_failures(failures);
                    Work::Ready(Ok(Vec::new()))
                }
            },
            Frame::Repeat {
                item,
                mut children,
                start,
                mark,
                failures,
            } => match result {
                Ok(mut new_children) => {
                    let advanced = self.engine.pos() != start;
                    children.append(&mut new_children);
                    if advanced {
                        let start = self.engine.pos();
                        let mark = self.engine.mark();
                        let failures = self.engine.failure_checkpoint();
                        Work::PushAndEval(
                            Frame::Repeat {
                                item,
                                children,
                                start,
                                mark,
                                failures,
                            },
                            item,
                        )
                    } else {
                        Work::Ready(Ok(children))
                    }
                }
                Err(err) if err.committed => Work::Ready(Err(err)),
                Err(_) => {
                    self.engine.restore(mark);
                    self.engine.restore_failures(failures);
                    Work::Ready(Ok(children))
                }
            },
            Frame::Repeat1 { item } => match result {
                Ok(children) => {
                    let start = self.engine.pos();
                    let mark = self.engine.mark();
                    let failures = self.engine.failure_checkpoint();
                    Work::PushAndEval(
                        Frame::Repeat {
                            item,
                            children,
                            start,
                            mark,
                            failures,
                        },
                        item,
                    )
                }
                Err(failure) => Work::Ready(Err(failure)),
            },
            Frame::And { mark, failures } => {
                self.engine.restore(mark);
                self.engine.restore_failures(failures);
                match result {
                    Ok(_) => Work::Ready(Ok(Vec::new())),
                    Err(_) => Work::Ready(Err(self.engine.fail("lookahead"))),
                }
            }
            Frame::Not { mark, failures } => {
                self.engine.restore(mark);
                self.engine.restore_failures(failures);
                match result {
                    Ok(_) => Work::Ready(Err(self.engine.fail("negative lookahead"))),
                    Err(_) => Work::Ready(Ok(Vec::new())),
                }
            }
            Frame::Label { label } => {
                Work::Ready(result.map_err(|failure| self.labeled_failure(failure, label)))
            }
            Frame::Cut => Work::Ready(result.map_err(mark_committed)),
        }
    }

    fn span_from_positions(&self, start: TokenIndex, end: TokenIndex) -> Span {
        if start.0 < end.0 {
            let begin = span_at(self.engine.tokens(), self.engine.eof_span(), start.0).begin;
            let end = span_at(self.engine.tokens(), self.engine.eof_span(), end.0 - 1).end;
            Span::from_begin_end(begin, end)
        } else {
            span_at(self.engine.tokens(), self.engine.eof_span(), start.0)
        }
    }

    fn labeled_failure(&mut self, failure: Failure, label: &str) -> Failure {
        let mut expected = BTreeSet::new();
        expected.insert(label.to_string());
        let labeled = Failure {
            pos: failure.pos,
            span: failure.span,
            expected,
            committed: failure.committed,
        };
        self.engine.record_failure(labeled.clone());
        labeled
    }
}

pub(crate) fn tok<R, K>(kind: K) -> Peg<R, K> {
    Peg::Token(kind)
}

pub(crate) fn rule<R, K>(rule: R) -> Peg<R, K> {
    Peg::Rule(rule)
}

pub(crate) fn seq<R, K>(items: impl IntoIterator<Item = Peg<R, K>>) -> Peg<R, K> {
    Peg::Seq(items.into_iter().collect())
}

pub(crate) fn choice<R, K>(items: impl IntoIterator<Item = Peg<R, K>>) -> Peg<R, K> {
    Peg::Choice(items.into_iter().collect())
}

pub(crate) fn opt<R, K>(item: Peg<R, K>) -> Peg<R, K> {
    Peg::Optional(Box::new(item))
}

pub(crate) fn rep<R, K>(item: Peg<R, K>) -> Peg<R, K> {
    Peg::Repeat(Box::new(item))
}

pub(crate) fn rep1<R, K>(item: Peg<R, K>) -> Peg<R, K> {
    Peg::Repeat1(Box::new(item))
}

pub(crate) fn and<R, K>(item: Peg<R, K>) -> Peg<R, K> {
    Peg::And(Box::new(item))
}

pub(crate) fn not<R, K>(item: Peg<R, K>) -> Peg<R, K> {
    Peg::Not(Box::new(item))
}

pub(crate) fn label<R, K>(message: impl Into<String>, item: Peg<R, K>) -> Peg<R, K> {
    Peg::Label(message.into(), Box::new(item))
}

pub(crate) fn cut<R, K>(item: Peg<R, K>) -> Peg<R, K> {
    Peg::Cut(Box::new(item))
}

// This renderer is intentionally a verifier, not part of runtime parsing. The
// Rex grammar source of truth is the Rust `Grammar<R>` value; tests render it
// to canonical text and compare that text with checked `.peg` files so those
// human-readable specs cannot drift silently.
#[cfg(test)]
pub(crate) fn grammar_to_string<R, K>(grammar: &Grammar<R, K>) -> String
where
    R: Copy + Ord + fmt::Display,
    K: fmt::Display,
{
    let mut output = String::new();
    let rule_column = grammar
        .rules()
        .map(|(rule, _)| rule.to_string().len())
        .max()
        .unwrap_or(0)
        + 1;

    for item in grammar.items() {
        match item {
            Item::Rule(rule, expression) => {
                let name = rule.to_string();
                output.push_str(&name);
                output.push_str(&" ".repeat(rule_column - name.len()));
                output.push_str("<- ");
                output.push_str(&peg_to_string(expression));
                output.push('\n');
            }
            Item::Comment(comment) => render_comment(comment, &mut output),
        }
    }

    output
}

#[cfg(test)]
fn render_comment(comment: &str, output: &mut String) {
    output.push('\n');

    for line in comment.split('\n') {
        output.push_str("# ");
        output.push_str(line);
        output.push('\n');
    }

    output.push('\n');
}

#[cfg(test)]
fn peg_to_string<R, K>(expression: &Peg<R, K>) -> String
where
    R: fmt::Display,
    K: fmt::Display,
{
    render_peg(expression, RenderPrecedence::Choice)
}

#[cfg(test)]
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum RenderPrecedence {
    Choice,
    Sequence,
    Prefix,
    Postfix,
    Atom,
}

#[cfg(test)]
fn render_peg<R, K>(expression: &Peg<R, K>, context: RenderPrecedence) -> String
where
    R: fmt::Display,
    K: fmt::Display,
{
    let precedence = render_precedence(expression);
    let rendered = match expression {
        Peg::Token(kind) => kind.to_string(),
        Peg::Rule(rule) => rule.to_string(),
        Peg::Seq(items) => {
            assert!(
                !items.is_empty(),
                "empty PEG sequences are not supported by the canonical renderer"
            );
            items
                .iter()
                .map(|item| render_peg(item, RenderPrecedence::Sequence))
                .collect::<Vec<_>>()
                .join(" ")
        }
        Peg::Choice(items) => {
            assert!(
                !items.is_empty(),
                "empty PEG choices are not supported by the canonical renderer"
            );
            items
                .iter()
                .map(|item| render_peg(item, RenderPrecedence::Choice))
                .collect::<Vec<_>>()
                .join(" / ")
        }
        Peg::Optional(item) => format!("{}?", render_peg(item, RenderPrecedence::Atom)),
        Peg::Repeat(item) => format!("{}*", render_peg(item, RenderPrecedence::Atom)),
        Peg::Repeat1(item) => format!("{}+", render_peg(item, RenderPrecedence::Atom)),
        Peg::And(item) => format!("&{}", render_peg(item, RenderPrecedence::Prefix)),
        Peg::Not(item) => format!("!{}", render_peg(item, RenderPrecedence::Prefix)),
        Peg::Label(message, item) => format!(
            "label({}, {})",
            rust_string_literal(message),
            render_peg(item, RenderPrecedence::Choice)
        ),
        Peg::Cut(item) => format!("cut({})", render_peg(item, RenderPrecedence::Choice)),
    };

    if precedence < context {
        format!("({rendered})")
    } else {
        rendered
    }
}

#[cfg(test)]
fn render_precedence<R, K>(expression: &Peg<R, K>) -> RenderPrecedence {
    match expression {
        Peg::Choice(_) => RenderPrecedence::Choice,
        Peg::Seq(_) => RenderPrecedence::Sequence,
        Peg::And(_) | Peg::Not(_) => RenderPrecedence::Prefix,
        Peg::Optional(_) | Peg::Repeat(_) | Peg::Repeat1(_) => RenderPrecedence::Postfix,
        Peg::Token(_) | Peg::Rule(_) | Peg::Label(_, _) | Peg::Cut(_) => RenderPrecedence::Atom,
    }
}

#[cfg(test)]
fn rust_string_literal(message: &str) -> String {
    format!("{message:?}")
}

fn merge_failure(target: &mut Option<Failure>, failure: Failure) {
    match target {
        Some(existing) => {
            if failure.pos > existing.pos {
                *existing = failure;
            } else if failure.pos == existing.pos {
                existing.expected.extend(failure.expected);
                existing.committed |= failure.committed;
            }
        }
        None => *target = Some(failure),
    }
}

fn mark_committed(mut failure: Failure) -> Failure {
    failure.committed = true;
    failure
}

fn operator_token_name(token: &Token) -> Option<&'static str> {
    match token {
        Token::Add(..) => Some("+"),
        Token::And(..) => Some("&&"),
        Token::Concat(..) => Some("++"),
        Token::Div(..) => Some("/"),
        Token::Eq(..) => Some("=="),
        Token::Ne(..) => Some("!="),
        Token::Ge(..) => Some(">="),
        Token::Gt(..) => Some(">"),
        Token::Le(..) => Some("<="),
        Token::Lt(..) => Some("<"),
        Token::Mod(..) => Some("%"),
        Token::Mul(..) => Some("*"),
        Token::Or(..) => Some("||"),
        Token::Sub(..) => Some("-"),
        _ => None,
    }
}

fn binary_operator_token_name(token: &Token) -> Option<&'static str> {
    match token {
        Token::ColonColon(..) => Some("::"),
        _ => operator_token_name(token),
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
    enum TestRule {
        Start,
        Term,
    }

    impl fmt::Display for TestRule {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{self:?}")
        }
    }

    #[test]
    fn grammar_to_string_renders_rules_in_stable_order() {
        let grammar = Grammar::from_items(
            TestRule::Start,
            [
                Item::Rule(
                    TestRule::Start,
                    seq([
                        tok(TokenKind::Ident),
                        rep(rule(TestRule::Term)),
                        tok(TokenKind::Eof),
                    ]),
                ),
                Item::Rule(
                    TestRule::Term,
                    choice([tok(TokenKind::Int), tok(TokenKind::String)]),
                ),
            ],
        );

        assert_eq!(
            grammar_to_string(&grammar),
            "Start <- IDENT Term* EOF\nTerm  <- INT / STRING\n"
        );
    }

    #[test]
    fn grammar_to_string_parenthesizes_to_preserve_structure() {
        let grammar = Grammar::new(
            TestRule::Start,
            [(
                TestRule::Start,
                seq([
                    choice([tok(TokenKind::Ident), tok(TokenKind::Int)]),
                    opt(seq([tok(TokenKind::String), tok(TokenKind::Bool)])),
                    rep(and(tok(TokenKind::Float))),
                    and(opt(tok(TokenKind::Question))),
                ]),
            )],
        );

        assert_eq!(
            grammar_to_string(&grammar),
            "Start <- (IDENT / INT) (STRING BOOL)? (&FLOAT)* &QUESTION?\n"
        );
    }

    #[test]
    fn grammar_to_string_renders_cut_and_label() {
        let grammar = Grammar::new(
            TestRule::Start,
            [(
                TestRule::Start,
                cut(label(
                    "expected \"thing\"\n",
                    seq([tok(TokenKind::Ident), tok(TokenKind::SemiColon)]),
                )),
            )],
        );

        assert_eq!(
            grammar_to_string(&grammar),
            "Start <- cut(label(\"expected \\\"thing\\\"\\n\", IDENT SEMI_COLON))\n"
        );
    }

    #[test]
    fn grammar_to_string_renders_comment_items() {
        let grammar = Grammar::from_items(
            TestRule::Start,
            [
                Item::Comment("Group heading\n  detail".to_string()),
                Item::Rule(TestRule::Start, tok(TokenKind::Eof)),
            ],
        );

        assert_eq!(
            grammar_to_string(&grammar),
            "\n# Group heading\n#   detail\n\nStart <- EOF\n"
        );
    }
}
