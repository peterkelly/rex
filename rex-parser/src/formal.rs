use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use rex_lexer::{Token, span::Span};

use crate::peg::{Engine, Failure, FailureTracker, Mark, MemoEntry, Pos, span_at};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum TokenKind {
    As,
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
    When,
    Then,
    With,
    Where,
    Div,
    Dot,
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
    HashTag,
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
    HttpsUrl,
    Ident,
    ValueOperator,
    BinaryOperator,
    Eof,
}

impl TokenKind {
    pub(crate) fn matches(self, token: &Token) -> bool {
        match self {
            TokenKind::As => matches!(token, Token::As(..)),
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
            TokenKind::When => matches!(token, Token::When(..)),
            TokenKind::Then => matches!(token, Token::Then(..)),
            TokenKind::With => matches!(token, Token::With(..)),
            TokenKind::Where => matches!(token, Token::Where(..)),
            TokenKind::Div => matches!(token, Token::Div(..)),
            TokenKind::Dot => matches!(token, Token::Dot(..)),
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
            TokenKind::HashTag => matches!(token, Token::HashTag(..)),
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
            TokenKind::HttpsUrl => matches!(token, Token::HttpsUrl(..)),
            TokenKind::Ident => matches!(token, Token::Ident(..)),
            TokenKind::ValueOperator => operator_token_name(token).is_some(),
            TokenKind::BinaryOperator => binary_operator_token_name(token).is_some(),
            TokenKind::Eof => matches!(token, Token::Eof(..)),
        }
    }

    fn label(self) -> &'static str {
        match self {
            TokenKind::As => "`as`",
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
            TokenKind::When => "`when`",
            TokenKind::Then => "`then`",
            TokenKind::With => "`with`",
            TokenKind::Where => "`where`",
            TokenKind::Div => "`/`",
            TokenKind::Dot => "`.`",
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
            TokenKind::HashTag => "`#`",
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
            TokenKind::HttpsUrl => "URL",
            TokenKind::Ident => "identifier",
            TokenKind::ValueOperator => "operator name",
            TokenKind::BinaryOperator => "binary operator",
            TokenKind::Eof => "EOF",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Peg<R> {
    Token(TokenKind),
    Rule(R),
    Seq(Vec<Peg<R>>),
    Choice(Vec<Peg<R>>),
    Optional(Box<Peg<R>>),
    Repeat(Box<Peg<R>>),
    Repeat1(Box<Peg<R>>),
    And(Box<Peg<R>>),
    Not(Box<Peg<R>>),
    Label(&'static str, Box<Peg<R>>),
    Cut(Box<Peg<R>>),
}

#[derive(Clone, Debug)]
pub(crate) struct Grammar<R> {
    start: R,
    rules: BTreeMap<R, Peg<R>>,
}

impl<R> Grammar<R>
where
    R: Copy + Ord,
{
    pub(crate) fn new(start: R, rules: impl IntoIterator<Item = (R, Peg<R>)>) -> Self {
        Self {
            start,
            rules: rules.into_iter().collect(),
        }
    }

    pub(crate) fn start(&self) -> R {
        self.start
    }

    fn expr(&self, rule: R) -> Option<&Peg<R>> {
        self.rules.get(&rule)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cst<R> {
    Node(Arc<CstNode<R>>),
    Token(Token),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CstNode<R> {
    pub(crate) rule: R,
    pub(crate) span: Span,
    pub(crate) children: Vec<Cst<R>>,
}

impl<R> Drop for CstNode<R> {
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

enum Work<'peg, R> {
    Eval(&'peg Peg<R>),
    Ready(Result<Vec<Cst<R>>, Failure>),
    PushAndEval(Frame<'peg, R>, &'peg Peg<R>),
}

enum Frame<'peg, R> {
    Rule {
        rule: R,
        start: Pos,
        mark: Mark,
    },
    Seq {
        items: &'peg [Peg<R>],
        next: usize,
        children: Vec<Cst<R>>,
        mark: Mark,
    },
    Choice {
        alternatives: &'peg [Peg<R>],
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
        item: &'peg Peg<R>,
        children: Vec<Cst<R>>,
        start: Pos,
        mark: Mark,
        failures: FailureTracker,
    },
    Repeat1 {
        item: &'peg Peg<R>,
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
        label: &'static str,
    },
    Cut,
}

pub(crate) struct GrammarParser<'grammar, 'engine, 'input, R> {
    grammar: &'grammar Grammar<R>,
    engine: &'engine mut Engine<'input>,
    memo: BTreeMap<(R, Pos), MemoEntry<Arc<CstNode<R>>>>,
}

impl<'grammar, 'engine, 'input, R> GrammarParser<'grammar, 'engine, 'input, R>
where
    R: Copy + Ord,
{
    pub(crate) fn new(grammar: &'grammar Grammar<R>, engine: &'engine mut Engine<'input>) -> Self {
        Self {
            grammar,
            engine,
            memo: BTreeMap::new(),
        }
    }

    pub(crate) fn parse_start(&mut self) -> Result<Arc<CstNode<R>>, Failure> {
        self.parse_rule(self.grammar.start())
    }

    pub(crate) fn parse_rule(&mut self, rule: R) -> Result<Arc<CstNode<R>>, Failure> {
        let root = Peg::Rule(rule);
        let mut children = self.parse_expr_iterative(&root)?;
        match children.pop() {
            Some(Cst::Node(node)) if children.is_empty() => Ok(node),
            _ => Err(self.engine.fail("grammar rule")),
        }
    }

    fn parse_expr_iterative(&mut self, root: &Peg<R>) -> Result<Vec<Cst<R>>, Failure> {
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
                        let Some(rule_expr) = self.grammar.expr(*rule) else {
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
        frame: Frame<'peg, R>,
        result: Result<Vec<Cst<R>>, Failure>,
    ) -> Work<'peg, R> {
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

    fn span_from_positions(&self, start: Pos, end: Pos) -> Span {
        if start.0 < end.0 {
            let begin = span_at(self.engine.tokens(), self.engine.eof_span(), start.0).begin;
            let end = span_at(self.engine.tokens(), self.engine.eof_span(), end.0 - 1).end;
            Span::from_begin_end(begin, end)
        } else {
            span_at(self.engine.tokens(), self.engine.eof_span(), start.0)
        }
    }

    fn labeled_failure(&mut self, failure: Failure, label: &'static str) -> Failure {
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

pub(crate) fn tok<R>(kind: TokenKind) -> Peg<R> {
    Peg::Token(kind)
}

pub(crate) fn rule<R>(rule: R) -> Peg<R> {
    Peg::Rule(rule)
}

pub(crate) fn seq<R>(items: impl IntoIterator<Item = Peg<R>>) -> Peg<R> {
    Peg::Seq(items.into_iter().collect())
}

pub(crate) fn choice<R>(items: impl IntoIterator<Item = Peg<R>>) -> Peg<R> {
    Peg::Choice(items.into_iter().collect())
}

pub(crate) fn opt<R>(item: Peg<R>) -> Peg<R> {
    Peg::Optional(Box::new(item))
}

pub(crate) fn rep<R>(item: Peg<R>) -> Peg<R> {
    Peg::Repeat(Box::new(item))
}

pub(crate) fn rep1<R>(item: Peg<R>) -> Peg<R> {
    Peg::Repeat1(Box::new(item))
}

pub(crate) fn and<R>(item: Peg<R>) -> Peg<R> {
    Peg::And(Box::new(item))
}

pub(crate) fn not<R>(item: Peg<R>) -> Peg<R> {
    Peg::Not(Box::new(item))
}

pub(crate) fn label<R>(message: &'static str, item: Peg<R>) -> Peg<R> {
    Peg::Label(message, Box::new(item))
}

pub(crate) fn cut<R>(item: Peg<R>) -> Peg<R> {
    Peg::Cut(Box::new(item))
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
