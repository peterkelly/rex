#![forbid(unsafe_code)]

//! Parsing for Rex.
//!
//! The parser is written to be straightforward to step through in a debugger:
//! no parser-generator indirection, and (mostly) explicit control flow.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    vec,
};

use rex_ast::expr::{
    intern, ClassDecl, ClassMethodSig, Decl, Expr, FnDecl, InstanceDecl, InstanceMethodImpl,
    Pattern, Program, Scope, Symbol, TypeConstraint, TypeDecl, TypeExpr, TypeVariant, Var,
};
use rex_lexer::{
    span::{Position, Span, Spanned},
    Token, Tokens,
};

use crate::{error::ParserErr, op::Operator};

pub mod error;
pub mod op;

pub struct Parser {
    token_cursor: usize,
    tokens: Vec<Token>,
    eof: Span,
    errors: Vec<ParserErr>,
}

impl Parser {
    fn operator_token_name(token: &Token) -> Option<(&'static str, Span)> {
        match token {
            Token::Add(span, ..) => Some(("+", *span)),
            Token::And(span, ..) => Some(("&&", *span)),
            Token::Concat(span, ..) => Some(("++", *span)),
            Token::Div(span, ..) => Some(("/", *span)),
            Token::Eq(span, ..) => Some(("==", *span)),
            Token::Ne(span, ..) => Some(("!=", *span)),
            Token::Ge(span, ..) => Some((">=", *span)),
            Token::Gt(span, ..) => Some((">", *span)),
            Token::Le(span, ..) => Some(("<=", *span)),
            Token::Lt(span, ..) => Some(("<", *span)),
            Token::Mod(span, ..) => Some(("%", *span)),
            Token::Mul(span, ..) => Some(("*", *span)),
            Token::Or(span, ..) => Some(("||", *span)),
            Token::Sub(span, ..) => Some(("-", *span)),
            _ => None,
        }
    }

    fn parse_value_name(&mut self) -> Result<(Symbol, Span), ParserErr> {
        match self.current_token() {
            Token::Ident(name, span, ..) => {
                let span = span;
                self.next_token();
                Ok((intern(&name), span))
            }
            token if Self::operator_token_name(&token).is_some() => {
                let (name, span) = Self::operator_token_name(&token)
                    .expect("checked operator_token_name is_some");
                self.next_token();
                Ok((intern(name), span))
            }
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected identifier or operator name got {}", token),
            )),
        }
    }

    pub fn new(tokens: Tokens) -> Parser {
        let mut parser = Parser {
            token_cursor: 0,
            tokens: tokens
                .items
                .into_iter()
                .filter_map(|token| match token {
                    Token::Whitespace(..) => None,
                    token => Some(token),
                })
                .collect(),
            eof: tokens.eof,
            errors: Vec::new(),
        };
        // println!("tokens = {:#?}", parser.tokens);
        parser.strip_comments();
        parser
    }

    fn skip_newlines(&mut self) {
        while self.token_cursor < self.tokens.len() {
            if matches!(self.tokens[self.token_cursor], Token::WhitespaceNewline(..)) {
                self.token_cursor += 1;
                continue;
            }
            break;
        }
    }

    fn current_token(&mut self) -> Token {
        self.skip_newlines();
        if self.token_cursor < self.tokens.len() {
            return self.tokens[self.token_cursor].clone();
        }
        Token::Eof(self.eof)
    }

    fn peek_token(&mut self, n: usize) -> Token {
        self.skip_newlines();
        let mut cursor = self.token_cursor;
        let mut seen = 0usize;
        while cursor < self.tokens.len() {
            if matches!(self.tokens[cursor], Token::WhitespaceNewline(..)) {
                cursor += 1;
                continue;
            }
            if seen == n {
                return self.tokens[cursor].clone();
            }
            seen += 1;
            cursor += 1;
        }
        Token::Eof(self.eof)
    }

    fn next_token(&mut self) {
        self.token_cursor += 1;
        self.skip_newlines();
    }

    // Advances by exactly one token and does *not* skip newlines.
    //
    // We use this for layout-sensitive headers (`class`/`instance`) where the
    // newline boundary matters. Most of the parser treats newlines as whitespace,
    // but for optional-`where` method blocks we need to know what was on the
    // header line vs. what starts the indented block.
    fn next_token_raw(&mut self) {
        self.token_cursor += 1;
    }

    fn strip_comments(&mut self) {
        let mut cursor = 0;

        while cursor < self.tokens.len() {
            match self.tokens[cursor] {
                Token::CommentL(..) => {
                    self.tokens.remove(cursor);
                    while cursor < self.tokens.len() {
                        if let Token::CommentR(..) = self.tokens[cursor] {
                            self.tokens.remove(cursor);
                            break;
                        }
                        self.tokens.remove(cursor);
                    }
                }
                _ => {
                    cursor += 1;
                    continue;
                }
            }
        }
    }

    fn record_error(&mut self, e: ParserErr) {
        self.errors.push(e);
    }

    pub fn parse_program(&mut self) -> Result<Program, Vec<ParserErr>> {
        let mut decls = Vec::new();
        loop {
            match self.current_token() {
                Token::Type(..) => match self.parse_type_decl() {
                    Ok(decl) => decls.push(Decl::Type(decl)),
                    Err(e) => {
                        self.record_error(e);
                        break;
                    }
                },
                Token::Fn(..) => match self.parse_fn_decl() {
                    Ok(decl) => decls.push(Decl::Fn(decl)),
                    Err(e) => {
                        self.record_error(e);
                        break;
                    }
                },
                Token::Class(..) => match self.parse_class_decl() {
                    Ok(decl) => decls.push(Decl::Class(decl)),
                    Err(e) => {
                        self.record_error(e);
                        break;
                    }
                },
                Token::Instance(..) => match self.parse_instance_decl() {
                    Ok(decl) => decls.push(Decl::Instance(decl)),
                    Err(e) => {
                        self.record_error(e);
                        break;
                    }
                },
                _ => break,
            }
        }

        let expr = match self.parse_expr() {
            Ok(expr) => expr,
            Err(e) => {
                self.record_error(e);
                return Err(self.errors.clone());
            }
        };

        // Make sure there's no trailing tokens. The whole program
        // should be one expression.
        match self.current_token() {
            Token::Eof(..) => {}
            token => self.record_error(ParserErr::new(
                *token.span(),
                format!("unexpected {}", token),
            )),
        }

        if !self.errors.is_empty() {
            Err(self.errors.clone())
        } else {
            Ok(Program {
                decls,
                expr: Arc::new(expr),
            })
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, ParserErr> {
        let lhs_expr = self.parse_unary_expr()?;
        self.parse_binary_expr(lhs_expr)
    }

    fn parse_binary_expr(&mut self, lhs_expr: Expr) -> Result<Expr, ParserErr> {
        let lhs_expr_span = lhs_expr.span();

        // Get the next token.
        let token = match self.current_token() {
            // Having no next token should finish the parsing of the binary
            // expression.
            Token::Eof(..) => return Ok(lhs_expr),
            token => token,
        };
        let prec = token.precedence();

        // Parse the binary operator.
        let operator = match token {
            Token::Add(..) => Operator::Add,
            Token::And(..) => Operator::And,
            Token::Concat(..) => Operator::Concat,
            Token::Div(..) => Operator::Div,
            Token::Eq(..) => Operator::Eq,
            Token::Ne(..) => Operator::Ne,
            Token::Ge(..) => Operator::Ge,
            Token::Gt(..) => Operator::Gt,
            Token::Le(..) => Operator::Le,
            Token::Lt(..) => Operator::Lt,
            Token::Mod(..) => Operator::Mod,
            Token::Mul(..) => Operator::Mul,
            Token::Or(..) => Operator::Or,
            Token::Sub(..) => Operator::Sub,
            _ => {
                return Ok(lhs_expr);
            }
        };
        let operator_span = token.span();

        // We have now decided that this token can be parsed so we consume it.
        self.next_token();

        // Parse the next part of this binary expression.
        let rhs_expr = self.parse_unary_expr()?;
        let rhs_expr_span = *rhs_expr.span();

        let next_binary_expr_takes_precedence = match self.current_token() {
            // No more tokens
            Token::Eof(..) => false,
            // Next token has lower precedence
            token if prec > token.precedence() => false,
            // Next token has the same precedence
            token if prec == token.precedence() => match token {
                // But it is left-associative
                Token::Add(..)
                | Token::And(..)
                | Token::Concat(..)
                | Token::Div(..)
                | Token::Mul(..)
                | Token::Mod(..)
                | Token::Or(..)
                | Token::Sub(..) => false,
                // But it is right-associative
                _ => true,
            },
            // Next token has higher precedence
            _ => true,
        };

        let rhs_expr = if next_binary_expr_takes_precedence {
            self.parse_binary_expr(rhs_expr)?
        } else {
            rhs_expr
        };

        let inner_span = Span::from_begin_end(lhs_expr_span.begin, operator_span.end);
        let outer_span = Span::from_begin_end(lhs_expr_span.begin, rhs_expr_span.end);

        self.parse_binary_expr(Expr::App(
            outer_span,
            Arc::new(Expr::App(
                inner_span,
                Arc::new(Expr::Var(Var::with_span(
                    *operator_span,
                    operator.to_string(),
                ))),
                Arc::new(lhs_expr),
            )),
            Arc::new(rhs_expr),
        ))
    }

    fn parse_unary_expr(&mut self) -> Result<Expr, ParserErr> {
        // println!("parse_unary_expr: self.current_token() = {:#?}", self.current_token());
        let mut call_base_expr = self.parse_atom_expr()?;
        let call_base_expr_span = *call_base_expr.span();

        let mut call_arg_exprs = VecDeque::new();
        loop {
            let call_arg_expr = match self.current_token() {
                Token::ParenL(..)
                | Token::BracketL(..)
                | Token::BraceL(..)
                | Token::Bool(..)
                | Token::Float(..)
                | Token::Int(..)
                | Token::String(..)
                | Token::Ident(..)
                | Token::BackSlash(..)
                | Token::Let(..)
                | Token::If(..)
                | Token::Match(..) => self.parse_atom_expr(),
                _ => break,
            }?;
            call_arg_exprs.push_back(call_arg_expr);
        }

        while let Some(call_arg_expr) = call_arg_exprs.pop_front() {
            let call_arg_expr_span_end = call_arg_expr.span().end;
            call_base_expr = Expr::App(
                Span::from_begin_end(call_base_expr_span.begin, call_arg_expr_span_end),
                Arc::new(call_base_expr),
                Arc::new(call_arg_expr),
            );
        }
        loop {
            match self.current_token() {
                Token::Dot(..) | Token::Colon(..) => {
                    self.next_token();
                }
                _ => break,
            }
            let field = match self.current_token() {
                Token::Ident(name, span, ..) => {
                    let name = intern(&name);
                    let end = span.end;
                    self.next_token();
                    (name, end)
                }
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        "expected field name after `.`",
                    ));
                }
            };
            let span = Span::from_begin_end(call_base_expr.span().begin, field.1);
            call_base_expr = Expr::Project(span, Arc::new(call_base_expr), field.0);
        }
        loop {
            match self.current_token() {
                Token::Is(..) => {
                    self.next_token();
                    let ann = self.parse_type_expr()?;
                    let span = Span::from_begin_end(call_base_expr.span().begin, ann.span().end);
                    call_base_expr = Expr::Ann(span, Arc::new(call_base_expr), ann);
                }
                _ => break,
            }
        }
        Ok(call_base_expr)
    }

    fn parse_atom_expr(&mut self) -> Result<Expr, ParserErr> {
        match self.current_token() {
            Token::ParenL(..) => self.parse_paren_expr(),
            Token::BracketL(..) => self.parse_bracket_expr(),
            Token::BraceL(..) => self.parse_brace_expr(),
            Token::Bool(..) => self.parse_literal_bool_expr(),
            Token::Float(..) => self.parse_literal_float_expr(),
            Token::Int(..) => self.parse_literal_int_expr(),
            Token::String(..) => self.parse_literal_str_expr(),
            Token::Ident(..) => self.parse_ident_expr(),
            Token::BackSlash(..) => self.parse_lambda_expr(),
            Token::Let(..) => self.parse_let_expr(),
            Token::If(..) => self.parse_if_expr(),
            Token::Match(..) => self.parse_match_expr(),
            Token::Sub(..) => self.parse_neg_expr(),
            Token::Eof(span) => Err(ParserErr::new(span, "unexpected EOF".to_string())),
            token => Err(ParserErr::new(
                *token.span(),
                format!("unexpected {}", token),
            )),
        }
    }

    fn parse_paren_expr(&mut self) -> Result<Expr, ParserErr> {
        // Eat the left parenthesis.
        let span_begin = match self.current_token() {
            Token::ParenL(span, ..) => {
                self.next_token();
                span
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `(` got {}", token),
                ));
            }
        };

        // Parse the inner expression.
        let expr = match self.current_token() {
            Token::ParenR(span, ..) => {
                self.next_token();
                // Empty tuple
                return Ok(Expr::Tuple(
                    Span::from_begin_end(span_begin.begin, span.end),
                    vec![],
                ));
            }
            Token::Add(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, "+"))
            }
            Token::And(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, "&&"))
            }
            Token::Concat(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, "++"))
            }
            Token::Div(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, "/"))
            }
            Token::Eq(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, "=="))
            }
            Token::Ge(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, ">="))
            }
            Token::Gt(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, ">"))
            }
            Token::Le(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, "<="))
            }
            Token::Lt(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, "<"))
            }
            Token::Mod(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, "%"))
            }
            Token::Mul(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, "*"))
            }
            Token::Or(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, "||"))
            }
            Token::Sub(span, ..) => {
                if let Token::ParenR(..) = self.peek_token(1) {
                    // In the case of the `-` operator we need to explicitly
                    // check for the closing right parenthesis, because it is
                    // valid to have an expressions like `(- 69)`. This is
                    // different from other operators, because it is not valid
                    // to have an expression like `(+ 69)`` or `(>= 3)``.
                    //
                    // It would not be a crazy idea to explicitly check for the
                    // closing right parenthesis in other operators. Although we
                    // do not want to allow expressions like `(+ 420)` the
                    // explicit check will allow for better error messages.
                    self.next_token();
                    Expr::Var(Var::with_span(span, "-"))
                } else {
                    self.parse_expr()?
                }
            }
            _ => self.parse_expr()?,
        };

        // Eat the right parenthesis.
        let span_end = match self.current_token() {
            Token::ParenR(span, ..) => {
                self.next_token();
                span
            }
            Token::Comma(..) => {
                // parse inner expressions
                return self.parse_tuple(span_begin, expr);
            }
            token => {
                self.record_error(ParserErr::new(*token.span(), "expected `)`"));
                return Ok(expr);
            }
        };

        let expr = expr.with_span_begin_end(span_begin.begin, span_end.end);

        Ok(expr)
    }

    fn parse_tuple(&mut self, span_begin: Span, first_item: Expr) -> Result<Expr, ParserErr> {
        let mut items = vec![Arc::new(first_item)];
        loop {
            // eat the comma
            match self.current_token() {
                Token::Comma(..) => self.next_token(),
                Token::ParenR(end_span) => {
                    self.next_token();
                    return Ok(Expr::Tuple(
                        Span::from_begin_end(span_begin.begin, end_span.end),
                        items,
                    ));
                }
                _ => items.push(Arc::new(self.parse_expr()?)),
            }
        }
    }

    fn parse_bracket_expr(&mut self) -> Result<Expr, ParserErr> {
        // Eat the left bracket.
        let span_begin = match self.current_token() {
            Token::BracketL(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `[` got {}", token),
                ));
            }
        };

        let mut exprs = Vec::new();
        loop {
            if let Token::BracketR(..) = self.current_token() {
                break;
            }

            // Parse the next expression.
            exprs.push(Arc::new(self.parse_expr()?));
            // Eat the comma.
            match self.current_token() {
                Token::Comma(..) => self.next_token(),
                Token::Eof(span) => {
                    self.record_error(ParserErr::new(span, "expected `,` or `]`"));
                    break;
                }
                _ => {
                    break;
                }
            };
        }

        // Eat the right bracket.
        let span_end = match self.current_token() {
            Token::BracketR(span, ..) => {
                self.next_token();
                span.end
            }
            token => {
                self.record_error(ParserErr::new(
                    *token.span(),
                    format!("expected `]` got {}", token),
                ));

                return Ok(Expr::List(
                    Span::from_begin_end(span_begin, token.span().end),
                    exprs,
                ));
            }
        };

        Ok(Expr::List(
            Span::from_begin_end(span_begin, span_end),
            exprs,
        ))
    }

    fn parse_brace_expr(&mut self) -> Result<Expr, ParserErr> {
        // Eat the left brace.
        let span_begin = match self.current_token() {
            Token::BraceL(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `{{` got {}", token),
                ));
            }
        };

        // `{}` is always an empty dict literal.
        if let Token::BraceR(span, ..) = self.current_token() {
            self.next_token();
            return Ok(Expr::Dict(
                Span::from_begin_end(span_begin, span.end),
                BTreeMap::new(),
            ));
        }

        // Disambiguate:
        // - `{ x = e }` is a dict literal
        // - `{ base with { x = e } }` is a record-update expression
        let looks_like_dict_literal = matches!(self.current_token(), Token::Ident(..))
            && matches!(self.peek_token(1), Token::Assign(..));

        if looks_like_dict_literal {
            return self.parse_dict_expr_after_lbrace(span_begin);
        }

        let base = match self.parse_expr() {
            Ok(expr) => expr,
            Err(err) => {
                self.record_error(err);
                while !matches!(self.current_token(), Token::BraceR(..) | Token::Eof(..)) {
                    self.next_token();
                }
                if let Token::BraceR(..) = self.current_token() {
                    self.next_token();
                }
                return Ok(Expr::Dict(
                    Span::from_begin_end(span_begin, Position::new(0, 0)),
                    BTreeMap::new(),
                ));
            }
        };

        match self.current_token() {
            Token::With(..) => self.next_token(),
            token => {
                self.record_error(ParserErr::new(*token.span(), "expected `with`"));
                while !matches!(self.current_token(), Token::BraceR(..) | Token::Eof(..)) {
                    self.next_token();
                }
                if let Token::BraceR(..) = self.current_token() {
                    self.next_token();
                }
                return Ok(base);
            }
        };

        let updates = match self.parse_dict_expr() {
            Ok(Expr::Dict(_, kvs)) => kvs,
            Ok(other) => {
                self.record_error(ParserErr::new(*other.span(), "expected `{...}`"));
                BTreeMap::new()
            }
            Err(err) => {
                self.record_error(err);
                BTreeMap::new()
            }
        };

        // Eat the right brace of the update expression.
        let span_end = match self.current_token() {
            Token::BraceR(span, ..) => {
                self.next_token();
                span.end
            }
            token => {
                self.record_error(ParserErr::new(
                    *token.span(),
                    format!("expected `}}` got {}", token),
                ));
                Position::new(0, 0)
            }
        };

        Ok(Expr::RecordUpdate(
            Span::from_begin_end(span_begin, span_end),
            Arc::new(base),
            updates,
        ))
    }

    fn parse_dict_expr(&mut self) -> Result<Expr, ParserErr> {
        // Eat the left brace.
        let span_begin = match self.current_token() {
            Token::BraceL(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `{{` got {}", token),
                ));
            }
        };

        self.parse_dict_expr_after_lbrace(span_begin)
    }

    fn parse_dict_expr_after_lbrace(&mut self, span_begin: Position) -> Result<Expr, ParserErr> {
        let mut kvs = Vec::new();
        loop {
            if let Token::BraceR(..) = self.current_token() {
                break;
            }

            // Parse the ident.
            let var = match self.parse_ident_expr()? {
                Expr::Var(var) => var,
                _ => unreachable!(),
            };
            // Eat the =.
            match self.current_token() {
                Token::Assign(..) => self.next_token(),
                token => {
                    self.record_error(ParserErr::new(*token.span(), "expected `=`"));
                    break;
                }
            };
            // Parse the expression.
            kvs.push((var.name, Arc::new(self.parse_expr()?)));
            // Eat the comma.
            match self.current_token() {
                Token::Comma(..) => self.next_token(),
                Token::Eof(span) => {
                    self.record_error(ParserErr::new(span, "expected `,` or `}`"));
                    break;
                }
                _ => {
                    break;
                }
            };
        }

        // Eat the right brace.
        let span_end = match self.current_token() {
            Token::BraceR(span, ..) => {
                self.next_token();
                span.end
            }
            token => {
                self.record_error(ParserErr::new(
                    *token.span(),
                    format!("expected `}}` got {}", token),
                ));

                return Ok(Expr::Dict(
                    Span::from_begin_end(span_begin, Position::new(0, 0)),
                    kvs.into_iter().collect(),
                ));
            }
        };

        Ok(Expr::Dict(
            Span::from_begin_end(span_begin, span_end),
            kvs.into_iter().collect(),
        ))
    }

    fn parse_neg_expr(&mut self) -> Result<Expr, ParserErr> {
        // Eat the minus.
        let span_token = match self.current_token() {
            Token::Sub(span, ..) => {
                self.next_token();
                span
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `-` got {}", token),
                ));
            }
        };

        // Parse the inner expression.
        let expr = self.parse_expr()?;
        let expr_span_end = expr.span().end;

        // Return the negative expression.
        Ok(Expr::App(
            Span::from_begin_end(span_token.begin, expr_span_end),
            Arc::new(Expr::Var(Var::with_span(span_token, "negate"))),
            Arc::new(expr),
        ))
    }

    //
    fn parse_lambda_expr(&mut self) -> Result<Expr, ParserErr> {
        // Eat the backslash.
        let span_begin = match self.current_token() {
            Token::BackSlash(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `\\` got {}", token),
                ));
            }
        };

        // Parse the params.
        let mut params = VecDeque::new();
        loop {
            match self.current_token() {
                Token::Ident(..) | Token::ParenL(..) => {
                    let (var, ann, span) = self.parse_lambda_param()?;
                    params.push_back((span, var, ann));
                }
                _ => break,
            }
        }

        let mut constraints = Vec::new();
        if matches!(self.current_token(), Token::Where(..)) {
            self.next_token();
            constraints = self.parse_type_constraints()?;
        }

        // Parse the arrow.
        let _span_arrow = match self.current_token() {
            Token::ArrowR(span, ..) => {
                self.next_token();
                span
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `->` got {}", token),
                ));
            }
        };

        // Parse the body
        let mut body = self.parse_expr()?;
        let mut body_span_end = body.span().end;
        while let Some((param_span, param, ann)) = params.pop_back() {
            let lam_constraints = if params.is_empty() {
                std::mem::take(&mut constraints)
            } else {
                Vec::new()
            };
            body = Expr::Lam(
                Span::from_begin_end(param_span.begin, body_span_end),
                Scope::new_sync(),
                param,
                ann,
                lam_constraints,
                Arc::new(body),
            );
            body_span_end = body.span().end;
        }
        // Adjust the outer most lambda to include the initial backslash
        let body = body.with_span_begin(span_begin);

        Ok(body)
    }

    fn parse_lambda_param(&mut self) -> Result<(Var, Option<TypeExpr>, Span), ParserErr> {
        match self.current_token() {
            Token::Ident(name, span, ..) => {
                self.next_token();
                let mut ann = None;
                let mut param_span = span;
                if let Token::Colon(..) = self.current_token() {
                    self.next_token();
                    let ann_expr = self.parse_type_expr()?;
                    param_span = Span::from_begin_end(span.begin, ann_expr.span().end);
                    ann = Some(ann_expr);
                }
                Ok((Var::with_span(span, name), ann, param_span))
            }
            Token::ParenL(span_begin, ..) => {
                self.next_token();
                let (name, name_span) = match self.current_token() {
                    Token::Ident(name, span, ..) => {
                        let span = span;
                        self.next_token();
                        (name, span)
                    }
                    token => {
                        return Err(ParserErr::new(
                            *token.span(),
                            format!("expected parameter name got {}", token),
                        ));
                    }
                };
                match self.current_token() {
                    Token::Colon(..) => self.next_token(),
                    token => {
                        return Err(ParserErr::new(
                            *token.span(),
                            format!("expected `:` got {}", token),
                        ));
                    }
                }
                let ann = self.parse_type_expr()?;
                let span_end = match self.current_token() {
                    Token::ParenR(span, ..) => {
                        self.next_token();
                        span.end
                    }
                    token => {
                        return Err(ParserErr::new(
                            *token.span(),
                            format!("expected `)` got {}", token),
                        ));
                    }
                };
                Ok((
                    Var::with_span(name_span, name),
                    Some(ann),
                    Span::from_begin_end(span_begin.begin, span_end),
                ))
            }
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected lambda param got {}", token),
            )),
        }
    }

    fn parse_type_constraints(&mut self) -> Result<Vec<TypeConstraint>, ParserErr> {
        let mut constraints = Vec::new();
        loop {
            let class = match self.current_token() {
                Token::Ident(name, _span, ..) => {
                    let name = intern(&name);
                    self.next_token();
                    name
                }
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected type class name got {}", token),
                    ));
                }
            };

            let typ = self.parse_type_app()?;
            constraints.push(TypeConstraint::new(class, typ));

            match self.current_token() {
                Token::Comma(..) => {
                    self.next_token();
                    continue;
                }
                _ => break,
            }
        }
        Ok(constraints)
    }

    //
    fn parse_let_expr(&mut self) -> Result<Expr, ParserErr> {
        // Eat the `let` token
        let span_begin = match self.current_token() {
            Token::Let(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `let` got {}", token),
                ));
            }
        };

        // Parse the variable declarations.
        let mut decls = VecDeque::new();
        // Variable name
        while let Token::Ident(val, span, ..) = self.current_token() {
            self.next_token();
            let var = (span, val);
            let mut ann = None;
            if let Token::Colon(..) = self.current_token() {
                self.next_token();
                ann = Some(self.parse_type_expr()?);
            }

            // =
            match self.current_token() {
                Token::Assign(_span, ..) => {
                    self.next_token();
                }
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `=` got {}", token),
                    ));
                }
            }
            // Parse the variable definition
            decls.push_back((var, ann, self.parse_expr()?));
            // Parse `,` or `in`
            match self.current_token() {
                Token::Comma(_span, ..) => {
                    self.next_token();
                    continue;
                }
                Token::In(..) => break,
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `,` or `in` got {}", token),
                    ));
                }
            }
        }

        // Parse the `in` token
        let _span_arrow = match self.current_token() {
            Token::In(span, ..) => {
                self.next_token();
                span
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `in` got {}", token),
                ));
            }
        };

        // Parse the body
        let mut body = self.parse_expr()?;
        let mut body_span_end = body.span().end;
        while let Some(((var_span, var), ann, def)) = decls.pop_back() {
            body = Expr::Let(
                Span::from_begin_end(var_span.begin, body_span_end),
                Var::with_span(var_span, var),
                ann,
                Arc::new(def),
                Arc::new(body),
            );
            body_span_end = body.span().end;
        }
        // Adjust the outer most let-in expression to include the initial let
        // token
        let body = body.with_span_begin(span_begin);

        Ok(body)
    }

    //
    fn parse_if_expr(&mut self) -> Result<Expr, ParserErr> {
        // Eat the `if` token
        let span_begin = match self.current_token() {
            Token::If(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `if` got {}", token),
                ));
            }
        };

        // Parse the cond expression
        let cond = self.parse_expr()?;

        // Parse the `then` token
        let _span_arrow = match self.current_token() {
            Token::Then(span, ..) => {
                self.next_token();
                span
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `then` got {}", token),
                ));
            }
        };

        // Parse the then expression
        let then = self.parse_expr()?;

        // Parse the `else` token
        let _span_arrow = match self.current_token() {
            Token::Else(span, ..) => {
                self.next_token();
                span
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `else` got {}", token),
                ));
            }
        };

        // Parse the else expression
        let r#else = self.parse_expr()?;
        let else_span_end = r#else.span().end;

        Ok(Expr::Ite(
            Span::from_begin_end(span_begin, else_span_end),
            Arc::new(cond),
            Arc::new(then),
            Arc::new(r#else),
        ))
    }

    fn parse_match_expr(&mut self) -> Result<Expr, ParserErr> {
        // Eat the `match` token
        let span_begin = match self.current_token() {
            Token::Match(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `match` got {}", token),
                ));
            }
        };

        let scrutinee = self.parse_atom_expr()?;
        let mut arms = Vec::new();
        loop {
            match self.current_token() {
                Token::When(..) => {
                    self.next_token();
                }
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `when` got {}", token),
                    ));
                }
            }

            let pattern = self.parse_pattern()?;

            match self.current_token() {
                Token::ArrowR(..) => self.next_token(),
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `->` got {}", token),
                    ));
                }
            }

            let expr = self.parse_expr()?;
            arms.push((pattern, Arc::new(expr)));

            match self.current_token() {
                Token::When(..) => continue,
                _ => break,
            }
        }

        let span_end = arms
            .last()
            .map(|(_, expr)| expr.span().end)
            .unwrap_or_else(|| scrutinee.span().end);

        Ok(Expr::Match(
            Span::from_begin_end(span_begin, span_end),
            Arc::new(scrutinee),
            arms,
        ))
    }

    fn parse_type_expr_slice(&self, slice: &[Token]) -> Result<TypeExpr, ParserErr> {
        if slice.is_empty() {
            return Err(ParserErr::new(self.eof, "expected type".to_string()));
        }
        let eof = *slice.last().unwrap().span();
        let tokens = Tokens {
            items: slice.to_vec(),
            eof,
        };
        let mut parser = Parser::new(tokens);
        let expr = parser.parse_type_expr()?;
        match parser.current_token() {
            Token::Eof(..) => Ok(expr),
            token => Err(ParserErr::new(
                *token.span(),
                format!("unexpected {} in type", token),
            )),
        }
    }

    fn parse_type_app_slice(&self, slice: &[Token]) -> Result<TypeExpr, ParserErr> {
        if slice.is_empty() {
            return Err(ParserErr::new(self.eof, "expected type".to_string()));
        }
        let eof = *slice.last().unwrap().span();
        let tokens = Tokens {
            items: slice.to_vec(),
            eof,
        };
        let mut parser = Parser::new(tokens);
        let expr = parser.parse_type_app()?;
        match parser.current_token() {
            Token::Eof(..) => Ok(expr),
            token => Err(ParserErr::new(
                *token.span(),
                format!("unexpected {} in type", token),
            )),
        }
    }

    fn parse_type_constraints_slice(&self, slice: &[Token]) -> Result<Vec<TypeConstraint>, ParserErr> {
        if slice.is_empty() {
            return Err(ParserErr::new(
                self.eof,
                "expected type constraint".to_string(),
            ));
        }
        let eof = *slice.last().unwrap().span();
        let tokens = Tokens {
            items: slice.to_vec(),
            eof,
        };
        let mut parser = Parser::new(tokens);
        let constraints = parser.parse_type_constraints()?;
        match parser.current_token() {
            Token::Eof(..) => Ok(constraints),
            token => Err(ParserErr::new(
                *token.span(),
                format!("unexpected {} in type constraints", token),
            )),
        }
    }

    fn parse_expr_slice(&self, slice: &[Token]) -> Result<Expr, ParserErr> {
        if slice.is_empty() {
            return Err(ParserErr::new(self.eof, "expected expression".to_string()));
        }
        let eof = *slice.last().unwrap().span();
        let tokens = Tokens {
            items: slice.to_vec(),
            eof,
        };
        let mut parser = Parser::new(tokens);
        let expr = parser.parse_expr()?;
        match parser.current_token() {
            Token::Eof(..) => Ok(expr),
            token => Err(ParserErr::new(
                *token.span(),
                format!("unexpected {} in expression", token),
            )),
        }
    }

    fn parse_class_decl(&mut self) -> Result<ClassDecl, ParserErr> {
        let span_begin = match self.current_token() {
            Token::Class(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `class` got {}", token),
                ));
            }
        };

        let (name, name_span) = match self.current_token() {
            Token::Ident(name, span, ..) => {
                let span = span;
                self.next_token_raw();
                (intern(&name), span)
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected class name got {}", token),
                ));
            }
        };

        let class_indent = span_begin.column;
        let header_line = name_span.begin.line;
        let mut header_end = name_span.end;

        let mut params = Vec::new();
        while let Some(Token::Ident(p, span, ..)) = self.tokens.get(self.token_cursor).cloned() {
            // Parameters only live on the header line. Newlines are significant
            // here because `where` is optional and the next line may be a method
            // block.
            if span.begin.line != header_line {
                break;
            }
            header_end = header_end.max(span.end);
            params.push(intern(&p));
            self.next_token_raw();
        }

        let mut supers = Vec::new();
        if let Some(Token::Le(span, ..)) = self.tokens.get(self.token_cursor).cloned() {
            if span.begin.line == header_line {
                self.next_token_raw();
                let start_idx = self.token_cursor;
                let end_idx = find_layout_header_clause_end(&self.tokens, start_idx, |t| {
                    matches!(t, Token::Where(..))
                });
                supers = self.parse_type_constraints_slice(&self.tokens[start_idx..end_idx])?;
                self.token_cursor = end_idx;
            }
            if let Some(last) = supers.last() {
                header_end = header_end.max(last.typ.span().end);
            }
        }

        // `where` is optional. If it is omitted, we only treat the following
        // layout block as a list of method signatures when it is clearly a
        // method block:
        // - It is indented more than the `class` keyword, AND
        // - It starts with `name : ...` (operator names allowed).
        //
        // This keeps parsing unambiguous and prevents accidentally consuming
        // the program expression as if it were method signatures.
        let mut saw_where = false;
        let where_span = match self.current_token() {
            Token::Where(span, ..) => {
                self.next_token();
                header_end = header_end.max(span.end);
                saw_where = true;
                span
            }
            _ => Span::default(),
        };

        // Method signatures are a layout block: each signature starts at the
        // indentation of the first method name after `where` (or after the
        // class header if `where` is omitted).
        let mut methods = Vec::new();
        let implicit_method_start = if saw_where {
            false
        } else {
            let token = self.current_token();
            let token_span = *token.span();
            let next = self.peek_token(1);
            token_span.begin.column > class_indent
                && ((matches!(token, Token::Ident(..)) && matches!(next, Token::Colon(..)))
                    || (Self::operator_token_name(&token).is_some()
                        && matches!(next, Token::Colon(..))))
        };
        let has_method_block = saw_where || implicit_method_start;
        let block_indent = if has_method_block {
            match self.current_token() {
                Token::Ident(_, span, ..) => Some(span.begin.column),
                token => Self::operator_token_name(&token).map(|(_, span)| span.begin.column),
            }
        } else {
            None
        };

        while has_method_block
            && (matches!(self.current_token(), Token::Ident(..))
                || Self::operator_token_name(&self.current_token()).is_some())
        {
            let span = *self.current_token().span();
            if let Some(indent) = block_indent {
                if span.begin.column != indent {
                    break;
                }
            }
            let (m_name, _name_span) = self.parse_value_name()?;

            match self.current_token() {
                Token::Colon(..) => self.next_token(),
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `:` got {}", token),
                    ));
                }
            }

            let start_idx = self.token_cursor;
            let end_idx = find_layout_expr_end(&self.tokens, start_idx, block_indent, |t, next| {
                matches!(t, Token::Ident(..)) && matches!(next, Token::Colon(..))
                    || Self::operator_token_name(t).is_some() && matches!(next, Token::Colon(..))
            });
            let typ = self.parse_type_expr_slice(&self.tokens[start_idx..end_idx])?;
            self.token_cursor = end_idx;
            self.skip_newlines();
            methods.push(ClassMethodSig { name: m_name, typ });
        }

        let span_end = methods
            .last()
            .map(|m| m.typ.span().end)
            .or_else(|| supers.last().map(|s| s.typ.span().end))
            .unwrap_or(header_end.max(where_span.end));

        Ok(ClassDecl {
            span: Span::from_begin_end(span_begin, span_end),
            name,
            params,
            supers,
            methods,
        })
    }

    fn parse_instance_decl(&mut self) -> Result<InstanceDecl, ParserErr> {
        let span_begin = match self.current_token() {
            Token::Instance(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `instance` got {}", token),
                ));
            }
        };

        let (class, class_span) = match self.current_token() {
            Token::Ident(name, span, ..) => {
                let span = span;
                self.next_token_raw();
                (intern(&name), span)
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected class name got {}", token),
                ));
            }
        };

        let instance_indent = span_begin.column;
        let header_line = class_span.begin.line;

        let start_idx = self.token_cursor;
        let end_idx = find_layout_header_clause_end(&self.tokens, start_idx, |t| {
            matches!(t, Token::Le(..) | Token::Where(..))
        });
        if end_idx == start_idx {
            let span = self
                .tokens
                .get(start_idx)
                .map(|t| *t.span())
                .unwrap_or(self.eof);
            return Err(ParserErr::new(span, "expected type".to_string()));
        }
        let head = self.parse_type_app_slice(&self.tokens[start_idx..end_idx])?;
        self.token_cursor = end_idx;
        let mut header_end = head.span().end;

        let mut context = Vec::new();
        if let Some(Token::Le(span, ..)) = self.tokens.get(self.token_cursor).cloned() {
            if span.begin.line == header_line {
                self.next_token_raw();
                let start_idx = self.token_cursor;
                let end_idx = find_layout_header_clause_end(&self.tokens, start_idx, |t| {
                    matches!(t, Token::Where(..))
                });
                context = self.parse_type_constraints_slice(&self.tokens[start_idx..end_idx])?;
                self.token_cursor = end_idx;
                if let Some(last) = context.last() {
                    header_end = header_end.max(last.typ.span().end);
                }
            }
        }

        // `where` is optional. If it is omitted, we only treat the following
        // layout block as a list of method implementations when it is clearly
        // a method block:
        // - It is indented more than the `instance` keyword, AND
        // - It starts with `name = ...` (operator names allowed).
        let mut saw_where = false;
        let where_span = match self.current_token() {
            Token::Where(span, ..) => {
                self.next_token();
                header_end = header_end.max(span.end);
                saw_where = true;
                span
            }
            _ => Span::default(),
        };

        // Instance method implementations are a layout block. We rely on
        // indentation (token column) to decide where the block ends.
        let has_method_block = if saw_where {
            true
        } else {
            let token = self.current_token();
            let token_span = *token.span();
            let next = self.peek_token(1);
            token_span.begin.column > instance_indent
                && ((matches!(token, Token::Ident(..)) && matches!(next, Token::Assign(..)))
                    || (Self::operator_token_name(&token).is_some()
                        && matches!(next, Token::Assign(..))))
        };

        let block_indent = if has_method_block {
            match self.current_token() {
                Token::Ident(_, span, ..) => Some(span.begin.column),
                token => Self::operator_token_name(&token).map(|(_, span)| span.begin.column),
            }
        } else {
            None
        };

        let mut methods = Vec::new();
        while has_method_block
            && (matches!(self.current_token(), Token::Ident(..))
                || Self::operator_token_name(&self.current_token()).is_some())
        {
            let span = *self.current_token().span();
            if let Some(indent) = block_indent {
                if span.begin.column != indent {
                    break;
                }
            }
            let (name, _name_span) = self.parse_value_name()?;

            match self.current_token() {
                Token::Assign(..) => self.next_token(),
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `=` got {}", token),
                    ));
                }
            }

            let start_idx = self.token_cursor;
            let end_idx = find_layout_expr_end(&self.tokens, start_idx, block_indent, |t, next| {
                matches!(t, Token::Ident(..)) && matches!(next, Token::Assign(..))
                    || Self::operator_token_name(t).is_some() && matches!(next, Token::Assign(..))
            });
            let body = self.parse_expr_slice(&self.tokens[start_idx..end_idx])?;
            self.token_cursor = end_idx;
            self.skip_newlines();

            methods.push(InstanceMethodImpl {
                name,
                body: Arc::new(body),
            });
        }

        let span_end = methods
            .last()
            .map(|m| m.body.span().end)
            .or_else(|| context.last().map(|c| c.typ.span().end))
            .unwrap_or(header_end.max(where_span.end));

        Ok(InstanceDecl {
            span: Span::from_begin_end(span_begin, span_end),
            class,
            head,
            context,
            methods,
        })
    }

    fn parse_fn_decl(&mut self) -> Result<FnDecl, ParserErr> {
        let span_begin = match self.current_token() {
            Token::Fn(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `fn` got {}", token),
                ));
            }
        };

        let (name, name_span) = match self.current_token() {
            Token::Ident(name, span, ..) => {
                let span = span;
                self.next_token();
                (name, span)
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected function name got {}", token),
                ));
            }
        };

        let name_var = Var::with_span(name_span, name);
        let mut params: Vec<(Var, TypeExpr)> = Vec::new();

        let is_named_param_head = |token: &Token, next: &Token| {
            matches!(token, Token::Ident(..)) && matches!(next, Token::Colon(..))
        };

        // Params (new syntax):
        //   fn foo x: a -> y: b -> i32 = ...
        //
        // Params (legacy syntax, still accepted):
        //   fn foo (x: a, y: b) -> i32 = ...
        match (self.current_token(), self.peek_token(1)) {
            (Token::ParenL(..), _) => {
                // (x: a, y: b)
                self.next_token();
                if !matches!(self.current_token(), Token::ParenR(..)) {
                    loop {
                        let (param_name, param_span) = match self.current_token() {
                            Token::Ident(name, span, ..) => {
                                let span = span;
                                self.next_token();
                                (name, span)
                            }
                            token => {
                                return Err(ParserErr::new(
                                    *token.span(),
                                    format!("expected parameter name got {}", token),
                                ));
                            }
                        };
                        match self.current_token() {
                            Token::Colon(..) => self.next_token(),
                            token => {
                                return Err(ParserErr::new(
                                    *token.span(),
                                    format!("expected `:` got {}", token),
                                ));
                            }
                        }
                        let ann = self.parse_type_expr()?;
                        params.push((Var::with_span(param_span, param_name), ann));
                        match self.current_token() {
                            Token::Comma(..) => {
                                self.next_token();
                                continue;
                            }
                            Token::ParenR(..) => break,
                            token => {
                                return Err(ParserErr::new(
                                    *token.span(),
                                    format!("expected `,` or `)` got {}", token),
                                ));
                            }
                        }
                    }
                }

                match self.current_token() {
                    Token::ParenR(..) => self.next_token(),
                    token => {
                        return Err(ParserErr::new(
                            *token.span(),
                            format!("expected `)` got {}", token),
                        ));
                    }
                }

                // `->`
                match self.current_token() {
                    Token::ArrowR(..) => self.next_token(),
                    token => {
                        return Err(ParserErr::new(
                            *token.span(),
                            format!("expected `->` got {}", token),
                        ));
                    }
                }
            }
            (tok, next) if is_named_param_head(&tok, &next) => {
                // x: a -> y: b -> i32
                loop {
                    let (param_name, param_span) = match self.current_token() {
                        Token::Ident(name, span, ..) => {
                            let span = span;
                            self.next_token();
                            (name, span)
                        }
                        token => {
                            return Err(ParserErr::new(
                                *token.span(),
                                format!("expected parameter name got {}", token),
                            ));
                        }
                    };

                    match self.current_token() {
                        Token::Colon(..) => self.next_token(),
                        token => {
                            return Err(ParserErr::new(
                                *token.span(),
                                format!("expected `:` got {}", token),
                            ));
                        }
                    }

                    // Parse the parameter type, stopping at the `->` separator at depth 0.
                    // To use a function type as a parameter type, parentheses are required:
                    //   x: (a -> c) -> ...
                    let ty_start = self.token_cursor;
                    let mut depth = 0usize;
                    let mut arrow_idx = None;
                    let mut stop_span = None;
                    for i in ty_start..self.tokens.len() {
                        match self.tokens[i] {
                            Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => depth += 1,
                            Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                                depth = depth.saturating_sub(1)
                            }
                            Token::ArrowR(..) if depth == 0 => {
                                arrow_idx = Some(i);
                                break;
                            }
                            Token::Assign(span, ..) | Token::Where(span, ..) if depth == 0 => {
                                stop_span = Some(span);
                                break;
                            }
                            _ => {}
                        }
                    }
                    let Some(arrow_idx) = arrow_idx else {
                        let span = stop_span.unwrap_or(self.eof);
                        return Err(ParserErr::new(
                            span,
                            "expected `->` after parameter type".to_string(),
                        ));
                    };
                    let ann = self.parse_type_expr_slice(&self.tokens[ty_start..arrow_idx])?;
                    self.token_cursor = arrow_idx;
                    params.push((Var::with_span(param_span, param_name), ann));

                    match self.current_token() {
                        Token::ArrowR(..) => self.next_token(),
                        token => {
                            return Err(ParserErr::new(
                                *token.span(),
                                format!("expected `->` got {}", token),
                            ));
                        }
                    }

                    let tok = self.current_token();
                    let next = self.peek_token(1);
                    if is_named_param_head(&tok, &next) {
                        continue;
                    }
                    break;
                }
            }
            (token, _) => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `(` or parameter name got {}", token),
                ));
            }
        }

        // Find `=` and (optional) `where` between return type and `=`.
        let ret_start = self.token_cursor;
        let mut depth = 0usize;
        let mut assign_idx = None;
        for i in ret_start..self.tokens.len() {
            match self.tokens[i] {
                Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => depth += 1,
                Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                    depth = depth.saturating_sub(1)
                }
                Token::Assign(..) if depth == 0 => {
                    assign_idx = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let assign_idx = assign_idx.ok_or_else(|| {
            ParserErr::new(self.eof, "expected `=` in function declaration".to_string())
        })?;

        let mut where_idx = None;
        depth = 0;
        for i in ret_start..assign_idx {
            match self.tokens[i] {
                Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => depth += 1,
                Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                    depth = depth.saturating_sub(1)
                }
                Token::Where(..) if depth == 0 => {
                    where_idx = Some(i);
                    break;
                }
                _ => {}
            }
        }

        let ret_end = where_idx.unwrap_or(assign_idx);
        let ret = self.parse_type_expr_slice(&self.tokens[ret_start..ret_end])?;
        self.token_cursor = ret_end;

        let mut constraints = Vec::new();
        if matches!(self.current_token(), Token::Where(..)) {
            self.next_token();
            constraints = self.parse_type_constraints()?;
        }

        // `=`
        match self.current_token() {
            Token::Assign(..) => self.next_token(),
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `=` got {}", token),
                ));
            }
        }

        // Parse a body expression, delimited by newline (unless inside parens/brackets/braces).
        let body_start = self.token_cursor;
        let body_line = match self.tokens.get(body_start) {
            Some(tok) => tok.span().begin.line,
            None => return Err(ParserErr::new(self.eof, "unexpected EOF".to_string())),
        };
        let mut depth = 0usize;
        let mut body_end = body_start;
        for i in body_start..self.tokens.len() {
            let tok = &self.tokens[i];
            if depth == 0 && tok.span().begin.line > body_line {
                break;
            }
            match tok {
                Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => depth += 1,
                Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                    depth = depth.saturating_sub(1)
                }
                _ => {}
            }
            body_end = i + 1;
        }
        let body = self.parse_expr_slice(&self.tokens[body_start..body_end])?;
        self.token_cursor = body_end;
        let span_end = body.span().end;

        Ok(FnDecl {
            span: Span::from_begin_end(span_begin, span_end),
            name: name_var,
            params,
            ret,
            constraints,
            body: Arc::new(body),
        })
    }

    fn parse_type_decl(&mut self) -> Result<TypeDecl, ParserErr> {
        let span_begin = match self.current_token() {
            Token::Type(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `type` got {}", token),
                ));
            }
        };

        let (name, _name_span) = match self.current_token() {
            Token::Ident(name, span, ..) => {
                let name = intern(&name);
                let span = span;
                self.next_token();
                (name, span)
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected type name got {}", token),
                ));
            }
        };

        let mut params = Vec::new();
        while let Token::Ident(_param, ..) = self.current_token() {
            let name = match self.current_token() {
                Token::Ident(param, ..) => intern(&param),
                _ => unreachable!(),
            };
            self.next_token();
            params.push(name);
        }

        match self.current_token() {
            Token::Assign(..) => self.next_token(),
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `=` got {}", token),
                ));
            }
        }

        let (first, first_span) = self.parse_type_variant()?;
        let mut variants = vec![first];
        let mut span_end = first_span.end;
        loop {
            match self.current_token() {
                Token::Pipe(..) => {
                    self.next_token();
                    let (variant, vspan) = self.parse_type_variant()?;
                    span_end = vspan.end;
                    variants.push(variant);
                    continue;
                }
                _ => break,
            }
        }

        Ok(TypeDecl {
            span: Span::from_begin_end(span_begin, span_end),
            name,
            params,
            variants,
        })
    }

    fn parse_type_variant(&mut self) -> Result<(TypeVariant, Span), ParserErr> {
        let (name, name_span) = match self.current_token() {
            Token::Ident(name, span, ..) => {
                let name = intern(&name);
                let span = span;
                self.next_token();
                (name, span)
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected constructor name got {}", token),
                ));
            }
        };

        let mut args = Vec::new();
        let mut span_end = name_span.end;
        loop {
            match self.current_token() {
                Token::Ident(..) | Token::ParenL(..) | Token::BraceL(..) => {
                    let arg = self.parse_type_atom()?;
                    span_end = arg.span().end;
                    args.push(arg);
                }
                _ => break,
            }
        }

        Ok((
            TypeVariant { name, args },
            Span::from_begin_end(name_span.begin, span_end),
        ))
    }

    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParserErr> {
        self.parse_type_fun()
    }

    fn parse_type_fun(&mut self) -> Result<TypeExpr, ParserErr> {
        let lhs = self.parse_type_app()?;
        match self.current_token() {
            Token::ArrowR(..) => {
                self.next_token();
                let rhs = self.parse_type_fun()?;
                let span = Span::from_begin_end(lhs.span().begin, rhs.span().end);
                Ok(TypeExpr::Fun(span, Box::new(lhs), Box::new(rhs)))
            }
            _ => Ok(lhs),
        }
    }

    fn parse_type_app(&mut self) -> Result<TypeExpr, ParserErr> {
        let mut lhs = self.parse_type_atom()?;
        loop {
            match self.current_token() {
                Token::Ident(..) | Token::ParenL(..) | Token::BraceL(..) => {
                    let rhs = self.parse_type_atom()?;
                    let span = Span::from_begin_end(lhs.span().begin, rhs.span().end);
                    lhs = TypeExpr::App(span, Box::new(lhs), Box::new(rhs));
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_type_atom(&mut self) -> Result<TypeExpr, ParserErr> {
        match self.current_token() {
            Token::Ident(name, span, ..) => {
                let name = intern(&name);
                let span = span;
                self.next_token();
                Ok(TypeExpr::Name(span, name))
            }
            Token::ParenL(..) => self.parse_type_paren(),
            Token::BraceL(..) => self.parse_type_record(),
            token => Err(ParserErr::new(
                *token.span(),
                format!("unexpected {} in type", token),
            )),
        }
    }

    fn parse_type_paren(&mut self) -> Result<TypeExpr, ParserErr> {
        let span_begin = match self.current_token() {
            Token::ParenL(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `(` got {}", token),
                ));
            }
        };

        let first = self.parse_type_expr()?;
        let mut elems = Vec::new();
        let span_end = match self.current_token() {
            Token::Comma(..) => {
                self.next_token();
                elems.push(first);
                loop {
                    elems.push(self.parse_type_expr()?);
                    match self.current_token() {
                        Token::Comma(..) => {
                            self.next_token();
                            continue;
                        }
                        Token::ParenR(span, ..) => {
                            self.next_token();
                            break span.end;
                        }
                        token => {
                            return Err(ParserErr::new(
                                *token.span(),
                                format!("expected `)` got {}", token),
                            ));
                        }
                    }
                }
            }
            Token::ParenR(_span, ..) => {
                self.next_token();
                return Ok(first);
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `)` or `,` got {}", token),
                ));
            }
        };

        Ok(TypeExpr::Tuple(
            Span::from_begin_end(span_begin, span_end),
            elems,
        ))
    }

    fn parse_type_record(&mut self) -> Result<TypeExpr, ParserErr> {
        let span_begin = match self.current_token() {
            Token::BraceL(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `{{` got {}", token),
                ));
            }
        };

        let mut fields = Vec::new();
        if let Token::BraceR(span, ..) = self.current_token() {
            self.next_token();
            return Ok(TypeExpr::Record(
                Span::from_begin_end(span_begin, span.end),
                fields,
            ));
        }

        let span_end = loop {
            let (name, _span) = match self.current_token() {
                Token::Ident(name, span, ..) => {
                    let name = intern(&name);
                    let span = span;
                    self.next_token();
                    (name, span)
                }
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected field name got {}", token),
                    ));
                }
            };

            match self.current_token() {
                Token::Colon(..) => self.next_token(),
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `:` got {}", token),
                    ));
                }
            }

            let ty = self.parse_type_expr()?;
            fields.push((name, ty));

            match self.current_token() {
                Token::Comma(..) => {
                    self.next_token();
                }
                Token::BraceR(span, ..) => {
                    self.next_token();
                    break span.end;
                }
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `}}` got {}", token),
                    ));
                }
            }
        };

        Ok(TypeExpr::Record(
            Span::from_begin_end(span_begin, span_end),
            fields,
        ))
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParserErr> {
        self.parse_pattern_cons()
    }

    fn parse_pattern_cons(&mut self) -> Result<Pattern, ParserErr> {
        let mut lhs = self.parse_pattern_app()?;
        loop {
            match self.current_token() {
                Token::Colon(span_colon, ..) => {
                    self.next_token();
                    let rhs = self.parse_pattern_cons()?;
                    let span = Span::from_begin_end(lhs.span().begin, rhs.span().end);
                    lhs = Pattern::Cons(span, Box::new(lhs), Box::new(rhs));
                    let _ = span_colon;
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_pattern_app(&mut self) -> Result<Pattern, ParserErr> {
        let head = self.parse_pattern_atom()?;
        let mut args = Vec::new();
        loop {
            match self.current_token() {
                Token::Ident(..) | Token::BracketL(..) | Token::BraceL(..) | Token::ParenL(..) => {
                    let arg = self.parse_pattern_atom()?;
                    args.push(arg);
                }
                _ => break,
            }
        }

        if args.is_empty() {
            if let Pattern::Var(var) = &head {
                let is_constructor = var
                    .name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false);
                if is_constructor {
                    return Ok(Pattern::Named(var.span, var.name.clone(), vec![]));
                }
            }
            return Ok(head);
        }

        // If there are args, the head must be a constructor (identifier) pattern
        if let Pattern::Var(var) = head {
            let begin = var.span.begin;
            let end = args.last().unwrap().span().end;
            Ok(Pattern::Named(
                Span::from_begin_end(begin, end),
                var.name,
                args,
            ))
        } else {
            Err(ParserErr::new(
                *args.first().unwrap().span(),
                "constructor patterns must start with an identifier",
            ))
        }
    }

    fn parse_pattern_atom(&mut self) -> Result<Pattern, ParserErr> {
        match self.current_token() {
            Token::Ident(name, span, ..) if name == "_" => {
                self.next_token();
                Ok(Pattern::Wildcard(span))
            }
            Token::Ident(name, span, ..) => {
                self.next_token();
                Ok(Pattern::Var(Var::with_span(span, name)))
            }
            Token::BracketL(..) => self.parse_list_pattern(),
            Token::BraceL(..) => self.parse_dict_pattern(),
            Token::ParenL(..) => self.parse_paren_pattern(),
            token => Err(ParserErr::new(
                *token.span(),
                format!("unexpected {} in pattern", token),
            )),
        }
    }

    fn parse_list_pattern(&mut self) -> Result<Pattern, ParserErr> {
        let span_begin = match self.current_token() {
            Token::BracketL(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `[` got {}", token),
                ));
            }
        };

        if let Token::BracketR(span, ..) = self.current_token() {
            self.next_token();
            return Ok(Pattern::List(
                Span::from_begin_end(span_begin, span.end),
                Vec::new(),
            ));
        }

        let mut patterns = Vec::new();
        let span_end = loop {
            patterns.push(self.parse_pattern()?);

            match self.current_token() {
                Token::Comma(..) => {
                    self.next_token();
                }
                Token::BracketR(span, ..) => {
                    self.next_token();
                    break span.end;
                }
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `,` or `]` got {}", token),
                    ));
                }
            }
        };

        Ok(Pattern::List(
            Span::from_begin_end(span_begin, span_end),
            patterns,
        ))
    }

    fn parse_dict_pattern(&mut self) -> Result<Pattern, ParserErr> {
        let span_begin = match self.current_token() {
            Token::BraceL(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `{{` got {}", token),
                ));
            }
        };

        if let Token::BraceR(span, ..) = self.current_token() {
            self.next_token();
            return Ok(Pattern::Dict(
                Span::from_begin_end(span_begin, span.end),
                Vec::new(),
            ));
        }

        let mut keys = Vec::new();
        let span_end = loop {
            match self.current_token() {
                Token::Ident(name, _key_span, ..) => {
                    keys.push(intern(&name));
                    self.next_token();
                    match self.current_token() {
                        Token::Comma(..) => {
                            self.next_token();
                        }
                        Token::BraceR(span, ..) => {
                            self.next_token();
                            break span.end;
                        }
                        token => {
                            return Err(ParserErr::new(
                                *token.span(),
                                format!("expected `,` or `}}` got {}", token),
                            ));
                        }
                    }
                }
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected identifier in dict pattern got {}", token),
                    ));
                }
            }
        };

        Ok(Pattern::Dict(
            Span::from_begin_end(span_begin, span_end),
            keys,
        ))
    }

    fn parse_paren_pattern(&mut self) -> Result<Pattern, ParserErr> {
        let span_begin = match self.current_token() {
            Token::ParenL(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `(` got {}", token),
                ));
            }
        };

        let pat = self.parse_pattern()?;

        let span_end = match self.current_token() {
            Token::ParenR(span, ..) => {
                self.next_token();
                span.end
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `)` got {}", token),
                ));
            }
        };

        Ok(pat.with_span(Span::from_begin_end(span_begin, span_end)))
    }

    //
    fn parse_literal_bool_expr(&mut self) -> Result<Expr, ParserErr> {
        let token = self.current_token();
        self.next_token();
        match token {
            Token::Bool(val, span, ..) => Ok(Expr::Bool(span, val)),
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected `bool` got {}", token),
            )),
        }
    }

    //
    fn parse_literal_float_expr(&mut self) -> Result<Expr, ParserErr> {
        let token = self.current_token();
        self.next_token();
        match token {
            Token::Float(val, span, ..) => Ok(Expr::Float(span, val)),
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected `float` got {}", token),
            )),
        }
    }

    //
    fn parse_literal_int_expr(&mut self) -> Result<Expr, ParserErr> {
        let token = self.current_token();
        self.next_token();
        match token {
            Token::Int(val, span, ..) => Ok(Expr::Uint(span, val)),
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected `int` got {}", token),
            )),
        }
    }

    //
    fn parse_literal_str_expr(&mut self) -> Result<Expr, ParserErr> {
        let token = self.current_token();
        self.next_token();
        match token {
            Token::String(val, span, ..) => Ok(Expr::String(span, val)),
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected `str` got {}", token),
            )),
        }
    }

    //
    fn parse_ident_expr(&mut self) -> Result<Expr, ParserErr> {
        let token = self.current_token();
        self.next_token();
        match token {
            Token::Ident(name, span, ..) => Ok(Expr::Var(Var::with_span(span, name))),
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected `ident` got {}", token),
            )),
        }
    }
}

fn find_layout_expr_end(
    tokens: &[Token],
    start_idx: usize,
    block_indent: Option<usize>,
    is_stmt_head: impl Fn(&Token, &Token) -> bool,
) -> usize {
    // Scan forward until we hit either:
    // - a newline at depth 0 followed by a next statement head at the block indent
    // - a newline at depth 0 followed by a token with indentation < block indent (block end)
    let mut depth = 0usize;
    let mut idx = start_idx;
    while idx < tokens.len() {
        match &tokens[idx] {
            Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => depth += 1,
            Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                depth = depth.saturating_sub(1)
            }
            Token::WhitespaceNewline(nl_span, ..) if depth == 0 => {
                let mut j = idx + 1;
                while j < tokens.len() && matches!(tokens[j], Token::WhitespaceNewline(..)) {
                    j += 1;
                }
                if j >= tokens.len() {
                    return idx;
                }
                if let Some(indent) = block_indent {
                    let next_span = tokens[j].span();
                    if next_span.begin.column < indent {
                        return idx;
                    }
                    if next_span.begin.column == indent {
                        // Head check needs the following token too.
                        let mut k = j + 1;
                        while k < tokens.len() && matches!(tokens[k], Token::WhitespaceNewline(..))
                        {
                            k += 1;
                        }
                        if k < tokens.len() && is_stmt_head(&tokens[j], &tokens[k]) {
                            return idx;
                        }
                    }
                } else {
                    let _ = nl_span;
                }
            }
            _ => {}
        }
        idx += 1;
    }
    tokens.len()
}

fn find_layout_header_clause_end(
    tokens: &[Token],
    start_idx: usize,
    is_terminator: impl Fn(&Token) -> bool,
) -> usize {
    // Scan forward until we hit:
    // - a newline at depth 0 (end of the header line), OR
    // - a terminator token at depth 0 (e.g. `where` or `<=`).
    //
    // This is intentionally small and explicit: the parser mostly ignores
    // newlines, but `class`/`instance` headers need to stop on the newline so
    // optional-`where` layout blocks don't accidentally get pulled into the
    // header.
    let mut depth = 0usize;
    let mut idx = start_idx;
    while idx < tokens.len() {
        match &tokens[idx] {
            Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => depth += 1,
            Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                depth = depth.saturating_sub(1)
            }
            Token::WhitespaceNewline(..) if depth == 0 => return idx,
            token if depth == 0 && is_terminator(token) => return idx,
            _ => {}
        }
        idx += 1;
    }
    tokens.len()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::error::ParserErr;
    use rex_ast::{
        app, assert_expr_eq, b, d,
        expr::{Decl, Expr, Pattern, Scope, TypeExpr, Var},
        f, l, s, tup, u, v,
    };
    use rex_lexer::{span, span::Span, Token};

    use super::*;

    fn parse(code: &str) -> Arc<Expr> {
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        parser.parse_program().unwrap().expr
    }

    fn lam(param: &str, body: Arc<Expr>) -> Arc<Expr> {
        Arc::new(Expr::Lam(
            Span::default(),
            Scope::new_sync(),
            Var::new(param),
            None,
            Vec::new(),
            body,
        ))
    }

    #[test]
    fn test_parse_comment() {
        let mut parser = Parser::new(Token::tokenize("true {- this is a boolean -}").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(expr, b!(span!(1:1 - 1:5); true));

        let mut parser = Parser::new(Token::tokenize("{- this is a boolean -} false").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(expr, b!(span!(1:25 - 1:30); false));

        let mut parser = Parser::new(Token::tokenize("(3.54 {- this is a float -}, {- this is an int -} 42, false {- this is a boolean -})").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(
            expr,
            tup!(
                span!(1:1 - 1:85);
                f!(span!(1:2 - 1:6); 3.54),
                u!(span!(1:51 - 1:53); 42),
                b!(span!(1:55 - 1:60); false),
            )
        );
    }

    #[test]
    fn test_add() {
        let mut parser = Parser::new(Token::tokenize("1 + 2").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:6);
                app!(
                    span!(1:1 - 1:4);
                    v!(span!(1:3 - 1:4); "+"),
                    u!(span!(1:1 - 1:2); 1)
                ),
                u!(span!(1:5 - 1:6); 2)
            )
        );

        let mut parser = Parser::new(Token::tokenize("(6.9 + 3.14)").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:13);
                app!(
                    span!(1:2 - 1:7);
                    v!(span!(1:6 - 1:7); "+"),
                    f!(span!(1:2 - 1:5); 6.9)
                ),
                f!(span!(1:8 - 1:12); 3.14)
            )
        );

        let mut parser = Parser::new(Token::tokenize("(+) 420").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:8);
                v!(span!(1:1 - 1:4); "+"),
                u!(span!(1:5 - 1:8); 420)
            )
        );
    }

    #[test]
    fn test_parse_type_decl() {
        let code = r#"
        type MyADT a b c = MyCtor1 | MyCtor2 a b | MyCtor3 { field1: c }
        42
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program().unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::Type(decl) => {
                assert_eq!(decl.name, intern("MyADT"));
                assert_eq!(decl.params, vec![intern("a"), intern("b"), intern("c")]);
                assert_eq!(decl.variants.len(), 3);
                assert_eq!(decl.variants[0].name, intern("MyCtor1"));
                assert!(decl.variants[0].args.is_empty());
                assert_eq!(decl.variants[1].name, intern("MyCtor2"));
                assert_eq!(decl.variants[1].args.len(), 2);
                assert_eq!(decl.variants[2].name, intern("MyCtor3"));
                match &decl.variants[2].args[0] {
                    TypeExpr::Record(_, fields) => {
                        assert_eq!(fields.len(), 1);
                        assert_eq!(fields[0].0, intern("field1"));
                        assert!(matches!(
                            fields[0].1,
                            TypeExpr::Name(_, ref n) if n.as_ref() == "c"
                        ));
                    }
                    other => panic!("expected record type, got {other:?}"),
                }
            }
            other => panic!("expected type decl, got {other:?}"),
        }
        assert_expr_eq!(program.expr, u!(span!(3:9 - 3:11); 42));
    }

    #[test]
    fn test_parse_fn_decl_simple() {
        let code = r#"
        fn add x: i32 -> y: i32 -> i32 = x + y
        add 1 2
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program().unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::Fn(fd) => {
                assert_eq!(fd.name.name, intern("add"));
                assert_eq!(fd.params.len(), 2);
                assert_eq!(fd.params[0].0.name, intern("x"));
                assert!(matches!(
                    fd.params[0].1,
                    TypeExpr::Name(_, ref n) if n.as_ref() == "i32"
                ));
                assert_eq!(fd.params[1].0.name, intern("y"));
                assert!(matches!(
                    fd.params[1].1,
                    TypeExpr::Name(_, ref n) if n.as_ref() == "i32"
                ));
                assert!(matches!(fd.ret, TypeExpr::Name(_, ref n) if n.as_ref() == "i32"));
                assert!(fd.constraints.is_empty());
            }
            other => panic!("expected fn decl, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_fn_decl_where_constraints() {
        let code = r#"
        fn my_fun x: a -> y: b -> c where Iterable (a, b) = x
        my_fun
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program().unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::Fn(fd) => {
                assert_eq!(fd.name.name, intern("my_fun"));
                assert_eq!(fd.params.len(), 2);
                assert!(matches!(
                    fd.constraints[0].class,
                    ref n if n.as_ref() == "Iterable"
                ));
                match &fd.constraints[0].typ {
                    TypeExpr::Tuple(_, elems) => {
                        assert_eq!(elems.len(), 2);
                        assert!(matches!(elems[0], TypeExpr::Name(_, ref n) if n.as_ref() == "a"));
                        assert!(matches!(elems[1], TypeExpr::Name(_, ref n) if n.as_ref() == "b"));
                    }
                    other => panic!("expected tuple constraint type, got {other:?}"),
                }
            }
            other => panic!("expected fn decl, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_fn_decl_param_fun_type_requires_parens() {
        let code = r#"
        fn apply x: (a -> c) -> y: a -> c = x y
        apply
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program().unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::Fn(fd) => {
                assert_eq!(fd.name.name, intern("apply"));
                assert_eq!(fd.params.len(), 2);
                assert_eq!(fd.params[0].0.name, intern("x"));
                assert!(matches!(
                    fd.params[0].1,
                    TypeExpr::Fun(_, _, _)
                ));
                assert_eq!(fd.params[1].0.name, intern("y"));
                assert!(matches!(
                    fd.params[1].1,
                    TypeExpr::Name(_, ref n) if n.as_ref() == "a"
                ));
                assert!(matches!(fd.ret, TypeExpr::Name(_, ref n) if n.as_ref() == "c"));
            }
            other => panic!("expected fn decl, got {other:?}"),
        }
    }

    #[test]
    fn test_sub() {
        let mut parser = Parser::new(Token::tokenize("1 - 2").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:6);
                app!(
                    span!(1:1 - 1:4);
                    v!(span!(1:3 - 1:4); "-"),
                    u!(span!(1:1 - 1:2); 1)
                ),
                u!(span!(1:5 - 1:6); 2)
            )
        );

        let mut parser = Parser::new(Token::tokenize("(6.9 - 3.14)").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:13);
                app!(
                    span!(1:2 - 1:7);
                    v!(span!(1:6 - 1:7); "-"),
                    f!(span!(1:2 - 1:5); 6.9)
                ),
                f!(span!(1:8 - 1:12); 3.14)
            )
        );

        let mut parser = Parser::new(Token::tokenize("(-) 4.20").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:9);
                v!(span!(1:1 - 1:4); "-"),
                f!(span!(1:5 - 1:9); 4.20)
            )
        );
    }

    #[test]
    fn test_negate() {
        let mut parser = Parser::new(Token::tokenize("-1").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:3);
                v!(span!(1:1 - 1:2); "negate"),
                u!(span!(1:2 - 1:3); 1)
            )
        );

        let mut parser = Parser::new(Token::tokenize("(-1)").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:5);
                v!(span!(1:2 - 1:3); "negate"),
                u!(span!(1:3 - 1:4); 1)
            )
        );

        let mut parser = Parser::new(Token::tokenize("(- 6.9)").unwrap());
        let expr = parser.parse_program().unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:8);
                v!(span!(1:2 - 1:3); "negate"),
                f!(span!(1:4 - 1:7); 6.9)
            )
        );
    }

    #[test]
    fn test_application_associativity() {
        let expr = parse("f x y z");
        let expected = app!(app!(app!(v!("f"), v!("x")), v!("y")), v!("z"));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_projection_expr() {
        let expr = parse("x.field");
        let expected = Arc::new(Expr::Project(Span::default(), v!("x"), intern("field")));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_projection_expr_colon() {
        let expr = parse("x:field");
        let expected = Arc::new(Expr::Project(Span::default(), v!("x"), intern("field")));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_operator_precedence() {
        let expr = parse("1 + 2 * 3 - 4");
        let expected = app!(
            app!(v!("+"), u!(1)),
            app!(app!(v!("-"), app!(app!(v!("*"), u!(2)), u!(3))), u!(4))
        );

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_collections_and_tuples() {
        let expr = parse("([1, 2], { foo = \"bar\", baz = false }, (true, 9))");
        let expected = tup!(
            l!(u!(1), u!(2)),
            d!(foo = s!("bar"), baz = b!(false)),
            tup!(b!(true), u!(9))
        );

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_record_update_expr() {
        let expr = parse("{ foo with { x = 1, y = 2 } }");
        match expr.as_ref() {
            Expr::RecordUpdate(_, base, updates) => {
                assert_expr_eq!(base.clone(), v!("foo"); ignore span);
                assert_expr_eq!(updates.get(&intern("x")).unwrap().clone(), u!(1); ignore span);
                assert_expr_eq!(updates.get(&intern("y")).unwrap().clone(), u!(2); ignore span);
            }
            other => panic!("expected record update, got {other:?}"),
        }
    }

    #[test]
    fn test_brace_expr_prefers_dict_literal() {
        let expr = parse("{ foo = 1 }");
        match expr.as_ref() {
            Expr::Dict(_, kvs) => {
                assert_eq!(kvs.len(), 1);
                assert_expr_eq!(kvs.get(&intern("foo")).unwrap().clone(), u!(1); ignore span);
            }
            other => panic!("expected dict literal, got {other:?}"),
        }
    }

    #[test]
    fn test_record_update_empty_updates() {
        let expr = parse("{ foo with { } }");
        match expr.as_ref() {
            Expr::RecordUpdate(_, base, updates) => {
                assert_expr_eq!(base.clone(), v!("foo"); ignore span);
                assert!(updates.is_empty());
            }
            other => panic!("expected record update, got {other:?}"),
        }
    }

    #[test]
    fn test_lambda_and_let_chain() {
        let expr = parse("let inc = \\x -> x + 1, dbl = \\x -> x * 2 in \\y -> inc (dbl y)");

        let inc = lam("x", app!(app!(v!("+"), v!("x")), u!(1)));
        let dbl = lam("x", app!(app!(v!("*"), v!("x")), u!(2)));
        let body = lam("y", app!(v!("inc"), app!(v!("dbl"), v!("y"))));

        let expected = Arc::new(Expr::Let(
            Span::default(),
            Var::new("inc"),
            None,
            inc,
            Arc::new(Expr::Let(Span::default(), Var::new("dbl"), None, dbl, body)),
        ));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_type_annotations() {
        let expr = parse("let x: u8 = foo in x");
        match expr.as_ref() {
            Expr::Let(_, var, Some(TypeExpr::Name(_, name)), _def, _body) => {
                assert_eq!(var.name.as_ref(), "x");
                assert_eq!(name.as_ref(), "u8");
            }
            other => panic!("expected typed let, got {other:?}"),
        }

        let expr = parse("foo bar is u8");
        match expr.as_ref() {
            Expr::Ann(_, inner, TypeExpr::Name(_, name)) => {
                assert_eq!(name.as_ref(), "u8");
                assert!(matches!(inner.as_ref(), Expr::App(..)));
            }
            other => panic!("expected type assertion, got {other:?}"),
        }

        let expr = parse("\\ (a : f32) -> a");
        match expr.as_ref() {
            Expr::Lam(_, _scope, param, Some(TypeExpr::Name(_, name)), constraints, body) => {
                assert_eq!(param.name.as_ref(), "a");
                assert_eq!(name.as_ref(), "f32");
                assert!(constraints.is_empty());
                assert!(matches!(body.as_ref(), Expr::Var(_)));
            }
            other => panic!("expected typed lambda, got {other:?}"),
        }

        let expr = parse("let t: f32 -> str -> Result bool str = x in t");
        match expr.as_ref() {
            Expr::Let(_, _var, Some(ann), _def, _body) => {
                fn is_name(expr: &TypeExpr, expected: &str) -> bool {
                    matches!(expr, TypeExpr::Name(_, name) if name.as_ref() == expected)
                }

                match ann {
                    TypeExpr::Fun(_, arg, ret) => {
                        assert!(is_name(arg, "f32"));
                        match ret.as_ref() {
                            TypeExpr::Fun(_, arg2, ret2) => {
                                assert!(is_name(arg2, "str"));
                                match ret2.as_ref() {
                                    TypeExpr::App(_, fun, arg3) => {
                                        match fun.as_ref() {
                                            TypeExpr::App(_, fun2, arg2) => {
                                                assert!(is_name(fun2, "Result"));
                                                assert!(is_name(arg2, "bool"));
                                            }
                                            _ => panic!("expected Result bool str"),
                                        }
                                        assert!(is_name(arg3, "str"));
                                    }
                                    _ => panic!("expected Result bool str"),
                                }
                            }
                            _ => panic!("expected f32 -> str -> Result bool str"),
                        }
                    }
                    _ => panic!("expected function type annotation"),
                }
            }
            other => panic!("expected typed let, got {other:?}"),
        }
    }

    #[test]
    fn test_match_named_patterns() {
        let expr = parse("match named when Ok x -> x when Err e -> e when _ -> default");
        let expected = Arc::new(Expr::Match(
            Span::default(),
            v!("named"),
            vec![
                (
                    Pattern::Named(
                        Span::default(),
                        "Ok".into(),
                        vec![Pattern::Var(Var::new("x"))],
                    ),
                    v!("x"),
                ),
                (
                    Pattern::Named(
                        Span::default(),
                        "Err".into(),
                        vec![Pattern::Var(Var::new("e"))],
                    ),
                    v!("e"),
                ),
                (Pattern::Wildcard(Span::default()), v!("default")),
            ],
        ));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_match_list_patterns() {
        let expr = parse(
            "match list when [] -> empty when [x] -> x when [x, y, z] -> z when x:xs -> xs when _ -> fallback",
        );
        let expected = Arc::new(Expr::Match(
            Span::default(),
            v!("list"),
            vec![
                (Pattern::List(Span::default(), vec![]), v!("empty")),
                (
                    Pattern::List(Span::default(), vec![Pattern::Var(Var::new("x"))]),
                    v!("x"),
                ),
                (
                    Pattern::List(
                        Span::default(),
                        vec![
                            Pattern::Var(Var::new("x")),
                            Pattern::Var(Var::new("y")),
                            Pattern::Var(Var::new("z")),
                        ],
                    ),
                    v!("z"),
                ),
                (
                    Pattern::Cons(
                        Span::default(),
                        Box::new(Pattern::Var(Var::new("x"))),
                        Box::new(Pattern::Var(Var::new("xs"))),
                    ),
                    v!("xs"),
                ),
                (Pattern::Wildcard(Span::default()), v!("fallback")),
            ],
        ));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_match_nested_patterns() {
        let expr = parse("match t when Cons x (Cons _ xs) -> xs when Pair (Just a) (Just b) -> a");
        let expected = Arc::new(Expr::Match(
            Span::default(),
            v!("t"),
            vec![
                (
                    Pattern::Named(
                        Span::default(),
                        "Cons".into(),
                        vec![
                            Pattern::Var(Var::new("x")),
                            Pattern::Named(
                                Span::default(),
                                "Cons".into(),
                                vec![
                                    Pattern::Wildcard(Span::default()),
                                    Pattern::Var(Var::new("xs")),
                                ],
                            ),
                        ],
                    ),
                    v!("xs"),
                ),
                (
                    Pattern::Named(
                        Span::default(),
                        "Pair".into(),
                        vec![
                            Pattern::Named(
                                Span::default(),
                                "Just".into(),
                                vec![Pattern::Var(Var::new("a"))],
                            ),
                            Pattern::Named(
                                Span::default(),
                                "Just".into(),
                                vec![Pattern::Var(Var::new("b"))],
                            ),
                        ],
                    ),
                    v!("a"),
                ),
            ],
        ));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_match_dict_pattern() {
        let expr = parse("match obj when {foo, bar} -> foo bar");
        let expected = Arc::new(Expr::Match(
            Span::default(),
            v!("obj"),
            vec![(
                Pattern::Dict(Span::default(), vec!["foo".into(), "bar".into()]),
                app!(v!("foo"), v!("bar")),
            )],
        ));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_match_cons_associativity() {
        let expr = parse("match xs when h:t:u -> u");
        let expected = Arc::new(Expr::Match(
            Span::default(),
            v!("xs"),
            vec![(
                Pattern::Cons(
                    Span::default(),
                    Box::new(Pattern::Var(Var::new("h"))),
                    Box::new(Pattern::Cons(
                        Span::default(),
                        Box::new(Pattern::Var(Var::new("t"))),
                        Box::new(Pattern::Var(Var::new("u"))),
                    )),
                ),
                v!("u"),
            )],
        ));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_match_wildcard_cons() {
        let expr = parse("match xs when (_:_) -> xs");
        let expected = Arc::new(Expr::Match(
            Span::default(),
            v!("xs"),
            vec![(
                Pattern::Cons(
                    Span::default(),
                    Box::new(Pattern::Wildcard(Span::default())),
                    Box::new(Pattern::Wildcard(Span::default())),
                ),
                v!("xs"),
            )],
        ));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_match_empty_dict_pattern() {
        let expr = parse("match obj when {} -> obj");
        let expected = Arc::new(Expr::Match(
            Span::default(),
            v!("obj"),
            vec![(Pattern::Dict(Span::default(), vec![]), v!("obj"))],
        ));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_errors() {
        let mut parser = Parser::new(Token::tokenize("1 + 2 + in + 3").unwrap());
        let res = parser.parse_program();
        assert_eq!(
            res,
            Err(vec![ParserErr::new(
                Span::new(1, 9, 1, 11),
                "unexpected in"
            )])
        );

        let mut parser = Parser::new(Token::tokenize("1 + 2 in + 3").unwrap());
        let res = parser.parse_program();
        assert_eq!(
            res,
            Err(vec![ParserErr::new(Span::new(1, 7, 1, 9), "unexpected in")])
        );

        let mut parser = Parser::new(Token::tokenize("get 0 [    ").unwrap());
        let res = parser.parse_program();
        assert_eq!(
            res,
            Err(vec![ParserErr::new(
                Span::new(1, 12, 1, 12),
                "unexpected EOF"
            )])
        );

        let mut parser = Parser::new(Token::tokenize("elem0 (  ").unwrap());
        let res = parser.parse_program();
        assert_eq!(
            res,
            Err(vec![ParserErr::new(
                Span::new(1, 10, 1, 10),
                "unexpected EOF"
            )])
        );

        let mut parser = Parser::new(
            Token::tokenize(
                "
            { a = 1, b }
            { a = 1, b = 2, c }
            { a = 1, b = 3, c = 3, d }
            ",
            )
            .unwrap(),
        );
        let res = parser.parse_program();
        assert_eq!(
            res,
            Err(vec![
                ParserErr::new(Span::new(2, 24, 2, 25), "expected `=`"),
                ParserErr::new(Span::new(3, 31, 3, 32), "expected `=`"),
                ParserErr::new(Span::new(4, 38, 4, 39), "expected `=`")
            ])
        );
    }

    #[test]
    fn test_typeclass_where_is_optional() {
        let code = r#"
class Default a
    default : a

instance Default i32
    default = 0

default
"#;

        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program().unwrap();
        assert_eq!(program.decls.len(), 2);

        match &program.decls[0] {
            Decl::Class(decl) => {
                assert_eq!(decl.name, intern("Default"));
                assert_eq!(decl.methods.len(), 1);
                assert_eq!(decl.methods[0].name, intern("default"));
            }
            other => panic!("expected class decl, got {other:?}"),
        }

        match &program.decls[1] {
            Decl::Instance(decl) => {
                assert_eq!(decl.class, intern("Default"));
                assert_eq!(decl.methods.len(), 1);
                assert_eq!(decl.methods[0].name, intern("default"));
            }
            other => panic!("expected instance decl, got {other:?}"),
        }
    }

    #[test]
    fn test_typeclass_where_optional_does_not_force_method_block() {
        // Without `where`, an indented expression after a class/instance header
        // is not treated as a method block unless it looks like `name :` / `name =`.
        let code = r#"
class Marker a

instance Marker i32

    true
"#;

        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program().unwrap();
        assert_eq!(program.decls.len(), 2);
        assert!(matches!(program.expr.as_ref(), Expr::Bool(..)));
    }
}
