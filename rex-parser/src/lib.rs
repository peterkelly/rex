use std::{collections::VecDeque, sync::Arc, vec};

use rex_ast::expr::{
    Decl, Expr, Pattern, Program, Scope, TypeDecl, TypeExpr, TypeVariant, Var,
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
    pub fn new(tokens: Tokens) -> Parser {
        let mut parser = Parser {
            token_cursor: 0,
            tokens: tokens
                .items
                .into_iter()
                .filter_map(|token| match token {
                    Token::Whitespace(..) | Token::WhitespaceNewline(..) => None,
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

    fn current_token(&self) -> Token {
        if self.token_cursor < self.tokens.len() {
            self.tokens[self.token_cursor].clone()
        } else {
            Token::Eof(self.eof)
        }
    }

    fn peek_token(&self, n: usize) -> Token {
        if self.token_cursor + n < self.tokens.len() {
            self.tokens[self.token_cursor + n].clone()
        } else {
            Token::Eof(self.eof)
        }
    }

    fn next_token(&mut self) {
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
        while matches!(self.current_token(), Token::Type(..)) {
            match self.parse_type_decl() {
                Ok(decl) => decls.push(Decl::Type(decl)),
                Err(e) => {
                    self.record_error(e);
                    break;
                }
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
            Token::Dot(..) => Operator::Dot,
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
            token => Err(ParserErr::new(*token.span(), format!("unexpected {}", token))),
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
            Token::Dot(span, ..) => {
                self.next_token();
                Expr::Var(Var::with_span(span, "."))
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
                    format!("expected `[` got {}", token),
                ));
            }
        };

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
                    self.record_error(ParserErr::new(span, "expected `,` or `}}`"));
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
        while let Token::Ident(param, span, ..) = self.current_token() {
            self.next_token();
            params.push_back((span, param));
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
        while let Some((param_span, param)) = params.pop_back() {
            body = Expr::Lam(
                Span::from_begin_end(param_span.begin, body_span_end),
                Scope::new_sync(),
                Var::with_span(param_span, param),
                Arc::new(body),
            );
            body_span_end = body.span().end;
        }
        // Adjust the outer most lambda to include the initial backslash
        let body = body.with_span_begin(span_begin);

        Ok(body)
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
            decls.push_back((var, self.parse_expr()?));
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
        while let Some(((var_span, var), def)) = decls.pop_back() {
            body = Expr::Let(
                Span::from_begin_end(var_span.begin, body_span_end),
                Var::with_span(var_span, var),
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
                let name = name.clone();
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
                Token::Ident(param, ..) => param.clone(),
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
                let name = name.clone();
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
                let name = name.clone();
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
                    let name = name.clone();
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
                    keys.push(name);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::error::ParserErr;
    use rex_ast::{
        app, assert_expr_eq, b, d, f, l, s, tup, u, v,
        expr::{Decl, Pattern, Scope, TypeExpr, Var},
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
                assert_eq!(decl.name, "MyADT");
                assert_eq!(decl.params, vec!["a", "b", "c"]);
                assert_eq!(decl.variants.len(), 3);
                assert_eq!(decl.variants[0].name, "MyCtor1");
                assert!(decl.variants[0].args.is_empty());
                assert_eq!(decl.variants[1].name, "MyCtor2");
                assert_eq!(decl.variants[1].args.len(), 2);
                assert_eq!(decl.variants[2].name, "MyCtor3");
                match &decl.variants[2].args[0] {
                    TypeExpr::Record(_, fields) => {
                        assert_eq!(fields.len(), 1);
                        assert_eq!(fields[0].0, "field1");
                        assert!(matches!(fields[0].1, TypeExpr::Name(_, ref n) if n == "c"));
                    }
                    other => panic!("expected record type, got {other:?}"),
                }
            }
        }
        assert_expr_eq!(program.expr, u!(span!(3:9 - 3:11); 42));
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
        let expected = app!(
            app!(app!(v!("f"), v!("x")), v!("y")),
            v!("z")
        );

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_operator_precedence() {
        let expr = parse("1 + 2 * 3 - 4");
        let expected = app!(
            app!(
                v!("+"),
                u!(1)
            ),
            app!(
                app!(v!("-"), app!(app!(v!("*"), u!(2)), u!(3))),
                u!(4)
            )
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
    fn test_lambda_and_let_chain() {
        let expr = parse("let inc = \\x -> x + 1, dbl = \\x -> x * 2 in \\y -> inc (dbl y)");

        let inc = lam("x", app!(app!(v!("+"), v!("x")), u!(1)));
        let dbl = lam("x", app!(app!(v!("*"), v!("x")), u!(2)));
        let body = lam("y", app!(v!("inc"), app!(v!("dbl"), v!("y"))));

        let expected = Arc::new(Expr::Let(
            Span::default(),
            Var::new("inc"),
            inc,
            Arc::new(Expr::Let(
                Span::default(),
                Var::new("dbl"),
                dbl,
                body,
            )),
        ));

        assert_expr_eq!(expr, expected; ignore span);
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
}
