//! Parser implementation for Rex.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    sync::Arc,
    vec,
};

use rex_ast::expr::{
    ClassDecl, ClassMethodSig, Decl, DeclareFnDecl, Expr, FnDecl, ImportClause, ImportDecl,
    ImportItem, ImportPath, InstanceDecl, InstanceMethodImpl, Pattern, Program, Scope, Symbol,
    TypeConstraint, TypeDecl, TypeExpr, TypeVariant, Var, intern,
};
use rex_lexer::{
    Token, Tokens,
    span::{Position, Span, Spanned},
};

use crate::{error::ParserErr, op::Operator};
use rex_util::GasMeter;

pub struct Parser {
    token_cursor: usize,
    tokens: Vec<Token>,
    eof: Span,
    errors: Vec<ParserErr>,
    limits: ParserLimits,
    nesting_depth: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ParserLimits {
    pub max_nesting: Option<usize>,
}

impl ParserLimits {
    pub fn unlimited() -> Self {
        Self { max_nesting: None }
    }

    pub fn safe_defaults() -> Self {
        Self {
            max_nesting: Some(512),
        }
    }
}

impl Default for ParserLimits {
    fn default() -> Self {
        Self::unlimited()
    }
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
                self.next_token();
                Ok((intern(&name), span))
            }
            token => {
                if let Some((name, span)) = Self::operator_token_name(&token) {
                    self.next_token();
                    Ok((intern(name), span))
                } else {
                    Err(ParserErr::new(
                        *token.span(),
                        format!("expected identifier or operator name got {}", token),
                    ))
                }
            }
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
            limits: ParserLimits::default(),
            nesting_depth: 0,
        };
        // println!("tokens = {:#?}", parser.tokens);
        parser.strip_comments();
        parser
    }

    pub fn set_limits(&mut self, limits: ParserLimits) {
        self.limits = limits;
    }

    fn with_nesting<T>(
        &mut self,
        span: Span,
        f: impl FnOnce(&mut Self) -> Result<T, ParserErr>,
    ) -> Result<T, ParserErr> {
        if let Some(max) = self.limits.max_nesting
            && self.nesting_depth >= max
        {
            return Err(ParserErr::new(
                span,
                format!("maximum nesting depth exceeded (max {max})"),
            ));
        }
        self.nesting_depth += 1;
        let res = f(self);
        self.nesting_depth = self.nesting_depth.saturating_sub(1);
        res
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

    fn expect_colon(&mut self) -> Result<(), ParserErr> {
        let token = self.current_token();
        match token {
            Token::Colon(..) => {
                self.next_token();
                Ok(())
            }
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected `:` got {}", token),
            )),
        }
    }

    fn expect_assign(&mut self) -> Result<(), ParserErr> {
        let token = self.current_token();
        match token {
            Token::Assign(..) => {
                self.next_token();
                Ok(())
            }
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected `=` got {}", token),
            )),
        }
    }

    fn expect_paren_r(&mut self) -> Result<Span, ParserErr> {
        let token = self.current_token();
        match token {
            Token::ParenR(span, ..) => {
                self.next_token();
                Ok(span)
            }
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected `)` got {}", token),
            )),
        }
    }

    fn expect_ident(&mut self, what: &str) -> Result<(String, Span), ParserErr> {
        match self.current_token() {
            Token::Ident(name, span, ..) => {
                self.next_token();
                Ok((name, span))
            }
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected {what} got {}", token),
            )),
        }
    }

    fn is_value_name_token(token: &Token) -> bool {
        matches!(token, Token::Ident(..)) || Self::operator_token_name(token).is_some()
    }

    fn parse_layout_block<T>(
        &mut self,
        has_block: bool,
        block_indent: Option<usize>,
        mut parse_item: impl FnMut(&mut Self) -> Result<T, ParserErr>,
    ) -> Result<Vec<T>, ParserErr> {
        if !has_block {
            return Ok(Vec::new());
        }

        let mut items = Vec::new();
        loop {
            let token = self.current_token();
            if !Self::is_value_name_token(&token) {
                break;
            }
            let span = *token.span();
            if let Some(indent) = block_indent
                && span.begin.column != indent
            {
                break;
            }
            items.push(parse_item(self)?);
        }
        Ok(items)
    }

    fn find_token_at_depth0(
        &self,
        start: usize,
        end: usize,
        mut matches: impl FnMut(&Token) -> bool,
    ) -> Option<usize> {
        let mut depth = 0usize;
        let end = end.min(self.tokens.len());
        for i in start..end {
            match &self.tokens[i] {
                Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => depth += 1,
                Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                    depth = depth.saturating_sub(1)
                }
                _ => {}
            }
            if depth == 0 && matches(&self.tokens[i]) {
                return Some(i);
            }
        }
        None
    }

    fn find_same_line_end(&self, start_idx: usize) -> Result<usize, ParserErr> {
        let start_line = self
            .tokens
            .get(start_idx)
            .ok_or_else(|| ParserErr::new(self.eof, "unexpected EOF".to_string()))?
            .span()
            .begin
            .line;

        let mut depth = 0usize;
        let mut end_idx = start_idx;
        for i in start_idx..self.tokens.len() {
            let tok = &self.tokens[i];
            if depth == 0 && tok.span().begin.line > start_line {
                break;
            }
            match tok {
                Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => depth += 1,
                Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                    depth = depth.saturating_sub(1)
                }
                _ => {}
            }
            end_idx = i + 1;
        }
        Ok(end_idx)
    }

    fn find_dedent_end(&self, start_idx: usize, base_indent: usize) -> Result<usize, ParserErr> {
        let mut depth = 0usize;
        let mut end_idx = start_idx;
        for i in start_idx..self.tokens.len() {
            let tok = &self.tokens[i];
            match tok {
                Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => depth += 1,
                Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                    depth = depth.saturating_sub(1)
                }
                Token::WhitespaceNewline(..) if depth == 0 => {
                    let mut j = i + 1;
                    while j < self.tokens.len()
                        && matches!(self.tokens[j], Token::WhitespaceNewline(..))
                    {
                        j += 1;
                    }
                    if j >= self.tokens.len() {
                        return Ok(i);
                    }
                    if self.tokens[j].span().begin.column <= base_indent {
                        return Ok(i);
                    }
                }
                _ => {}
            }
            end_idx = i + 1;
        }
        Ok(end_idx)
    }

    fn paren_group_has_top_level_comma(&self, paren_start: usize) -> bool {
        let mut depth = 0usize;
        for i in (paren_start + 1)..self.tokens.len() {
            match self.tokens[i] {
                Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => depth += 1,
                Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                    if depth == 0 {
                        break;
                    }
                    depth = depth.saturating_sub(1)
                }
                Token::Comma(..) if depth == 0 => return true,
                _ => {}
            }
        }
        false
    }

    fn paren_group_has_top_level_colon(&self, paren_start: usize) -> bool {
        let mut depth = 0usize;
        for i in (paren_start + 1)..self.tokens.len() {
            match self.tokens[i] {
                Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => depth += 1,
                Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                    if depth == 0 {
                        break;
                    }
                    depth = depth.saturating_sub(1)
                }
                Token::Colon(..) if depth == 0 => return true,
                _ => {}
            }
        }
        false
    }

    fn parse_legacy_param_group(&mut self) -> Result<Vec<(Var, TypeExpr)>, ParserErr> {
        match self.current_token() {
            Token::ParenL(..) => self.next_token(),
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `(` got {}", token),
                ));
            }
        }

        let mut params = Vec::new();
        if matches!(self.current_token(), Token::ParenR(..)) {
            self.next_token();
            return Ok(params);
        }

        loop {
            let (param_name, param_span) = self.expect_ident("parameter name")?;
            self.expect_colon()?;

            let ty_start = self.token_cursor;
            let ty_end = self
                .find_token_at_depth0(ty_start, self.tokens.len(), |t| {
                    matches!(t, Token::Comma(..) | Token::ParenR(..))
                })
                .ok_or_else(|| {
                    ParserErr::new(
                        self.eof,
                        "expected `,` or `)` after parameter type".to_string(),
                    )
                })?;
            let ann = self.parse_type_expr_slice(&self.tokens[ty_start..ty_end])?;
            self.token_cursor = ty_end;
            params.push((Var::with_span(param_span, param_name), ann));

            match self.current_token() {
                Token::Comma(..) => {
                    self.next_token();
                    continue;
                }
                Token::ParenR(..) => {
                    self.next_token();
                    break;
                }
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `,` or `)` got {}", token),
                    ));
                }
            }
        }

        Ok(params)
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

    fn parse_program_core(&mut self) -> Result<Program, Vec<ParserErr>> {
        let mut decls = Vec::new();
        loop {
            let mut is_pub = false;
            if let Token::Pub(..) = self.current_token() {
                is_pub = true;
                self.next_token();
            }

            match self.current_token() {
                Token::Type(..) => match self.parse_type_decl(is_pub) {
                    Ok(decl) => decls.push(Decl::Type(decl)),
                    Err(e) => {
                        self.record_error(e);
                        break;
                    }
                },
                Token::Fn(..) => match self.parse_fn_decl(is_pub) {
                    Ok(decl) => decls.push(Decl::Fn(decl)),
                    Err(e) => {
                        self.record_error(e);
                        break;
                    }
                },
                Token::Declare(..) => match self.parse_declare_fn_decl_toplevel(is_pub) {
                    Ok(decl) => decls.push(Decl::DeclareFn(decl)),
                    Err(e) => {
                        self.record_error(e);
                        break;
                    }
                },
                Token::Import(..) => match self.parse_import_decl(is_pub) {
                    Ok(decl) => decls.push(Decl::Import(decl)),
                    Err(e) => {
                        self.record_error(e);
                        break;
                    }
                },
                Token::Class(..) => match self.parse_class_decl(is_pub) {
                    Ok(decl) => decls.push(Decl::Class(decl)),
                    Err(e) => {
                        self.record_error(e);
                        break;
                    }
                },
                Token::Instance(..) => match self.parse_instance_decl(is_pub) {
                    Ok(decl) => decls.push(Decl::Instance(decl)),
                    Err(e) => {
                        self.record_error(e);
                        break;
                    }
                },
                _ => break,
            }
        }

        let expr = if matches!(self.current_token(), Token::Eof(..)) {
            // The trailing expression is optional; a declarations-only file
            // evaluates to unit `()`.
            Expr::Tuple(self.eof, vec![])
        } else {
            match self.parse_expr() {
                Ok(expr) => expr,
                Err(e) => {
                    self.record_error(e);
                    return Err(self.errors.clone());
                }
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

    pub fn parse_program(&mut self, gas: &mut GasMeter) -> Result<Program, Vec<ParserErr>> {
        let token_cost = gas
            .costs
            .parse_token
            .saturating_mul(self.tokens.len() as u64);
        if let Err(e) = gas.charge(token_cost) {
            return Err(vec![ParserErr::new(Span::default(), e.to_string())]);
        }

        let program = self.parse_program_core()?;
        let expr_nodes = count_expr_nodes(program.expr.as_ref());
        let decl_nodes = program.decls.len() as u64;
        let node_cost = gas
            .costs
            .parse_node
            .saturating_mul(expr_nodes.saturating_add(decl_nodes));
        if let Err(e) = gas.charge(node_cost) {
            return Err(vec![ParserErr::new(Span::default(), e.to_string())]);
        }
        Ok(program)
    }

    fn parse_expr(&mut self) -> Result<Expr, ParserErr> {
        let span = *self.current_token().span();
        self.with_nesting(span, |this| {
            let lhs_expr = this.parse_unary_expr()?;
            this.parse_binary_expr(lhs_expr)
        })
    }

    fn parse_binary_expr(&mut self, lhs_expr: Expr) -> Result<Expr, ParserErr> {
        let span = *lhs_expr.span();
        self.with_nesting(span, move |this| {
            let lhs_expr_span = lhs_expr.span();

            // Get the next token.
            let token = match this.current_token() {
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
                Token::ColonColon(..) => {
                    let operator_span = token.span();
                    this.next_token();

                    let rhs_expr = this.parse_unary_expr()?;
                    let rhs_expr_span = *rhs_expr.span();

                    let next_binary_expr_takes_precedence = match this.current_token() {
                        Token::Eof(..) => false,
                        token if prec > token.precedence() => false,
                        token if prec == token.precedence() => !matches!(
                            token,
                            Token::Add(..)
                                | Token::And(..)
                                | Token::Concat(..)
                                | Token::Div(..)
                                | Token::Mul(..)
                                | Token::Mod(..)
                                | Token::Or(..)
                                | Token::Sub(..)
                        ),
                        _ => true,
                    };

                    let rhs_expr = if next_binary_expr_takes_precedence {
                        this.parse_binary_expr(rhs_expr)?
                    } else {
                        rhs_expr
                    };

                    let cons_span = Span::from_begin_end(lhs_expr_span.begin, operator_span.end);
                    let outer_span = Span::from_begin_end(lhs_expr_span.begin, rhs_expr_span.end);
                    return this.parse_binary_expr(Expr::App(
                        outer_span,
                        Arc::new(Expr::App(
                            cons_span,
                            Arc::new(Expr::Var(Var::with_span(*operator_span, "Cons"))),
                            Arc::new(lhs_expr),
                        )),
                        Arc::new(rhs_expr),
                    ));
                }
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
            this.next_token();

            // Parse the next part of this binary expression.
            let rhs_expr = this.parse_unary_expr()?;
            let rhs_expr_span = *rhs_expr.span();

            let next_binary_expr_takes_precedence = match this.current_token() {
                // No more tokens
                Token::Eof(..) => false,
                // Next token has lower precedence
                token if prec > token.precedence() => false,
                // Next token has the same precedence
                token if prec == token.precedence() => {
                    // Right-associative unless token is one of the left-associative ops.
                    !matches!(
                        token,
                        Token::Add(..)
                            | Token::And(..)
                            | Token::Concat(..)
                            | Token::Div(..)
                            | Token::Mul(..)
                            | Token::Mod(..)
                            | Token::Or(..)
                            | Token::Sub(..)
                    )
                }
                // Next token has higher precedence
                _ => true,
            };

            let rhs_expr = if next_binary_expr_takes_precedence {
                this.parse_binary_expr(rhs_expr)?
            } else {
                rhs_expr
            };

            let inner_span = Span::from_begin_end(lhs_expr_span.begin, operator_span.end);
            let outer_span = Span::from_begin_end(lhs_expr_span.begin, rhs_expr_span.end);

            this.parse_binary_expr(Expr::App(
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
        })
    }

    fn parse_unary_expr(&mut self) -> Result<Expr, ParserErr> {
        // println!("parse_unary_expr: self.current_token() = {:#?}", self.current_token());
        let mut call_base_expr = self.parse_postfix_expr()?;
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
                | Token::Question(..)
                | Token::Ident(..)
                | Token::BackSlash(..)
                | Token::Let(..)
                | Token::If(..)
                | Token::Match(..) => self.parse_postfix_expr(),
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
        while let Token::Is(..) = self.current_token() {
            self.next_token();
            let ann = self.parse_type_expr()?;
            let span = Span::from_begin_end(call_base_expr.span().begin, ann.span().end);
            call_base_expr = Expr::Ann(span, Arc::new(call_base_expr), ann);
        }
        Ok(call_base_expr)
    }

    fn parse_postfix_expr(&mut self) -> Result<Expr, ParserErr> {
        let mut base = self.parse_atom_expr()?;
        while let Token::Dot(..) = self.current_token() {
            self.next_token();

            let (field, end) = match self.current_token() {
                Token::Ident(name, span, ..) => {
                    let name = intern(&name);
                    let end = span.end;
                    self.next_token();
                    (name, end)
                }
                Token::Int(value, span) => {
                    let name = intern(&value.to_string());
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

            let span = Span::from_begin_end(base.span().begin, end);
            base = Expr::Project(span, Arc::new(base), field);
        }
        Ok(base)
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
            Token::Question(..) => self.parse_hole_expr(),
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

        self.with_nesting(span_begin, |this| {
            // Parse the inner expression.
            let expr = match this.current_token() {
                Token::ParenR(span, ..) => {
                    this.next_token();
                    // Empty tuple
                    return Ok(Expr::Tuple(
                        Span::from_begin_end(span_begin.begin, span.end),
                        vec![],
                    ));
                }
                Token::Add(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, "+"))
                }
                Token::And(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, "&&"))
                }
                Token::Concat(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, "++"))
                }
                Token::Div(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, "/"))
                }
                Token::Eq(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, "=="))
                }
                Token::Ge(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, ">="))
                }
                Token::Gt(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, ">"))
                }
                Token::Le(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, "<="))
                }
                Token::Lt(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, "<"))
                }
                Token::Mod(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, "%"))
                }
                Token::Mul(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, "*"))
                }
                Token::Or(span, ..) => {
                    this.next_token();
                    Expr::Var(Var::with_span(span, "||"))
                }
                Token::Sub(span, ..) => {
                    if let Token::ParenR(..) = this.peek_token(1) {
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
                        this.next_token();
                        Expr::Var(Var::with_span(span, "-"))
                    } else {
                        this.parse_expr()?
                    }
                }
                _ => this.parse_expr()?,
            };

            // Eat the right parenthesis.
            let span_end = match this.current_token() {
                Token::ParenR(span, ..) => {
                    this.next_token();
                    span
                }
                Token::Comma(..) => {
                    // parse inner expressions
                    return this.parse_tuple(span_begin, expr);
                }
                token => {
                    this.record_error(ParserErr::new(*token.span(), "expected `)`"));
                    return Ok(expr);
                }
            };

            let expr = expr.with_span_begin_end(span_begin.begin, span_end.end);
            Ok(expr)
        })
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
                span
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `[` got {}", token),
                ));
            }
        };

        self.with_nesting(span_begin, |this| {
            let mut exprs = Vec::new();
            loop {
                if let Token::BracketR(..) = this.current_token() {
                    break;
                }

                // Parse the next expression.
                exprs.push(Arc::new(this.parse_expr()?));
                // Eat the comma.
                match this.current_token() {
                    Token::Comma(..) => this.next_token(),
                    Token::Eof(span) => {
                        this.record_error(ParserErr::new(span, "expected `,` or `]`"));
                        break;
                    }
                    _ => {
                        break;
                    }
                };
            }

            // Eat the right bracket.
            let span_end = match this.current_token() {
                Token::BracketR(span, ..) => {
                    this.next_token();
                    span.end
                }
                token => {
                    this.record_error(ParserErr::new(
                        *token.span(),
                        format!("expected `]` got {}", token),
                    ));

                    return Ok(Expr::List(
                        Span::from_begin_end(span_begin.begin, token.span().end),
                        exprs,
                    ));
                }
            };

            Ok(Expr::List(
                Span::from_begin_end(span_begin.begin, span_end),
                exprs,
            ))
        })
    }

    fn parse_brace_expr(&mut self) -> Result<Expr, ParserErr> {
        // Eat the left brace.
        let span_begin = match self.current_token() {
            Token::BraceL(span, ..) => {
                self.next_token();
                span
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `{{` got {}", token),
                ));
            }
        };

        self.with_nesting(span_begin, |this| {
            let span_begin_pos = span_begin.begin;

            // `{}` is always an empty dict literal.
            if let Token::BraceR(span, ..) = this.current_token() {
                this.next_token();
                return Ok(Expr::Dict(
                    Span::from_begin_end(span_begin_pos, span.end),
                    BTreeMap::new(),
                ));
            }

            // Disambiguate:
            // - `{ x = e }` is a dict literal
            // - `{ base with { x = e } }` is a record-update expression
            let looks_like_dict_literal = matches!(this.current_token(), Token::Ident(..))
                && matches!(this.peek_token(1), Token::Assign(..));

            if looks_like_dict_literal {
                return this.parse_dict_expr_after_lbrace(span_begin_pos);
            }

            let base = match this.parse_expr() {
                Ok(expr) => expr,
                Err(err) => {
                    this.record_error(err);
                    while !matches!(this.current_token(), Token::BraceR(..) | Token::Eof(..)) {
                        this.next_token();
                    }
                    if let Token::BraceR(..) = this.current_token() {
                        this.next_token();
                    }
                    return Ok(Expr::Dict(
                        Span::from_begin_end(span_begin_pos, Position::new(0, 0)),
                        BTreeMap::new(),
                    ));
                }
            };

            match this.current_token() {
                Token::With(..) => this.next_token(),
                token => {
                    this.record_error(ParserErr::new(*token.span(), "expected `with`"));
                    while !matches!(this.current_token(), Token::BraceR(..) | Token::Eof(..)) {
                        this.next_token();
                    }
                    if let Token::BraceR(..) = this.current_token() {
                        this.next_token();
                    }
                    return Ok(base);
                }
            };

            let updates = match this.parse_dict_expr() {
                Ok(Expr::Dict(_, kvs)) => kvs,
                Ok(other) => {
                    this.record_error(ParserErr::new(*other.span(), "expected `{...}`"));
                    BTreeMap::new()
                }
                Err(err) => {
                    this.record_error(err);
                    BTreeMap::new()
                }
            };

            // Eat the right brace of the update expression.
            let span_end = match this.current_token() {
                Token::BraceR(span, ..) => {
                    this.next_token();
                    span.end
                }
                token => {
                    this.record_error(ParserErr::new(
                        *token.span(),
                        format!("expected `}}` got {}", token),
                    ));
                    Position::new(0, 0)
                }
            };

            Ok(Expr::RecordUpdate(
                Span::from_begin_end(span_begin_pos, span_end),
                Arc::new(base),
                updates,
            ))
        })
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

        self.with_nesting(Span::from_begin_end(span_begin, span_begin), |this| {
            this.parse_dict_expr_after_lbrace(span_begin)
        })
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
        while let Token::Ident(..) | Token::ParenL(..) = self.current_token() {
            let (var, ann, span) = self.parse_lambda_param()?;
            params.push_back((span, var, ann));
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
                let (name, name_span) = self.expect_ident("parameter name")?;
                self.expect_colon()?;
                let ann = self.parse_type_expr()?;
                let span_end = self.expect_paren_r()?.end;
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

        let is_rec = if matches!(self.current_token(), Token::Rec(..)) {
            self.next_token();
            true
        } else {
            false
        };

        if is_rec {
            let mut bindings = Vec::new();
            loop {
                let (pat, ann) = match (self.current_token(), self.peek_token(1)) {
                    (Token::Ident(val, span, ..), Token::Colon(..)) => {
                        self.next_token();
                        self.next_token();
                        let var = Var::with_span(span, val);
                        let ann = Some(self.parse_type_expr()?);
                        (Pattern::Var(var), ann)
                    }
                    _ => {
                        let pat = self.parse_pattern()?;
                        let mut ann = None;
                        if let Token::Colon(..) = self.current_token() {
                            self.next_token();
                            ann = Some(self.parse_type_expr()?);
                        }
                        (pat, ann)
                    }
                };

                let var = match pat {
                    Pattern::Var(var) => var,
                    other => {
                        return Err(ParserErr::new(
                            *other.span(),
                            "let rec only supports variable bindings".to_string(),
                        ));
                    }
                };

                self.expect_assign()?;
                let def = Arc::new(self.parse_expr()?);
                bindings.push((var, ann, def));

                match self.current_token() {
                    Token::Comma(..) => {
                        self.next_token();
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

            match self.current_token() {
                Token::In(..) => self.next_token(),
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `in` got {}", token),
                    ));
                }
            };

            let body = Arc::new(self.parse_expr()?);
            let span = Span::from_begin_end(span_begin, body.span().end);
            Ok(Expr::LetRec(span, bindings, body))
        } else {
            // Parse the variable declarations.
            let mut decls = VecDeque::new();
            let is_pattern_start = |token: Token| {
                matches!(
                    token,
                    Token::Ident(..) | Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..)
                )
            };
            while is_pattern_start(self.current_token()) {
                let (pat, ann) = match (self.current_token(), self.peek_token(1)) {
                    (Token::Ident(val, span, ..), Token::Colon(..)) => {
                        self.next_token();
                        self.next_token();
                        let var = Var::with_span(span, val);
                        let ann = Some(self.parse_type_expr()?);
                        (Pattern::Var(var), ann)
                    }
                    _ => {
                        let pat = self.parse_pattern()?;
                        let mut ann = None;
                        if let Token::Colon(..) = self.current_token() {
                            self.next_token();
                            ann = Some(self.parse_type_expr()?);
                        }
                        (pat, ann)
                    }
                };

                // =
                self.expect_assign()?;
                // Parse the variable definition
                decls.push_back((pat, ann, self.parse_expr()?));
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
            while let Some((pat, ann, def)) = decls.pop_back() {
                match pat {
                    Pattern::Var(var) => {
                        body = Expr::Let(
                            Span::from_begin_end(var.span.begin, body_span_end),
                            var,
                            ann,
                            Arc::new(def),
                            Arc::new(body),
                        );
                    }
                    pat => {
                        let def_expr = match ann {
                            Some(ann) => {
                                let span = Span::from_begin_end(def.span().begin, ann.span().end);
                                Expr::Ann(span, Arc::new(def), ann)
                            }
                            None => def,
                        };
                        body = Expr::Match(
                            Span::from_begin_end(pat.span().begin, body_span_end),
                            Arc::new(def_expr),
                            vec![(pat, Arc::new(body))],
                        );
                    }
                }
                body_span_end = body.span().end;
            }
            // Adjust the outer most let-in expression to include the initial let
            // token
            let body = body.with_span_begin(span_begin);

            Ok(body)
        }
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

    fn eof_for_slice(&self, slice: &[Token]) -> Span {
        slice.last().map(|t| *t.span()).unwrap_or(self.eof)
    }

    fn parse_type_expr_slice(&self, slice: &[Token]) -> Result<TypeExpr, ParserErr> {
        if slice.is_empty() {
            return Err(ParserErr::new(self.eof, "expected type".to_string()));
        }
        let eof = self.eof_for_slice(slice);
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
        let eof = self.eof_for_slice(slice);
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

    fn parse_type_constraints_slice(
        &self,
        slice: &[Token],
    ) -> Result<Vec<TypeConstraint>, ParserErr> {
        if slice.is_empty() {
            return Err(ParserErr::new(
                self.eof,
                "expected type constraint".to_string(),
            ));
        }
        let eof = self.eof_for_slice(slice);
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
        let eof = self.eof_for_slice(slice);
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

    fn parse_class_decl(&mut self, is_pub: bool) -> Result<ClassDecl, ParserErr> {
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
        let implicit_method_start = if saw_where {
            false
        } else {
            let token = self.current_token();
            let token_span = *token.span();
            let next = self.peek_token(1);
            token_span.begin.column > class_indent
                && matches!(next, Token::Colon(..))
                && Self::is_value_name_token(&token)
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

        let methods = self.parse_layout_block(has_method_block, block_indent, |parser| {
            let (m_name, _name_span) = parser.parse_value_name()?;
            parser.expect_colon()?;

            let start_idx = parser.token_cursor;
            let end_idx =
                find_layout_expr_end(&parser.tokens, start_idx, block_indent, |t, next| {
                    matches!(next, Token::Colon(..)) && Self::is_value_name_token(t)
                });
            let typ = parser.parse_type_expr_slice(&parser.tokens[start_idx..end_idx])?;
            parser.token_cursor = end_idx;
            parser.skip_newlines();
            Ok(ClassMethodSig { name: m_name, typ })
        })?;

        let span_end = methods
            .last()
            .map(|m| m.typ.span().end)
            .or_else(|| supers.last().map(|s| s.typ.span().end))
            .unwrap_or(header_end.max(where_span.end));

        Ok(ClassDecl {
            span: Span::from_begin_end(span_begin, span_end),
            is_pub,
            name,
            params,
            supers,
            methods,
        })
    }

    fn parse_instance_decl(&mut self, is_pub: bool) -> Result<InstanceDecl, ParserErr> {
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
        if let Some(Token::Le(span, ..)) = self.tokens.get(self.token_cursor).cloned()
            && span.begin.line == header_line
        {
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
                && matches!(next, Token::Assign(..))
                && Self::is_value_name_token(&token)
        };

        let block_indent = if has_method_block {
            match self.current_token() {
                Token::Ident(_, span, ..) => Some(span.begin.column),
                token => Self::operator_token_name(&token).map(|(_, span)| span.begin.column),
            }
        } else {
            None
        };

        let methods = self.parse_layout_block(has_method_block, block_indent, |parser| {
            let (name, _name_span) = parser.parse_value_name()?;
            parser.expect_assign()?;

            let start_idx = parser.token_cursor;
            let end_idx =
                find_layout_expr_end(&parser.tokens, start_idx, block_indent, |t, next| {
                    matches!(next, Token::Assign(..)) && Self::is_value_name_token(t)
                });
            let body = parser.parse_expr_slice(&parser.tokens[start_idx..end_idx])?;
            parser.token_cursor = end_idx;
            parser.skip_newlines();

            Ok(InstanceMethodImpl {
                name,
                body: Arc::new(body),
            })
        })?;

        let span_end = methods
            .last()
            .map(|m| m.body.span().end)
            .or_else(|| context.last().map(|c| c.typ.span().end))
            .unwrap_or(header_end.max(where_span.end));

        Ok(InstanceDecl {
            span: Span::from_begin_end(span_begin, span_end),
            is_pub,
            class,
            head,
            context,
            methods,
        })
    }

    fn parse_fn_decl(&mut self, is_pub: bool) -> Result<FnDecl, ParserErr> {
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

        // Signature form:
        //   fn add : i32 -> i32 -> i32 = \x y -> x + y
        //
        // This desugars back into the existing `FnDecl { params, ret, body }` shape by:
        // - flattening the signature `a -> b -> c` into param types `[a, b]` and ret `c`
        // - extracting lambda binders from the body when present (so we keep user-chosen arg names)
        // - otherwise, eta-expanding the body to match the declared arity
        if matches!(self.current_token(), Token::Colon(..)) {
            self.next_token();

            // Parse a full type signature up to `where` or `=`.
            let sig_start = self.token_cursor;
            let sig_end = self
                .find_token_at_depth0(sig_start, self.tokens.len(), |t| {
                    matches!(t, Token::Where(..) | Token::Assign(..))
                })
                .ok_or_else(|| ParserErr::new(self.eof, "expected `=`".to_string()))?;
            let sig = self.parse_type_expr_slice(&self.tokens[sig_start..sig_end])?;
            self.token_cursor = sig_end;

            let mut constraints = Vec::new();
            if matches!(self.current_token(), Token::Where(..)) {
                self.next_token();
                constraints = self.parse_type_constraints()?;
            }

            // `=`
            self.expect_assign()?;

            // Parse a body expression until dedent back to the function's indentation.
            let body_start = self.token_cursor;
            let body_end = self.find_dedent_end(body_start, span_begin.column)?;
            let body_expr = self.parse_expr_slice(&self.tokens[body_start..body_end])?;
            self.token_cursor = body_end;
            let body = Arc::new(body_expr);
            let span_end = body.span().end;

            // Flatten `a -> b -> c` into params=[a,b], ret=c so downstream code can
            // reconstruct the same function type.
            let mut param_tys = Vec::new();
            let mut cur = sig;
            let ret = loop {
                match cur {
                    TypeExpr::Fun(_, arg, next_ret) => {
                        param_tys.push(*arg);
                        cur = *next_ret;
                    }
                    other => break other,
                }
            };
            if param_tys.is_empty() {
                return Err(ParserErr::new(
                    *ret.span(),
                    "expected function type after `:`; use `let` for values".to_string(),
                ));
            }

            let arity = param_tys.len();
            let mut body_constraints = Vec::new();
            let (params, body) = if matches!(body.as_ref(), Expr::Lam(..)) {
                let mut lam_params: Vec<Var> = Vec::new();
                let mut cur = body.clone();
                while matches!(cur.as_ref(), Expr::Lam(..)) {
                    let Expr::Lam(_span, _scope, param, _ann, lam_constraints, next) = cur.as_ref()
                    else {
                        break;
                    };
                    if !lam_constraints.is_empty() {
                        body_constraints.extend(lam_constraints.iter().cloned());
                    }
                    lam_params.push(param.clone());
                    cur = next.clone();
                }

                if lam_params.len() != arity {
                    return Err(ParserErr::new(
                        *body.span(),
                        format!(
                            "lambda has {} parameter(s) but signature expects {}",
                            lam_params.len(),
                            arity
                        ),
                    ));
                }

                let params: Vec<(Var, TypeExpr)> = lam_params.into_iter().zip(param_tys).collect();
                (params, cur)
            } else {
                // No leading lambda: eta-expand to match the declared arity.
                let var_span = *body.span();
                let vars: Vec<Var> = (0..arity)
                    .map(|i| Var::with_span(var_span, format!("_arg{i}")))
                    .collect();

                let mut applied = body.clone();
                for v in &vars {
                    applied = Arc::new(Expr::App(
                        Span::from_begin_end(applied.span().begin, applied.span().end),
                        applied,
                        Arc::new(Expr::Var(v.clone())),
                    ));
                }

                let params: Vec<(Var, TypeExpr)> = vars.into_iter().zip(param_tys).collect();
                (params, applied)
            };

            constraints.extend(body_constraints);

            return Ok(FnDecl {
                span: Span::from_begin_end(span_begin, span_end),
                is_pub,
                name: name_var,
                params,
                ret,
                constraints,
                body,
            });
        }

        let is_named_param_head = |token: &Token, next: &Token| {
            matches!(token, Token::Ident(..)) && matches!(next, Token::Colon(..))
        };
        let is_paren_param_head = |token: &Token, next: &Token, next2: &Token| {
            matches!(token, Token::ParenL(..))
                && matches!(next, Token::Ident(..))
                && matches!(next2, Token::Colon(..))
        };

        // Params (new syntax):
        //   fn foo x: a -> y: b -> i32 = ...
        //   fn foo (x: a) -> (y: b) -> i32 = ...
        //   fn foo (x: a) (y: b) -> i32 = ...
        //
        // Params (legacy syntax, still accepted):
        //   fn foo (x: a, y: b) -> i32 = ...
        // First, handle the legacy multi-parameter paren list `(x: a, y: b) -> ...`.
        if matches!(self.current_token(), Token::ParenL(..)) {
            // `()` is also treated as the legacy group syntax (nullary functions).
            if self.paren_group_has_top_level_comma(self.token_cursor)
                || matches!(self.peek_token(1), Token::ParenR(..))
            {
                params = self.parse_legacy_param_group()?;

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
        }

        // New syntax: a chain of named params (`x: a -> ...`) and/or parenthesized params (`(x: a) -> ...`).
        // Parameters are always delimited by `->`. Adjacency is not accepted.
        if params.is_empty() {
            loop {
                let tok = self.current_token();
                let next = self.peek_token(1);
                let next2 = self.peek_token(2);
                if is_paren_param_head(&tok, &next, &next2) {
                    // (x: a)
                    self.next_token(); // `(`
                    let (param_name, param_span) = self.expect_ident("parameter name")?;
                    self.expect_colon()?;

                    // Parse the parameter type, stopping at the closing `)` at depth 0.
                    let ty_start = self.token_cursor;
                    let rparen_idx = self
                        .find_token_at_depth0(ty_start, self.tokens.len(), |t| {
                            matches!(t, Token::ParenR(..))
                        })
                        .ok_or_else(|| ParserErr::new(self.eof, "expected `)`".to_string()))?;
                    let ann = self.parse_type_expr_slice(&self.tokens[ty_start..rparen_idx])?;
                    self.token_cursor = rparen_idx;

                    let _ = self.expect_paren_r()?;

                    params.push((Var::with_span(param_span, param_name), ann));
                } else if is_named_param_head(&tok, &next) {
                    // x: a -> y: b -> i32
                    let (param_name, param_span) = self.expect_ident("parameter name")?;

                    self.expect_colon()?;

                    // Parse the parameter type, stopping at the `->` separator at depth 0.
                    // To use a function type as a parameter type, parentheses are required:
                    //   x: (a -> c) -> ...
                    let ty_start = self.token_cursor;
                    let mut depth = 0usize;
                    let mut arrow_idx = None;
                    let mut stop_span = None;
                    for i in ty_start..self.tokens.len() {
                        match self.tokens[i] {
                            Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => {
                                depth += 1
                            }
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
                } else {
                    return Err(ParserErr::new(
                        *tok.span(),
                        format!("expected `(` or parameter name got {}", tok),
                    ));
                }

                // Separator: always `->` after a parameter.
                match self.current_token() {
                    Token::ArrowR(..) => self.next_token(),
                    token => {
                        return Err(ParserErr::new(
                            *token.span(),
                            format!("expected `->` got {}", token),
                        ));
                    }
                }

                // If another param head follows, keep parsing params; otherwise we are at the return type.
                let tok = self.current_token();
                let next = self.peek_token(1);
                let next2 = self.peek_token(2);
                if is_paren_param_head(&tok, &next, &next2) || is_named_param_head(&tok, &next) {
                    continue;
                }
                break;
            }
        }

        // Find `=` and (optional) `where` between return type and `=`.
        let ret_start = self.token_cursor;
        let assign_idx = self
            .find_token_at_depth0(ret_start, self.tokens.len(), |t| {
                matches!(t, Token::Assign(..))
            })
            .ok_or_else(|| {
                ParserErr::new(self.eof, "expected `=` in function declaration".to_string())
            })?;

        let ret_end = self
            .find_token_at_depth0(ret_start, assign_idx, |t| matches!(t, Token::Where(..)))
            .unwrap_or(assign_idx);
        let ret = self.parse_type_expr_slice(&self.tokens[ret_start..ret_end])?;
        self.token_cursor = ret_end;

        let mut constraints = Vec::new();
        if matches!(self.current_token(), Token::Where(..)) {
            self.next_token();
            constraints = self.parse_type_constraints()?;
        }

        // `=`
        self.expect_assign()?;

        // Parse a body expression, delimited by newline (unless inside parens/brackets/braces).
        let body_start = self.token_cursor;
        let body_end = self.find_same_line_end(body_start)?;
        let body = self.parse_expr_slice(&self.tokens[body_start..body_end])?;
        self.token_cursor = body_end;
        let span_end = body.span().end;

        Ok(FnDecl {
            span: Span::from_begin_end(span_begin, span_end),
            is_pub,
            name: name_var,
            params,
            ret,
            constraints,
            body: Arc::new(body),
        })
    }

    fn parse_declare_fn_decl_toplevel(&mut self, is_pub: bool) -> Result<DeclareFnDecl, ParserErr> {
        let start_idx = self.token_cursor;
        // `declare fn` has no body, so we parse only the declaration line.
        let end_idx = self.find_same_line_end(start_idx)?;

        let eof = self
            .tokens
            .get(end_idx.saturating_sub(1))
            .map(|t| *t.span())
            .unwrap_or(self.eof);
        let tokens = Tokens {
            items: self.tokens[start_idx..end_idx].to_vec(),
            eof,
        };
        let mut parser = Parser::new(tokens);
        let decl = parser.parse_declare_fn_decl(is_pub)?;
        match parser.current_token() {
            Token::Eof(..) => {}
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("unexpected {} in declaration", token),
                ));
            }
        }

        self.token_cursor = end_idx;
        self.skip_newlines();
        Ok(decl)
    }

    fn parse_declare_fn_decl(&mut self, is_pub: bool) -> Result<DeclareFnDecl, ParserErr> {
        let span_begin = match self.current_token() {
            Token::Declare(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `declare` got {}", token),
                ));
            }
        };

        match self.current_token() {
            Token::Fn(..) => self.next_token(),
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `fn` got {}", token),
                ));
            }
        }

        let (name, name_span) = match self.current_token() {
            Token::Ident(name, span, ..) => {
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

        // Allow an optional signature delimiter:
        //   declare fn id : a -> a
        if matches!(self.current_token(), Token::Colon(..)) {
            self.next_token();
        }

        let is_named_param_head = |token: &Token, next: &Token| {
            matches!(token, Token::Ident(..)) && matches!(next, Token::Colon(..))
        };
        let is_paren_param_head = |token: &Token, next: &Token, next2: &Token| {
            matches!(token, Token::ParenL(..))
                && matches!(next, Token::Ident(..))
                && matches!(next2, Token::Colon(..))
        };

        // `declare fn` supports two surface syntaxes:
        //  1) A full `fn`-style header with named parameters.
        //  2) A bare type signature (like class methods): `declare fn id a -> a`.
        let tok = self.current_token();
        let next = self.peek_token(1);
        let next2 = self.peek_token(2);
        let mut use_named_params =
            is_named_param_head(&tok, &next) || is_paren_param_head(&tok, &next, &next2);

        if !use_named_params && matches!(tok, Token::ParenL(..)) {
            // `()` means nullary params in the legacy `fn` syntax, so treat it as named-params mode.
            if matches!(next, Token::ParenR(..)) {
                use_named_params = true;
            } else {
                // Disambiguate legacy `(x: a, y: b)` params from grouped type expressions like `(a, b) -> c`.
                if self.paren_group_has_top_level_colon(self.token_cursor) {
                    use_named_params = true;
                }
            }
        }

        if !use_named_params {
            // Parse a bare type signature.
            let sig_start = self.token_cursor;
            let stop_idx = self.find_token_at_depth0(sig_start, self.tokens.len(), |t| {
                matches!(t, Token::Where(..) | Token::Assign(..))
            });
            let sig_end = match stop_idx {
                Some(i) => match self.tokens[i] {
                    Token::Where(..) => i,
                    Token::Assign(span, ..) => {
                        return Err(ParserErr::new(
                            span,
                            "declare fn cannot have a body; use `fn`".to_string(),
                        ));
                    }
                    _ => i,
                },
                None => self.tokens.len(),
            };
            let sig = self.parse_type_expr_slice(&self.tokens[sig_start..sig_end])?;
            self.token_cursor = sig_end;

            let mut constraints = Vec::new();
            if matches!(self.current_token(), Token::Where(..)) {
                self.next_token();
                constraints = self.parse_type_constraints()?;
            }

            // Flatten `a -> b -> c` into params=[a,b], ret=c so downstream code can
            // reconstruct the same function type.
            let mut param_tys = Vec::new();
            let mut cur = sig;
            let ret = loop {
                match cur {
                    TypeExpr::Fun(_, arg, next_ret) => {
                        param_tys.push(*arg);
                        cur = *next_ret;
                    }
                    other => break other,
                }
            };
            params = param_tys
                .into_iter()
                .enumerate()
                .map(|(i, ann)| (Var::with_span(*ann.span(), format!("_arg{i}")), ann))
                .collect();

            let span_end = constraints
                .last()
                .map(|c| c.typ.span().end)
                .unwrap_or(ret.span().end);
            return Ok(DeclareFnDecl {
                span: Span::from_begin_end(span_begin, span_end),
                is_pub,
                name: name_var,
                params,
                ret,
                constraints,
            });
        }

        // Params (new syntax):
        //   declare fn foo x: a -> y: b -> i32 where ...
        //   declare fn foo (x: a) -> (y: b) -> i32
        //
        // Params (legacy syntax, still accepted):
        //   declare fn foo (x: a, y: b) -> i32
        if matches!(self.current_token(), Token::ParenL(..)) {
            // `()` is also treated as the legacy group syntax (nullary functions).
            if self.paren_group_has_top_level_comma(self.token_cursor)
                || matches!(self.peek_token(1), Token::ParenR(..))
            {
                params = self.parse_legacy_param_group()?;

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
        }

        // New syntax: a chain of named params (`x: a -> ...`) and/or parenthesized params (`(x: a) -> ...`).
        // Parameters are always delimited by `->`. Adjacency is not accepted.
        if params.is_empty() {
            loop {
                let tok = self.current_token();
                let next = self.peek_token(1);
                let next2 = self.peek_token(2);
                if is_paren_param_head(&tok, &next, &next2) {
                    // (x: a)
                    self.next_token(); // `(`
                    let (param_name, param_span) = self.expect_ident("parameter name")?;
                    self.expect_colon()?;

                    // Parse the parameter type, stopping at the closing `)` at depth 0.
                    let ty_start = self.token_cursor;
                    let rparen_idx = self
                        .find_token_at_depth0(ty_start, self.tokens.len(), |t| {
                            matches!(t, Token::ParenR(..))
                        })
                        .ok_or_else(|| ParserErr::new(self.eof, "expected `)`".to_string()))?;
                    let ann = self.parse_type_expr_slice(&self.tokens[ty_start..rparen_idx])?;
                    self.token_cursor = rparen_idx;

                    let _ = self.expect_paren_r()?;

                    params.push((Var::with_span(param_span, param_name), ann));
                } else if is_named_param_head(&tok, &next) {
                    // x: a -> y: b -> i32
                    let (param_name, param_span) = self.expect_ident("parameter name")?;
                    self.expect_colon()?;

                    // Parse the parameter type, stopping at the `->` separator at depth 0.
                    // To use a function type as a parameter type, parentheses are required:
                    //   x: (a -> c) -> ...
                    let ty_start = self.token_cursor;
                    let mut depth = 0usize;
                    let mut arrow_idx = None;
                    let mut stop_span = None;
                    for i in ty_start..self.tokens.len() {
                        match self.tokens[i] {
                            Token::ParenL(..) | Token::BracketL(..) | Token::BraceL(..) => {
                                depth += 1
                            }
                            Token::ParenR(..) | Token::BracketR(..) | Token::BraceR(..) => {
                                depth = depth.saturating_sub(1)
                            }
                            Token::ArrowR(..) if depth == 0 => {
                                arrow_idx = Some(i);
                                break;
                            }
                            Token::Where(span, ..) if depth == 0 => {
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
                } else {
                    return Err(ParserErr::new(
                        *tok.span(),
                        format!("expected `(` or parameter name got {}", tok),
                    ));
                }

                // Separator: always `->` after a parameter.
                match self.current_token() {
                    Token::ArrowR(..) => self.next_token(),
                    token => {
                        return Err(ParserErr::new(
                            *token.span(),
                            format!("expected `->` got {}", token),
                        ));
                    }
                }

                // If another param head follows, keep parsing params; otherwise we are at the return type.
                let tok = self.current_token();
                let next = self.peek_token(1);
                let next2 = self.peek_token(2);
                if is_paren_param_head(&tok, &next, &next2) || is_named_param_head(&tok, &next) {
                    continue;
                }
                break;
            }
        }

        // Find an (optional) `where` in the remaining tokens. `declare fn` must not have `=`.
        let ret_start = self.token_cursor;
        let stop_idx = self.find_token_at_depth0(ret_start, self.tokens.len(), |t| {
            matches!(t, Token::Where(..) | Token::Assign(..))
        });
        let ret_end = match stop_idx {
            Some(i) => match self.tokens[i] {
                Token::Where(..) => i,
                Token::Assign(span, ..) => {
                    return Err(ParserErr::new(
                        span,
                        "declare fn cannot have a body; use `fn`".to_string(),
                    ));
                }
                _ => i,
            },
            None => self.tokens.len(),
        };
        let ret = self.parse_type_expr_slice(&self.tokens[ret_start..ret_end])?;
        self.token_cursor = ret_end;

        let mut constraints = Vec::new();
        if matches!(self.current_token(), Token::Where(..)) {
            self.next_token();
            constraints = self.parse_type_constraints()?;
        }

        let span_end = constraints
            .last()
            .map(|c| c.typ.span().end)
            .unwrap_or(ret.span().end);
        Ok(DeclareFnDecl {
            span: Span::from_begin_end(span_begin, span_end),
            is_pub,
            name: name_var,
            params,
            ret,
            constraints,
        })
    }

    fn parse_import_decl(&mut self, is_pub: bool) -> Result<ImportDecl, ParserErr> {
        let span_begin = match self.current_token() {
            Token::Import(span, ..) => {
                self.next_token();
                span.begin
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected `import` got {}", token),
                ));
            }
        };

        let (path, mut span_end, default_alias) = match self.current_token() {
            Token::HttpsUrl(url, span, ..) => {
                let url = url.clone();
                self.next_token();
                let (base_url, sha) = match url.split_once('#') {
                    Some((a, b)) if !b.is_empty() => (a.to_string(), Some(b.to_string())),
                    _ => (url, None),
                };
                let alias = base_url
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(".rex")
                    .to_string();
                (
                    ImportPath::Remote { url: base_url, sha },
                    span.end,
                    if alias.is_empty() {
                        None
                    } else {
                        Some(intern(&alias))
                    },
                )
            }
            Token::Ident(..) => {
                let mut segs: Vec<Symbol> = Vec::new();
                let (first, first_span) = match self.current_token() {
                    Token::Ident(name, span, ..) => (intern(&name), span),
                    token => {
                        return Err(ParserErr::new(
                            *token.span(),
                            format!("expected module path segment got {}", token),
                        ));
                    }
                };
                let mut end = first_span.end;
                segs.push(first);
                self.next_token();

                while matches!(self.current_token(), Token::Dot(..)) {
                    self.next_token();
                    let (seg, seg_span) = match self.current_token() {
                        Token::Ident(name, span, ..) => (intern(&name), span),
                        token => {
                            return Err(ParserErr::new(
                                *token.span(),
                                format!("expected module path segment got {}", token),
                            ));
                        }
                    };
                    segs.push(seg);
                    end = seg_span.end;
                    self.next_token();
                }

                let sha = if matches!(self.current_token(), Token::HashTag(..)) {
                    self.next_token();
                    match self.current_token() {
                        Token::Ident(s, span, ..) => {
                            self.next_token();
                            end = end.max(span.end);
                            Some(s.clone())
                        }
                        Token::Int(n, span, ..) => {
                            self.next_token();
                            end = end.max(span.end);
                            Some(n.to_string())
                        }
                        token => {
                            return Err(ParserErr::new(
                                *token.span(),
                                format!("expected sha token got {}", token),
                            ));
                        }
                    }
                } else {
                    None
                };
                let default_alias = segs.last().cloned();
                (
                    ImportPath::Local {
                        segments: segs,
                        sha,
                    },
                    end,
                    default_alias,
                )
            }
            token => {
                return Err(ParserErr::new(
                    *token.span(),
                    format!("expected module path got {}", token),
                ));
            }
        };

        let clause = if matches!(self.current_token(), Token::ParenL(..)) {
            self.next_token();
            if matches!(self.current_token(), Token::Mul(..)) {
                self.next_token();
                let end = match self.current_token() {
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
                span_end = span_end.max(end);
                Some(ImportClause::All)
            } else {
                let mut items: Vec<ImportItem> = Vec::new();
                let mut local_names: HashSet<Symbol> = HashSet::new();
                loop {
                    let (name, item_span) = self.parse_value_name()?;
                    let mut item_end = item_span.end;
                    let alias = if matches!(self.current_token(), Token::As(..)) {
                        self.next_token();
                        match self.current_token() {
                            Token::Ident(alias, alias_span, ..) => {
                                self.next_token();
                                item_end = alias_span.end;
                                Some(intern(&alias))
                            }
                            token => {
                                return Err(ParserErr::new(
                                    *token.span(),
                                    format!("expected alias name got {}", token),
                                ));
                            }
                        }
                    } else {
                        None
                    };

                    let local_name = alias.clone().unwrap_or_else(|| name.clone());
                    if local_names.contains(&local_name) {
                        return Err(ParserErr::new(
                            Span::from_begin_end(item_span.begin, item_end),
                            format!("duplicate imported name `{local_name}`"),
                        ));
                    }
                    local_names.insert(local_name);
                    items.push(ImportItem { name, alias });

                    match self.current_token() {
                        Token::Comma(..) => {
                            self.next_token();
                        }
                        Token::ParenR(span, ..) => {
                            self.next_token();
                            span_end = span_end.max(span.end.max(item_end));
                            break;
                        }
                        token => {
                            return Err(ParserErr::new(
                                *token.span(),
                                format!("expected `,` or `)` got {}", token),
                            ));
                        }
                    }
                }
                Some(ImportClause::Items(items))
            }
        } else {
            None
        };

        let alias = if matches!(self.current_token(), Token::As(..)) {
            if clause.is_some() {
                let span = *self.current_token().span();
                return Err(ParserErr::new(
                    span,
                    "cannot combine `as <alias>` with import clause `(...)`".to_string(),
                ));
            }
            self.next_token();
            match self.current_token() {
                Token::Ident(name, span, ..) => {
                    self.next_token();
                    span_end = span_end.max(span.end);
                    intern(&name)
                }
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected alias name got {}", token),
                    ));
                }
            }
        } else {
            default_alias
                .or_else(|| clause.as_ref().map(|_| intern("_")))
                .ok_or_else(|| {
                    ParserErr::new(
                        Span::from_begin_end(span_begin, span_end),
                        "import requires `as <alias>`".to_string(),
                    )
                })?
        };

        self.skip_newlines();
        Ok(ImportDecl {
            span: Span::from_begin_end(span_begin, span_end),
            is_pub,
            path,
            alias,
            clause,
        })
    }

    fn parse_type_decl(&mut self, is_pub: bool) -> Result<TypeDecl, ParserErr> {
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
        while let Token::Pipe(..) = self.current_token() {
            self.next_token();
            let (variant, vspan) = self.parse_type_variant()?;
            span_end = vspan.end;
            variants.push(variant);
        }

        Ok(TypeDecl {
            span: Span::from_begin_end(span_begin, span_end),
            is_pub,
            name,
            params,
            variants,
        })
    }

    fn parse_type_variant(&mut self) -> Result<(TypeVariant, Span), ParserErr> {
        let (name, name_span) = match self.current_token() {
            Token::Ident(name, span, ..) => {
                let name = intern(&name);
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
        while let Token::Ident(..) | Token::ParenL(..) | Token::BraceL(..) = self.current_token() {
            let arg = self.parse_type_atom()?;
            span_end = arg.span().end;
            args.push(arg);
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
        let span = *self.current_token().span();
        self.with_nesting(span, |this| {
            let lhs = this.parse_type_app()?;
            match this.current_token() {
                Token::ArrowR(..) => {
                    this.next_token();
                    let rhs = this.parse_type_fun()?;
                    let span = Span::from_begin_end(lhs.span().begin, rhs.span().end);
                    Ok(TypeExpr::Fun(span, Box::new(lhs), Box::new(rhs)))
                }
                _ => Ok(lhs),
            }
        })
    }

    fn parse_type_app(&mut self) -> Result<TypeExpr, ParserErr> {
        let mut lhs = self.parse_type_atom()?;
        while let Token::Ident(..) | Token::ParenL(..) | Token::BraceL(..) = self.current_token() {
            let rhs = self.parse_type_atom()?;
            let span = Span::from_begin_end(lhs.span().begin, rhs.span().end);
            lhs = TypeExpr::App(span, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_type_atom(&mut self) -> Result<TypeExpr, ParserErr> {
        match self.current_token() {
            Token::Ident(name, span, ..) => {
                let name = intern(&name);
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

        self.with_nesting(Span::from_begin_end(span_begin, span_begin), |this| {
            // Unit type: `()`
            if let Token::ParenR(span, ..) = this.current_token() {
                this.next_token();
                return Ok(TypeExpr::Tuple(
                    Span::from_begin_end(span_begin, span.end),
                    Vec::new(),
                ));
            }

            let first = this.parse_type_expr()?;
            let mut elems = Vec::new();
            let span_end = match this.current_token() {
                Token::Comma(..) => {
                    this.next_token();
                    elems.push(first);
                    loop {
                        elems.push(this.parse_type_expr()?);
                        match this.current_token() {
                            Token::Comma(..) => {
                                this.next_token();
                                continue;
                            }
                            Token::ParenR(span, ..) => {
                                this.next_token();
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
                    this.next_token();
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
        })
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

        self.with_nesting(Span::from_begin_end(span_begin, span_begin), |this| {
            let mut fields = Vec::new();
            if let Token::BraceR(span, ..) = this.current_token() {
                this.next_token();
                return Ok(TypeExpr::Record(
                    Span::from_begin_end(span_begin, span.end),
                    fields,
                ));
            }

            let span_end = loop {
                let (name, _span) = match this.current_token() {
                    Token::Ident(name, span, ..) => {
                        let name = intern(&name);
                        this.next_token();
                        (name, span)
                    }
                    token => {
                        return Err(ParserErr::new(
                            *token.span(),
                            format!("expected field name got {}", token),
                        ));
                    }
                };

                match this.current_token() {
                    Token::Colon(..) => this.next_token(),
                    token => {
                        return Err(ParserErr::new(
                            *token.span(),
                            format!("expected `:` got {}", token),
                        ));
                    }
                }

                let ty = this.parse_type_expr()?;
                fields.push((name, ty));

                match this.current_token() {
                    Token::Comma(..) => {
                        this.next_token();
                    }
                    Token::BraceR(span, ..) => {
                        this.next_token();
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
        })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParserErr> {
        self.parse_pattern_cons()
    }

    fn parse_pattern_cons(&mut self) -> Result<Pattern, ParserErr> {
        let span = *self.current_token().span();
        self.with_nesting(span, |this| {
            let mut lhs = this.parse_pattern_app()?;
            while let Token::ColonColon(..) = this.current_token() {
                this.next_token();
                let rhs = this.parse_pattern_cons()?;
                let span = Span::from_begin_end(lhs.span().begin, rhs.span().end);
                lhs = Pattern::Cons(span, Box::new(lhs), Box::new(rhs));
            }
            Ok(lhs)
        })
    }

    fn parse_pattern_app(&mut self) -> Result<Pattern, ParserErr> {
        let head = self.parse_pattern_atom()?;
        let mut args = Vec::new();
        while let Token::Ident(..) | Token::BracketL(..) | Token::BraceL(..) | Token::ParenL(..) =
            self.current_token()
        {
            let arg = self.parse_pattern_atom()?;
            args.push(arg);
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
            let end = args.last().map(|p| p.span().end).unwrap_or(begin);
            Ok(Pattern::Named(
                Span::from_begin_end(begin, end),
                var.name,
                args,
            ))
        } else {
            let span = args
                .first()
                .map(|p| *p.span())
                .unwrap_or_else(|| *self.current_token().span());
            Err(ParserErr::new(
                span,
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

        self.with_nesting(Span::from_begin_end(span_begin, span_begin), |this| {
            if let Token::BracketR(span, ..) = this.current_token() {
                this.next_token();
                return Ok(Pattern::List(
                    Span::from_begin_end(span_begin, span.end),
                    Vec::new(),
                ));
            }

            let mut patterns = Vec::new();
            let span_end = loop {
                patterns.push(this.parse_pattern()?);

                match this.current_token() {
                    Token::Comma(..) => {
                        this.next_token();
                    }
                    Token::BracketR(span, ..) => {
                        this.next_token();
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
        })
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

        self.with_nesting(Span::from_begin_end(span_begin, span_begin), |this| {
            if let Token::BraceR(span, ..) = this.current_token() {
                this.next_token();
                return Ok(Pattern::Dict(
                    Span::from_begin_end(span_begin, span.end),
                    Vec::new(),
                ));
            }

            let mut fields = Vec::new();
            let span_end = loop {
                match this.current_token() {
                    Token::Ident(name, key_span, ..) => {
                        let key_name = name;
                        let key = intern(&key_name);
                        this.next_token();

                        let pat = if matches!(this.current_token(), Token::Colon(..)) {
                            this.next_token();
                            this.parse_pattern()?
                        } else {
                            Pattern::Var(Var::with_span(key_span, key_name))
                        };
                        fields.push((key, pat));

                        match this.current_token() {
                            Token::Comma(..) => {
                                this.next_token();
                            }
                            Token::BraceR(span, ..) => {
                                this.next_token();
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
                fields,
            ))
        })
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

        self.with_nesting(Span::from_begin_end(span_begin, span_begin), |this| {
            // Unit tuple pattern: `()`
            if let Token::ParenR(span, ..) = this.current_token() {
                this.next_token();
                return Ok(Pattern::Tuple(
                    Span::from_begin_end(span_begin, span.end),
                    Vec::new(),
                ));
            }

            let first = this.parse_pattern()?;
            let mut elems = Vec::new();
            let span_end = match this.current_token() {
                Token::Comma(..) => {
                    this.next_token();
                    elems.push(first);
                    loop {
                        elems.push(this.parse_pattern()?);
                        match this.current_token() {
                            Token::Comma(..) => {
                                this.next_token();
                            }
                            Token::ParenR(span, ..) => {
                                this.next_token();
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
                Token::ParenR(span, ..) => {
                    this.next_token();
                    return Ok(first.with_span(Span::from_begin_end(span_begin, span.end)));
                }
                token => {
                    return Err(ParserErr::new(
                        *token.span(),
                        format!("expected `)` or `,` got {}", token),
                    ));
                }
            };

            Ok(Pattern::Tuple(
                Span::from_begin_end(span_begin, span_end),
                elems,
            ))
        })
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

    fn parse_hole_expr(&mut self) -> Result<Expr, ParserErr> {
        let token = self.current_token();
        self.next_token();
        match token {
            Token::Question(span, ..) => Ok(Expr::Hole(span)),
            token => Err(ParserErr::new(
                *token.span(),
                format!("expected `?` got {}", token),
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

fn count_expr_nodes(expr: &Expr) -> u64 {
    match expr {
        Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..)
        | Expr::Hole(..)
        | Expr::Var(..) => 1,
        Expr::Tuple(_, xs) | Expr::List(_, xs) => {
            1 + xs.iter().map(|e| count_expr_nodes(e)).sum::<u64>()
        }
        Expr::Dict(_, kvs) => 1 + kvs.values().map(|e| count_expr_nodes(e)).sum::<u64>(),
        Expr::RecordUpdate(_, base, updates) => {
            1 + count_expr_nodes(base) + updates.values().map(|e| count_expr_nodes(e)).sum::<u64>()
        }
        Expr::App(_, f, x) => 1 + count_expr_nodes(f) + count_expr_nodes(x),
        Expr::Project(_, e, _) => 1 + count_expr_nodes(e),
        Expr::Lam(_, _, _, ann, constraints, body) => {
            let ann_nodes = ann.as_ref().map(count_type_expr_nodes).unwrap_or(0);
            let constraint_nodes = constraints
                .iter()
                .map(count_type_constraint_nodes)
                .sum::<u64>();
            1 + ann_nodes + constraint_nodes + count_expr_nodes(body)
        }
        Expr::Let(_, _, ann, def, body) => {
            let ann_nodes = ann.as_ref().map(count_type_expr_nodes).unwrap_or(0);
            1 + ann_nodes + count_expr_nodes(def) + count_expr_nodes(body)
        }
        Expr::LetRec(_, bindings, body) => {
            let binding_nodes = bindings
                .iter()
                .map(|(_, ann, def)| {
                    ann.as_ref().map(count_type_expr_nodes).unwrap_or(0) + count_expr_nodes(def)
                })
                .sum::<u64>();
            1 + binding_nodes + count_expr_nodes(body)
        }
        Expr::Ite(_, a, b, c) => {
            1 + count_expr_nodes(a) + count_expr_nodes(b) + count_expr_nodes(c)
        }
        Expr::Match(_, scrutinee, arms) => {
            1 + count_expr_nodes(scrutinee)
                + arms
                    .iter()
                    .map(|(pat, e)| count_pattern_nodes(pat) + count_expr_nodes(e))
                    .sum::<u64>()
        }
        Expr::Ann(_, e, ty) => 1 + count_expr_nodes(e) + count_type_expr_nodes(ty),
    }
}

fn count_pattern_nodes(pat: &Pattern) -> u64 {
    match pat {
        Pattern::Wildcard(..) | Pattern::Var(..) => 1,
        Pattern::Named(_, _, ps) | Pattern::Tuple(_, ps) | Pattern::List(_, ps) => {
            1 + ps.iter().map(count_pattern_nodes).sum::<u64>()
        }
        Pattern::Cons(_, a, b) => 1 + count_pattern_nodes(a) + count_pattern_nodes(b),
        Pattern::Dict(_, fields) => {
            1 + fields
                .iter()
                .map(|(_, p)| count_pattern_nodes(p))
                .sum::<u64>()
        }
    }
}

fn count_type_constraint_nodes(c: &TypeConstraint) -> u64 {
    1 + count_type_expr_nodes(&c.typ)
}

fn count_type_expr_nodes(ty: &TypeExpr) -> u64 {
    match ty {
        TypeExpr::Name(..) => 1,
        TypeExpr::App(_, a, b) | TypeExpr::Fun(_, a, b) => {
            1 + count_type_expr_nodes(a) + count_type_expr_nodes(b)
        }
        TypeExpr::Tuple(_, elems) => 1 + elems.iter().map(count_type_expr_nodes).sum::<u64>(),
        TypeExpr::Record(_, fields) => {
            1 + fields
                .iter()
                .map(|(_, t)| count_type_expr_nodes(t))
                .sum::<u64>()
        }
    }
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
    use rex_lexer::{Token, span, span::Span};

    use super::*;

    fn parse(code: &str) -> Arc<Expr> {
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        parser.parse_program(&mut GasMeter::default()).unwrap().expr
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
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
        assert_expr_eq!(expr, b!(span!(1:1 - 1:5); true));

        let mut parser = Parser::new(Token::tokenize("{- this is a boolean -} false").unwrap());
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
        assert_expr_eq!(expr, b!(span!(1:25 - 1:30); false));

        let mut parser = Parser::new(Token::tokenize("(3.54 {- this is a float -}, {- this is an int -} 42, false {- this is a boolean -})").unwrap());
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
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
    fn test_max_nesting_depth_is_enforced_during_parse() {
        let code = format!("{}0{}", "(".repeat(6), ")".repeat(6));
        let mut parser = Parser::new(Token::tokenize(&code).unwrap());
        parser.set_limits(ParserLimits {
            max_nesting: Some(5),
        });

        let errs = parser.parse_program(&mut GasMeter::default()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.to_string().contains("maximum nesting depth exceeded")),
            "expected a max-nesting parse error, got: {errs:?}"
        );
    }

    #[test]
    fn test_max_nesting_binary_chain() {
        let code = std::iter::repeat_n("1", 12).collect::<Vec<_>>().join(" + ");
        let mut parser = Parser::new(Token::tokenize(&code).unwrap());
        parser.set_limits(ParserLimits {
            max_nesting: Some(5),
        });

        let errs = parser.parse_program(&mut GasMeter::default()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.to_string().contains("maximum nesting depth exceeded")),
            "expected a max-nesting parse error, got: {errs:?}"
        );
    }

    #[test]
    fn test_max_nesting_type_fun_chain() {
        let ty_chain = std::iter::repeat_n("a", 12)
            .collect::<Vec<_>>()
            .join(" -> ");
        let code = format!("let t: {ty_chain} = x in t");
        let mut parser = Parser::new(Token::tokenize(&code).unwrap());
        parser.set_limits(ParserLimits {
            max_nesting: Some(5),
        });

        let errs = parser.parse_program(&mut GasMeter::default()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.to_string().contains("maximum nesting depth exceeded")),
            "expected a max-nesting parse error, got: {errs:?}"
        );
    }

    #[test]
    fn test_max_nesting_cons_pattern_chain() {
        let pattern = (1..=12)
            .map(|i| format!("x{i}"))
            .collect::<Vec<_>>()
            .join(" :: ");
        let code = format!("match xs when {pattern} -> xs");
        let mut parser = Parser::new(Token::tokenize(&code).unwrap());
        parser.set_limits(ParserLimits {
            max_nesting: Some(5),
        });

        let errs = parser.parse_program(&mut GasMeter::default()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.to_string().contains("maximum nesting depth exceeded")),
            "expected a max-nesting parse error, got: {errs:?}"
        );
    }

    #[test]
    fn test_add() {
        let mut parser = Parser::new(Token::tokenize("1 + 2").unwrap());
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
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

        let mut parser = Parser::new(Token::tokenize("(6.9 + 3.17)").unwrap());
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:13);
                app!(
                    span!(1:2 - 1:7);
                    v!(span!(1:6 - 1:7); "+"),
                    f!(span!(1:2 - 1:5); 6.9)
                ),
                f!(span!(1:8 - 1:12); 3.17)
            )
        );

        let mut parser = Parser::new(Token::tokenize("(+) 420").unwrap());
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
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
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
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
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
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
    fn test_parse_fn_decl_signature_form_with_lambda_body() {
        let code = r#"
        fn add : i32 -> i32 -> i32 = \x y -> x + y
        add 1 2
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
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
                assert!(!matches!(fd.body.as_ref(), Expr::Lam(..)));
            }
            other => panic!("expected fn decl, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_fn_sig_multiline_lambda() {
        let code = r#"
        fn f : i32 -> i32 = \x ->
          x + 1
        f 1
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::Fn(fd) => {
                assert_eq!(fd.name.name, intern("f"));
                assert_eq!(fd.params.len(), 1);
                assert_eq!(fd.params[0].0.name, intern("x"));
                assert!(matches!(
                    fd.params[0].1,
                    TypeExpr::Name(_, ref n) if n.as_ref() == "i32"
                ));
                assert!(matches!(fd.ret, TypeExpr::Name(_, ref n) if n.as_ref() == "i32"));
            }
            other => panic!("expected fn decl, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_fn_decl_signature_form_eta_expands_non_lambda_body() {
        let code = r#"
        fn inc : i32 -> i32 = add 1
        inc
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::Fn(fd) => {
                assert_eq!(fd.name.name, intern("inc"));
                assert_eq!(fd.params.len(), 1);
                assert_eq!(fd.params[0].0.name, intern("_arg0"));
                assert!(matches!(
                    fd.params[0].1,
                    TypeExpr::Name(_, ref n) if n.as_ref() == "i32"
                ));
                assert!(matches!(fd.ret, TypeExpr::Name(_, ref n) if n.as_ref() == "i32"));
            }
            other => panic!("expected fn decl, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_fn_decl_signature_form_where_constraints() {
        let code = r#"
        fn my_fun : a -> b -> c where Iterable (a, b) = \x y -> x
        my_fun
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::Fn(fd) => {
                assert_eq!(fd.name.name, intern("my_fun"));
                assert_eq!(fd.params.len(), 2);
                assert!(matches!(
                    fd.constraints[0].class,
                    ref n if n.as_ref() == "Iterable"
                ));
            }
            other => panic!("expected fn decl, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_fn_decl_signature_form_rejects_mismatched_lambda_arity() {
        let code = r#"
        fn add : i32 -> i32 -> i32 = \x -> x
        add
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        assert!(parser.parse_program(&mut GasMeter::default()).is_err());
    }

    #[test]
    fn test_parse_fn_decl_where_constraints() {
        let code = r#"
        fn my_fun x: a -> y: b -> c where Iterable (a, b) = x
        my_fun
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
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
    fn test_parse_declare_fn_decl_where_constraints() {
        let code = r#"
        declare fn my_fun x: a -> y: b -> c where Iterable (a, b)
        42
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::DeclareFn(fd) => {
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
            other => panic!("expected declare fn decl, got {other:?}"),
        }
        assert_expr_eq!(program.expr, u!(span!(3:9 - 3:11); 42));
    }

    #[test]
    fn test_parse_declare_fn_decl_bare_signature() {
        let code = r#"
        declare fn info a -> string where Show a
        0
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::DeclareFn(fd) => {
                assert_eq!(fd.name.name, intern("info"));
                assert_eq!(fd.params.len(), 1);
                assert!(matches!(
                    fd.params[0].1,
                    TypeExpr::Name(_, ref n) if n.as_ref() == "a"
                ));
                assert!(matches!(
                    fd.ret,
                    TypeExpr::Name(_, ref n) if n.as_ref() == "string"
                ));
                assert_eq!(fd.constraints.len(), 1);
                assert!(matches!(
                    fd.constraints[0].class,
                    ref n if n.as_ref() == "Show"
                ));
            }
            other => panic!("expected declare fn decl, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_declare_fn_decl_bare_signature_with_colon() {
        let code = r#"
        declare fn info : a -> string where Show a
        0
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::DeclareFn(fd) => {
                assert_eq!(fd.name.name, intern("info"));
                assert_eq!(fd.params.len(), 1);
                assert!(matches!(
                    fd.params[0].1,
                    TypeExpr::Name(_, ref n) if n.as_ref() == "a"
                ));
                assert!(matches!(
                    fd.ret,
                    TypeExpr::Name(_, ref n) if n.as_ref() == "string"
                ));
                assert_eq!(fd.constraints.len(), 1);
                assert!(matches!(
                    fd.constraints[0].class,
                    ref n if n.as_ref() == "Show"
                ));
            }
            other => panic!("expected declare fn decl, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_declare_fn_decl_rejects_body() {
        let code = r#"
        declare fn my_fun x: a -> a = x
        0
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        assert!(parser.parse_program(&mut GasMeter::default()).is_err());
    }

    #[test]
    fn test_parse_fn_decl_param_fun_type_requires_parens() {
        let code = r#"
        fn apply x: (a -> c) -> y: a -> c = x y
        apply
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::Fn(fd) => {
                assert_eq!(fd.name.name, intern("apply"));
                assert_eq!(fd.params.len(), 2);
                assert_eq!(fd.params[0].0.name, intern("x"));
                assert!(matches!(fd.params[0].1, TypeExpr::Fun(_, _, _)));
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
    fn test_parse_fn_decl_parenthesized_params_allow_fun_types() {
        let code = r#"
        fn reduce (f: a -> a -> a) -> (x: t a) -> a = x
        reduce
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::Fn(fd) => {
                assert_eq!(fd.name.name, intern("reduce"));
                assert_eq!(fd.params.len(), 2);
                assert_eq!(fd.params[0].0.name, intern("f"));
                assert!(matches!(fd.params[0].1, TypeExpr::Fun(..)));
                assert_eq!(fd.params[1].0.name, intern("x"));
                assert!(matches!(fd.params[1].1, TypeExpr::App(..)));
                assert!(matches!(fd.ret, TypeExpr::Name(_, ref n) if n.as_ref() == "a"));
            }
            other => panic!("expected fn decl, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_fn_decl_parenthesized_params_require_arrow_delimiter() {
        let code = r#"
        fn reduce (f: a -> a -> a) (x: t a) -> a = x
        reduce
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        assert!(parser.parse_program(&mut GasMeter::default()).is_err());
    }

    #[test]
    fn test_parse_unit_type() {
        let code = r#"
        fn unit_id x: () -> () = x
        unit_id ()
        "#;
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            Decl::Fn(fd) => {
                assert_eq!(fd.name.name, intern("unit_id"));
                assert_eq!(fd.params.len(), 1);
                assert!(matches!(fd.params[0].1, TypeExpr::Tuple(_, ref xs) if xs.is_empty()));
                assert!(matches!(fd.ret, TypeExpr::Tuple(_, ref xs) if xs.is_empty()));
            }
            other => panic!("expected fn decl, got {other:?}"),
        }
    }

    #[test]
    fn test_sub() {
        let mut parser = Parser::new(Token::tokenize("1 - 2").unwrap());
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
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

        let mut parser = Parser::new(Token::tokenize("(6.9 - 3.17)").unwrap());
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:13);
                app!(
                    span!(1:2 - 1:7);
                    v!(span!(1:6 - 1:7); "-"),
                    f!(span!(1:2 - 1:5); 6.9)
                ),
                f!(span!(1:8 - 1:12); 3.17)
            )
        );

        let mut parser = Parser::new(Token::tokenize("(-) 4.20").unwrap());
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
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
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:3);
                v!(span!(1:1 - 1:2); "negate"),
                u!(span!(1:2 - 1:3); 1)
            )
        );

        let mut parser = Parser::new(Token::tokenize("(-1)").unwrap());
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
        assert_expr_eq!(
            expr,
            app!(
                span!(1:1 - 1:5);
                v!(span!(1:2 - 1:3); "negate"),
                u!(span!(1:3 - 1:4); 1)
            )
        );

        let mut parser = Parser::new(Token::tokenize("(- 6.9)").unwrap());
        let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
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
    fn test_projection_tuple_index_expr() {
        let expr = parse("x.0");
        let expected = Arc::new(Expr::Project(Span::default(), v!("x"), intern("0")));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_projection_expr_colon_rejected() {
        let mut parser = Parser::new(Token::tokenize("x:field").unwrap());
        assert!(parser.parse_program(&mut GasMeter::default()).is_err());
    }

    #[test]
    fn test_projection_binds_tighter_than_application() {
        let expr = parse("show p.x");
        let expected = app!(
            v!("show"),
            Arc::new(Expr::Project(Span::default(), v!("p"), intern("x")))
        );
        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_projection_can_be_applied_without_parens() {
        let expr = parse("x.field y");
        let expected = app!(
            Arc::new(Expr::Project(Span::default(), v!("x"), intern("field"))),
            v!("y")
        );
        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_list_cons_expr() {
        let expr = parse("x::xs");
        let expected = app!(app!(v!("Cons"), v!("x")), v!("xs"));
        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_list_cons_expr_right_associative() {
        let expr = parse("x::y::zs");
        let expected = app!(
            app!(v!("Cons"), v!("x")),
            app!(app!(v!("Cons"), v!("y")), v!("zs"))
        );
        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_list_cons_constructor_call_expr() {
        let expr = parse("Cons x xs");
        let expected = app!(app!(v!("Cons"), v!("x")), v!("xs"));
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
    fn test_let_rec_single_binding() {
        let expr = parse("let rec fact = \\n -> if n == 0 then 1 else n * fact (n - 1) in fact 5");
        match expr.as_ref() {
            Expr::LetRec(_, bindings, body) => {
                assert_eq!(bindings.len(), 1);
                let (name, ann, def) = &bindings[0];
                assert_eq!(name.name.as_ref(), "fact");
                assert!(ann.is_none());
                assert!(matches!(def.as_ref(), Expr::Lam(..)));
                assert_expr_eq!(body.clone(), app!(v!("fact"), u!(5)); ignore span);
            }
            other => panic!("expected let rec, got {other:?}"),
        }
    }

    #[test]
    fn test_let_rec_mutual_bindings() {
        let expr = parse("let rec even = \\n -> odd n, odd = \\n -> even n in (even 0, odd 1)");
        match expr.as_ref() {
            Expr::LetRec(_, bindings, body) => {
                assert_eq!(bindings.len(), 2);
                assert_eq!(bindings[0].0.name.as_ref(), "even");
                assert_eq!(bindings[1].0.name.as_ref(), "odd");
                assert!(matches!(bindings[0].2.as_ref(), Expr::Lam(..)));
                assert!(matches!(bindings[1].2.as_ref(), Expr::Lam(..)));
                assert!(matches!(body.as_ref(), Expr::Tuple(..)));
            }
            other => panic!("expected let rec, got {other:?}"),
        }
    }

    #[test]
    fn test_and_is_ident() {
        let expr = parse("let and = 1 in and");
        match expr.as_ref() {
            Expr::Let(_, var, _, def, body) => {
                assert_eq!(var.name.as_ref(), "and");
                assert_expr_eq!(def.clone(), u!(1); ignore span);
                assert_expr_eq!(body.clone(), v!("and"); ignore span);
            }
            other => panic!("expected let, got {other:?}"),
        }
    }

    #[test]
    fn test_let_tuple_destructuring() {
        let expr = parse("let (x, y) = (1, 2) in x");

        let pat = Pattern::Tuple(
            Span::default(),
            vec![Pattern::Var(Var::new("x")), Pattern::Var(Var::new("y"))],
        );
        let expected = Arc::new(Expr::Match(
            Span::default(),
            tup!(u!(1), u!(2)),
            vec![(pat, v!("x"))],
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
            "match list when [] -> empty when [x] -> x when [x, y, z] -> z when x::xs -> xs when _ -> fallback",
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
                Pattern::Dict(
                    Span::default(),
                    vec![
                        ("foo".into(), Pattern::Var(Var::new("foo"))),
                        ("bar".into(), Pattern::Var(Var::new("bar"))),
                    ],
                ),
                app!(v!("foo"), v!("bar")),
            )],
        ));

        assert_expr_eq!(expr, expected; ignore span);
    }

    #[test]
    fn test_match_cons_associativity() {
        let expr = parse("match xs when h::t::u -> u");
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
        let expr = parse("match xs when (_::_) -> xs");
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
    fn test_import_clause_all() {
        let code = "import foo.bar (*)\n()";
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        let Decl::Import(import) = &program.decls[0] else {
            panic!("expected import decl");
        };
        assert_eq!(import.alias, intern("bar"));
        assert!(matches!(import.clause, Some(ImportClause::All)));
    }

    #[test]
    fn test_import_clause_items_with_alias() {
        let code = "import foo.bar (x, y as z)\n()";
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        let Decl::Import(import) = &program.decls[0] else {
            panic!("expected import decl");
        };
        assert_eq!(import.alias, intern("bar"));
        let Some(ImportClause::Items(items)) = &import.clause else {
            panic!("expected import items");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, intern("x"));
        assert_eq!(items[0].alias, None);
        assert_eq!(items[1].name, intern("y"));
        assert_eq!(items[1].alias, Some(intern("z")));
    }

    #[test]
    fn test_import_clause_rejects_module_alias_combo() {
        let code = "import foo.bar (x) as Bar\n()";
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let err = parser.parse_program(&mut GasMeter::default()).unwrap_err();
        assert!(
            err[0]
                .message
                .contains("cannot combine `as <alias>` with import clause")
        );
    }

    #[test]
    fn test_import_clause_rejects_duplicate_local_names() {
        let code = "import foo.bar (x, y as x)\n()";
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        let err = parser.parse_program(&mut GasMeter::default()).unwrap_err();
        assert!(err[0].message.contains("duplicate imported name `x`"));
    }

    #[test]
    fn test_errors() {
        let mut parser = Parser::new(Token::tokenize("1 + 2 + in + 3").unwrap());
        let res = parser.parse_program(&mut GasMeter::default());
        assert_eq!(
            res,
            Err(vec![ParserErr::new(
                Span::new(1, 9, 1, 11),
                "unexpected in"
            )])
        );

        let mut parser = Parser::new(Token::tokenize("1 + 2 in + 3").unwrap());
        let res = parser.parse_program(&mut GasMeter::default());
        assert_eq!(
            res,
            Err(vec![ParserErr::new(Span::new(1, 7, 1, 9), "unexpected in")])
        );

        let mut parser = Parser::new(Token::tokenize("get 0 [    ").unwrap());
        let res = parser.parse_program(&mut GasMeter::default());
        assert_eq!(
            res,
            Err(vec![ParserErr::new(
                Span::new(1, 12, 1, 12),
                "unexpected EOF"
            )])
        );

        let mut parser = Parser::new(Token::tokenize("elem0 (  ").unwrap());
        let res = parser.parse_program(&mut GasMeter::default());
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
        let res = parser.parse_program(&mut GasMeter::default());
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
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
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
        let program = parser.parse_program(&mut GasMeter::default()).unwrap();
        assert_eq!(program.decls.len(), 2);
        assert!(matches!(program.expr.as_ref(), Expr::Bool(..)));
    }

    #[test]
    fn test_parse_top_level_hole_expr() {
        let expr = parse("?");
        assert!(matches!(expr.as_ref(), Expr::Hole(..)), "expr={expr:#?}");
    }

    #[test]
    fn test_parse_hole_in_let_with_annotation() {
        let expr = parse("let x : i32 = ? in x");
        match expr.as_ref() {
            Expr::Let(_, _var, ann, def, body) => {
                assert!(ann.is_some(), "expected annotation");
                assert!(matches!(def.as_ref(), Expr::Hole(..)), "def={def:#?}");
                assert!(matches!(body.as_ref(), Expr::Var(..)), "body={body:#?}");
            }
            other => panic!("expected let expr, got {other:#?}"),
        }
    }

    #[test]
    fn test_parse_hole_in_nested_expression_positions() {
        let expr = parse("(\\f -> f ?) (\\x -> x)");
        match expr.as_ref() {
            Expr::App(_, lhs, rhs) => {
                assert!(matches!(rhs.as_ref(), Expr::Lam(..)), "rhs={rhs:#?}");
                match lhs.as_ref() {
                    Expr::Lam(_, _, _param, _ann, _constraints, body) => match body.as_ref() {
                        Expr::App(_, f, arg) => {
                            assert!(matches!(f.as_ref(), Expr::Var(..)), "f={f:#?}");
                            assert!(matches!(arg.as_ref(), Expr::Hole(..)), "arg={arg:#?}");
                        }
                        other => panic!("expected app in lambda body, got {other:#?}"),
                    },
                    other => panic!("expected lambda lhs, got {other:#?}"),
                }
            }
            other => panic!("expected top-level app, got {other:#?}"),
        }
    }

    #[test]
    fn test_parse_hole_not_allowed_in_type_annotation_failure_case() {
        let mut parser = Parser::new(Token::tokenize("let x : ? = 1 in x").unwrap());
        let errs = parser.parse_program(&mut GasMeter::default()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("expected type") || e.message.contains("unexpected")),
            "errs={errs:#?}"
        );
    }
}
