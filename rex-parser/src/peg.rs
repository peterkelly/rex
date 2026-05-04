#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use rex_lexer::{
    Token, Tokens,
    span::{Span, Spanned},
};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct Pos(pub usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Mark(Pos);

#[derive(Clone, Debug)]
pub(crate) struct Input {
    tokens: Vec<Token>,
    eof: Span,
}

impl Input {
    pub(crate) fn new(tokens: Tokens) -> Self {
        Self {
            tokens: strip_comments(tokens.items),
            eof: tokens.eof,
        }
    }

    pub(crate) fn into_parts(self) -> (Vec<Token>, Span) {
        (self.tokens, self.eof)
    }

    pub(crate) fn engine(&self) -> Engine<'_, Token> {
        Engine::new(&self.tokens, Token::Eof(self.eof))
    }
}

fn strip_comments(mut tokens: Vec<Token>) -> Vec<Token> {
    let mut cursor = 0;

    while cursor < tokens.len() {
        match tokens[cursor] {
            Token::CommentL(..) => {
                tokens.remove(cursor);
                while cursor < tokens.len() {
                    if let Token::CommentR(..) = tokens[cursor] {
                        tokens.remove(cursor);
                        break;
                    }
                    tokens.remove(cursor);
                }
            }
            _ => {
                cursor += 1;
            }
        }
    }

    tokens
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Failure {
    pub(crate) pos: Pos,
    pub(crate) span: Span,
    pub(crate) expected: BTreeSet<String>,
    pub(crate) committed: bool,
}

impl Failure {
    fn new(pos: Pos, span: Span, expected: impl Into<String>) -> Self {
        let mut labels = BTreeSet::new();
        labels.insert(expected.into());
        Self {
            pos,
            span,
            expected: labels,
            committed: false,
        }
    }

    fn merge(&mut self, other: Failure) {
        if other.pos > self.pos {
            *self = other;
        } else if other.pos == self.pos {
            self.expected.extend(other.expected);
            self.committed |= other.committed;
        }
    }

    fn committed(mut self) -> Self {
        self.committed = true;
        self
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FailureTracker {
    farthest: Option<Failure>,
}

impl FailureTracker {
    pub(crate) fn record(&mut self, pos: usize, span: Span, expected: impl Into<String>) {
        let failure = Failure::new(Pos(pos), span, expected);
        self.record_failure(failure);
    }

    pub(crate) fn record_failure(&mut self, failure: Failure) {
        match &mut self.farthest {
            Some(existing) => existing.merge(failure),
            None => self.farthest = Some(failure),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn farthest(&self) -> Option<&Failure> {
        self.farthest.as_ref()
    }
}

pub(crate) type PegResult<T> = Result<T, Failure>;

#[allow(dead_code)]
pub(crate) type ParseResult<T> = Result<(T, Pos), Failure>;

// The cursor/failure engine is deliberately token-agnostic. Rex source and
// checked `.peg` grammar files have different lexers, but packrat bookkeeping
// only needs to clone tokens, recognize EOF, and map positions back to spans.
pub(crate) trait EngineToken: Clone {
    fn is_eof(&self) -> bool;
    fn span(&self) -> Span;
}

impl EngineToken for Token {
    fn is_eof(&self) -> bool {
        matches!(self, Token::Eof(..))
    }

    fn span(&self) -> Span {
        *Spanned::span(self)
    }
}

pub(crate) struct Engine<'input, T = Token>
where
    T: EngineToken,
{
    tokens: &'input [T],
    eof: T,
    cursor: Pos,
    failures: FailureTracker,
}

impl<'input, T> Engine<'input, T>
where
    T: EngineToken,
{
    pub(crate) fn new(tokens: &'input [T], eof: T) -> Self {
        Self {
            tokens,
            eof,
            cursor: Pos(0),
            failures: FailureTracker::default(),
        }
    }

    pub(crate) fn pos(&self) -> Pos {
        self.cursor
    }

    pub(crate) fn tokens(&self) -> &[T] {
        self.tokens
    }

    pub(crate) fn eof_span(&self) -> Span {
        self.eof.span()
    }

    pub(crate) fn mark(&self) -> Mark {
        Mark(self.cursor)
    }

    pub(crate) fn restore(&mut self, mark: Mark) {
        self.cursor = mark.0;
    }

    pub(crate) fn restore_pos(&mut self, pos: Pos) {
        self.cursor = pos;
    }

    pub(crate) fn current_span(&self) -> Span {
        span_at(self.tokens, self.eof_span(), self.cursor.0)
    }

    pub(crate) fn current_token(&self) -> T {
        self.tokens
            .get(self.cursor.0)
            .cloned()
            .unwrap_or_else(|| self.eof.clone())
    }

    pub(crate) fn bump(&mut self) -> T {
        let token = self.current_token();
        if !token.is_eof() {
            self.cursor.0 += 1;
        }
        token
    }

    pub(crate) fn farthest_failure(&self) -> Option<&Failure> {
        self.failures.farthest()
    }

    pub(crate) fn failure_checkpoint(&self) -> FailureTracker {
        self.failures.clone()
    }

    pub(crate) fn restore_failures(&mut self, failures: FailureTracker) {
        self.failures = failures;
    }

    pub(crate) fn fail(&mut self, expected: impl Into<String>) -> Failure {
        let failure = Failure::new(self.cursor, self.current_span(), expected);
        self.failures.record_failure(failure.clone());
        failure
    }

    pub(crate) fn fail_committed(&mut self, expected: impl Into<String>) -> Failure {
        let failure = Failure::new(self.cursor, self.current_span(), expected).committed();
        self.failures.record_failure(failure.clone());
        failure
    }

    pub(crate) fn record_failure(&mut self, failure: Failure) {
        self.failures.record_failure(failure);
    }

    pub(crate) fn satisfy<U>(
        &mut self,
        expected: impl Into<String>,
        predicate: impl FnOnce(&T) -> Option<U>,
    ) -> PegResult<U> {
        let token = self.current_token();
        if let Some(value) = predicate(&token) {
            if !token.is_eof() {
                self.cursor.0 += 1;
            }
            Ok(value)
        } else {
            Err(self.fail(expected))
        }
    }

    pub(crate) fn expect(
        &mut self,
        expected: impl Into<String>,
        predicate: impl FnOnce(&T) -> bool,
    ) -> PegResult<T> {
        self.satisfy(expected, |token| {
            if predicate(token) {
                Some(token.clone())
            } else {
                None
            }
        })
    }

    pub(crate) fn eof(&mut self) -> PegResult<()> {
        self.satisfy("EOF", |token| token.is_eof().then_some(()))
    }

    pub(crate) fn sequence<O>(
        &mut self,
        parser: impl FnOnce(&mut Self) -> PegResult<O>,
    ) -> PegResult<O> {
        let mark = self.mark();
        match parser(self) {
            Ok(value) => Ok(value),
            Err(failure) => {
                self.restore(mark);
                Err(failure)
            }
        }
    }

    pub(crate) fn choice<O>(
        &mut self,
        build: impl FnOnce(&mut Choice<'_, 'input, T, O>),
    ) -> PegResult<O> {
        let start = self.mark();
        let failures = self.failures.clone();
        let mut choice = Choice {
            engine: self,
            start,
            base_failures: failures,
            result: None,
            failure: None,
        };
        build(&mut choice);
        choice.finish()
    }

    pub(crate) fn optional<O>(
        &mut self,
        parser: impl FnOnce(&mut Self) -> PegResult<O>,
    ) -> PegResult<Option<O>> {
        let mark = self.mark();
        let failures = self.failures.clone();
        match parser(self) {
            Ok(value) => Ok(Some(value)),
            Err(_) => {
                self.restore(mark);
                self.failures = failures;
                Ok(None)
            }
        }
    }

    pub(crate) fn repeat<O>(
        &mut self,
        mut parser: impl FnMut(&mut Self) -> PegResult<O>,
    ) -> Vec<O> {
        let mut values = Vec::new();
        loop {
            let mark = self.mark();
            let failures = self.failures.clone();
            match parser(self) {
                Ok(value) => {
                    let advanced = self.pos() != mark.0;
                    values.push(value);
                    if !advanced {
                        break;
                    }
                }
                Err(_) => {
                    self.restore(mark);
                    self.failures = failures;
                    break;
                }
            }
        }
        values
    }

    pub(crate) fn repeat1<O>(
        &mut self,
        mut parser: impl FnMut(&mut Self) -> PegResult<O>,
    ) -> PegResult<Vec<O>> {
        let first = parser(self)?;
        let mut values = vec![first];
        values.extend(self.repeat(parser));
        Ok(values)
    }

    pub(crate) fn and_predicate<O>(
        &mut self,
        expected: impl Into<String>,
        parser: impl FnOnce(&mut Self) -> PegResult<O>,
    ) -> PegResult<()> {
        let mark = self.mark();
        let failures = self.failures.clone();
        let result = parser(self);
        self.restore(mark);
        self.failures = failures;
        match result {
            Ok(_) => Ok(()),
            Err(_) => Err(self.fail(expected)),
        }
    }

    pub(crate) fn not_predicate<O>(
        &mut self,
        expected: impl Into<String>,
        parser: impl FnOnce(&mut Self) -> PegResult<O>,
    ) -> PegResult<()> {
        let mark = self.mark();
        let failures = self.failures.clone();
        let result = parser(self);
        self.restore(mark);
        self.failures = failures;
        match result {
            Ok(_) => Err(self.fail(expected)),
            Err(_) => Ok(()),
        }
    }

    pub(crate) fn label<O>(
        &mut self,
        expected: impl Into<String>,
        parser: impl FnOnce(&mut Self) -> PegResult<O>,
    ) -> PegResult<O> {
        let expected = expected.into();
        match parser(self) {
            Ok(value) => Ok(value),
            Err(failure) => {
                let labeled = Failure::new(failure.pos, failure.span, expected);
                self.failures.record_failure(labeled.clone());
                Err(labeled)
            }
        }
    }
}

pub(crate) struct Choice<'engine, 'input, Tok, T>
where
    Tok: EngineToken,
{
    engine: &'engine mut Engine<'input, Tok>,
    start: Mark,
    base_failures: FailureTracker,
    result: Option<T>,
    failure: Option<Failure>,
}

impl<'input, Tok, T> Choice<'_, 'input, Tok, T>
where
    Tok: EngineToken,
{
    pub(crate) fn alternative(
        &mut self,
        parser: impl FnOnce(&mut Engine<'input, Tok>) -> PegResult<T>,
    ) {
        if self.result.is_some() {
            return;
        }

        self.engine.restore(self.start);
        self.engine.failures = self.base_failures.clone();
        match parser(self.engine) {
            Ok(value) => self.result = Some(value),
            Err(failure) => match &mut self.failure {
                Some(existing) => existing.merge(failure),
                None => self.failure = Some(failure),
            },
        }
    }

    fn finish(self) -> PegResult<T> {
        if let Some(value) = self.result {
            return Ok(value);
        }

        self.engine.restore(self.start);
        self.engine.failures = self.base_failures;
        let failure = self.failure.unwrap_or_else(|| {
            Failure::new(self.start.0, self.engine.current_span(), "alternative")
        });
        self.engine.record_failure(failure.clone());
        Err(failure)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct RuleId(pub(crate) &'static str);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MemoEntry<T> {
    Success { value: T, next: Pos },
    Failure(Failure),
}

#[derive(Clone, Debug)]
pub(crate) struct MemoCache<K, T> {
    entries: BTreeMap<(K, Pos), MemoEntry<T>>,
    stats: BTreeMap<K, RuleStats>,
}

impl<K, T> Default for MemoCache<K, T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            stats: BTreeMap::new(),
        }
    }
}

impl<K, T> MemoCache<K, T>
where
    K: Copy + Ord,
    T: Clone,
{
    pub(crate) fn rule<'input, Tok>(
        &mut self,
        engine: &mut Engine<'input, Tok>,
        rule: K,
        parser: impl FnOnce(&mut Engine<'input, Tok>) -> PegResult<T>,
    ) -> PegResult<T>
    where
        Tok: EngineToken,
    {
        let start = engine.pos();
        self.stats.entry(rule).or_default().calls += 1;

        if let Some(entry) = self.entries.get(&(rule, start)).cloned() {
            self.stats.entry(rule).or_default().hits += 1;
            match entry {
                MemoEntry::Success { value, next } => {
                    engine.restore(Mark(next));
                    return Ok(value);
                }
                MemoEntry::Failure(failure) => {
                    engine.restore(Mark(start));
                    engine.record_failure(failure.clone());
                    return Err(failure);
                }
            }
        }

        let mark = engine.mark();
        match parser(engine) {
            Ok(value) => {
                let next = engine.pos();
                self.entries.insert(
                    (rule, start),
                    MemoEntry::Success {
                        value: value.clone(),
                        next,
                    },
                );
                self.stats.entry(rule).or_default().stores += 1;
                Ok(value)
            }
            Err(failure) => {
                engine.restore(mark);
                self.entries
                    .insert((rule, start), MemoEntry::Failure(failure.clone()));
                self.stats.entry(rule).or_default().stores += 1;
                Err(failure)
            }
        }
    }

    pub(crate) fn stats(&self, rule: K) -> RuleStats {
        self.stats.get(&rule).copied().unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RuleStats {
    pub(crate) calls: usize,
    pub(crate) hits: usize,
    pub(crate) stores: usize,
}

pub(crate) fn span_at<T>(tokens: &[T], eof: Span, pos: usize) -> Span
where
    T: EngineToken,
{
    tokens.get(pos).map(EngineToken::span).unwrap_or(eof)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use rex_lexer::span::{Position, Span};

    use super::*;

    fn span(col: usize) -> Span {
        Span::new(1, col, 1, col + 1)
    }

    #[test]
    fn input_strips_comment_ranges() {
        let input = Input::new(Tokens {
            items: vec![
                Token::Int(1, span(1)),
                Token::CommentL(span(2)),
                Token::Ident("ignored".to_string(), span(3)),
                Token::CommentR(span(4)),
                Token::Int(2, span(5)),
            ],
            eof: Span::from_begin_end(Position::new(1, 6), Position::new(1, 6)),
        });

        let (tokens, _) = input.into_parts();
        assert_eq!(tokens, vec![Token::Int(1, span(1)), Token::Int(2, span(5))]);
    }

    #[test]
    fn failures_merge_expected_labels_at_farthest_position() {
        let mut tracker = FailureTracker::default();
        tracker.record(1, span(1), "identifier");
        tracker.record(2, span(2), "expression");
        tracker.record(2, span(2), "`;`");

        let failure = tracker.farthest().expect("expected failure");
        assert_eq!(failure.pos, Pos(2));
        assert!(failure.expected.contains("expression"));
        assert!(failure.expected.contains("`;`"));
        assert!(!failure.expected.contains("identifier"));
    }

    fn tokens(items: Vec<Token>) -> Input {
        Input::new(Tokens {
            items,
            eof: Span::from_begin_end(Position::new(1, 99), Position::new(1, 99)),
        })
    }

    fn int_value(engine: &mut Engine<'_>) -> PegResult<u64> {
        engine.satisfy("integer", |token| match token {
            Token::Int(value, ..) => Some(*value),
            _ => None,
        })
    }

    fn ident_value(engine: &mut Engine<'_>) -> PegResult<String> {
        engine.satisfy("identifier", |token| match token {
            Token::Ident(value, ..) => Some(value.clone()),
            _ => None,
        })
    }

    #[test]
    fn sequence_rolls_back_cursor_on_failure() {
        let input = tokens(vec![
            Token::Int(1, span(1)),
            Token::Ident("x".into(), span(2)),
        ]);
        let mut engine = input.engine();

        let result = engine.sequence(|engine| {
            int_value(engine)?;
            engine.expect("boolean", |token| matches!(token, Token::Bool(..)))
        });

        assert!(result.is_err());
        assert_eq!(engine.pos(), Pos(0));
        let failure = engine.farthest_failure().expect("expected failure");
        assert_eq!(failure.pos, Pos(1));
        assert!(failure.expected.contains("boolean"));
    }

    #[test]
    fn ordered_choice_backtracks_and_uses_first_success() {
        let input = tokens(vec![Token::Int(7, span(1))]);
        let mut engine = input.engine();

        let parsed = engine
            .choice(|choice| {
                choice.alternative(|engine| ident_value(engine).map(|_| "ident"));
                choice.alternative(|engine| int_value(engine).map(|_| "int"));
            })
            .expect("expected second alternative to match");

        assert_eq!(parsed, "int");
        assert_eq!(engine.pos(), Pos(1));
        assert!(engine.farthest_failure().is_none());
    }

    #[test]
    fn optional_and_repeat_suppress_their_stopping_failure() {
        let input = tokens(vec![
            Token::Int(1, span(1)),
            Token::Int(2, span(2)),
            Token::Ident("done".into(), span(3)),
        ]);
        let mut engine = input.engine();

        let missing = engine.optional(ident_value).expect("optional succeeds");
        assert_eq!(missing, None);
        assert_eq!(engine.pos(), Pos(0));
        assert!(engine.farthest_failure().is_none());

        let ints = engine.repeat(int_value);
        assert_eq!(ints, vec![1, 2]);
        assert_eq!(engine.pos(), Pos(2));
        assert!(engine.farthest_failure().is_none());
    }

    #[test]
    fn lookahead_predicates_do_not_consume_tokens() {
        let input = tokens(vec![Token::Ident("x".into(), span(1))]);
        let mut engine = input.engine();

        engine
            .and_predicate("identifier", ident_value)
            .expect("positive lookahead");
        assert_eq!(engine.pos(), Pos(0));

        engine
            .not_predicate("not integer", int_value)
            .expect("negative lookahead");
        assert_eq!(engine.pos(), Pos(0));

        assert_eq!(ident_value(&mut engine).expect("identifier"), "x");
        assert_eq!(engine.pos(), Pos(1));
    }

    #[test]
    fn memo_cache_reuses_successful_rule_entry() {
        let input = tokens(vec![Token::Int(42, span(1))]);
        let mut engine = input.engine();
        let mut memo = MemoCache::<RuleId, u64>::default();
        let calls = Cell::new(0);
        let rule = RuleId("Int");

        let first = memo
            .rule(&mut engine, rule, |engine| {
                calls.set(calls.get() + 1);
                int_value(engine)
            })
            .expect("first parse");
        engine.restore(Mark(Pos(0)));
        let second = memo
            .rule(&mut engine, rule, |engine| {
                calls.set(calls.get() + 1);
                int_value(engine)
            })
            .expect("memo hit");

        assert_eq!((first, second), (42, 42));
        assert_eq!(calls.get(), 1);
        assert_eq!(engine.pos(), Pos(1));
        assert_eq!(
            memo.stats(rule),
            RuleStats {
                calls: 2,
                hits: 1,
                stores: 1
            }
        );
    }
}
