use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    sync::Arc,
};

use rex_ast::{
    ClassDecl, ClassMethodSig, CompilationUnit, Decl, DeclareFnDecl, Expr, FnDecl, ImportClause,
    ImportDecl, ImportItem, ImportPath, InstanceDecl, InstanceMethodImpl, LetRecBinding, NameRef,
    Pattern, Scope, Symbol, TypeConstraint, TypeDecl, TypeDeclKind, TypeExpr, TypeField,
    TypeVariant, TypeVariantArg, Var,
};
use rex_ast::{Position, Span, Spanned};

use crate::{
    MAX_AST_DEPTH,
    error::ParseError,
    grammar::{Cst, CstNode, GrammarParser, TokenKind},
    lexer::{Token, Tokens},
    op::Operator,
    peg::{Failure, Input},
    rex::{self, RexRule},
};

pub(crate) struct PegParser {
    input: Input,
}

impl PegParser {
    pub(crate) fn new(tokens: Tokens) -> Self {
        Self {
            input: Input::new(tokens),
        }
    }

    pub(crate) fn parse_program(&mut self) -> Result<CompilationUnit, Vec<ParseError>> {
        let grammar = rex::rex_grammar();
        let mut engine = self.input.engine();
        let tokens = engine.tokens().to_vec();
        let eof = engine.eof_span();
        let mut parser = GrammarParser::new(&grammar, &mut engine);
        let cst = parser
            .parse_start()
            .map_err(|failure| vec![parser_error_from_failure(&failure, &tokens, eof)])?;
        drop(parser);

        AstBuilder::new().program(&cst)
    }

    #[cfg(test)]
    fn parse_pattern_for_test(&mut self) -> Result<Pattern, Vec<ParseError>> {
        let grammar = rex::rex_grammar();
        let mut engine = self.input.engine();
        let tokens = engine.tokens().to_vec();
        let eof = engine.eof_span();
        let mut parser = GrammarParser::new(&grammar, &mut engine);
        let cst = parser
            .parse_rule(RexRule::Pattern)
            .map_err(|failure| vec![parser_error_from_failure(&failure, &tokens, eof)])?;
        drop(parser);
        if !matches!(engine.current_token(), Token::Eof(..)) {
            let token = engine.current_token();
            return Err(vec![ParseError::new(
                token.span(),
                format!("unexpected {}", token),
            )]);
        }
        AstBuilder::new().pattern(&cst).map_err(|err| vec![err])
    }
}

struct AstBuilder {
    expr_depth: usize,
    type_depth: usize,
    pattern_depth: usize,
    errors: Vec<ParseError>,
}

enum GroupedApplicationStep<'cst> {
    Span(Span),
    Apply {
        terms: Vec<&'cst CstNode<RexRule>>,
        span: Span,
    },
}

impl AstBuilder {
    fn new() -> Self {
        Self {
            expr_depth: 0,
            type_depth: 0,
            pattern_depth: 0,
            errors: Vec::new(),
        }
    }

    fn program(mut self, node: &CstNode<RexRule>) -> Result<CompilationUnit, Vec<ParseError>> {
        let mut decls = Vec::new();
        for decl in child_rules(node, RexRule::Decl) {
            match self.decl(decl) {
                Ok(decl) => decls.push(decl),
                Err(err) => {
                    self.errors.push(err);
                    return Err(self.errors);
                }
            }
        }

        let body = match child_rules(node, RexRule::Expr).next() {
            Some(expr) => match self.expr(expr) {
                Ok(expr) => Some(Arc::new(expr)),
                Err(err) => {
                    self.errors.push(err);
                    return Err(self.errors);
                }
            },
            None => None,
        };

        if self.errors.is_empty() {
            Ok(CompilationUnit { decls, body })
        } else {
            Err(self.errors)
        }
    }

    fn decl(&mut self, node: &CstNode<RexRule>) -> Result<Decl, ParseError> {
        let public = first_rule(node, RexRule::PublicDecl);
        let (is_pub, body) = if let Some(public) = public {
            (true, expect_rule(public, RexRule::DeclBody)?)
        } else {
            let private = expect_rule(node, RexRule::PrivateDecl)?;
            (false, expect_rule(private, RexRule::DeclBody)?)
        };

        if let Some(decl) = first_rule(body, RexRule::ImportDecl) {
            return self.import_decl(decl, is_pub).map(Decl::Import);
        }
        if let Some(decl) = first_rule(body, RexRule::TypeDecl) {
            return self.type_decl(decl, is_pub).map(Decl::Type);
        }
        if let Some(decl) = first_rule(body, RexRule::FnDecl) {
            return self.fn_decl(decl, is_pub).map(Decl::Fn);
        }
        if let Some(decl) = first_rule(body, RexRule::DeclareFnDecl) {
            return self.declare_fn_decl(decl, is_pub).map(Decl::DeclareFn);
        }
        if let Some(decl) = first_rule(body, RexRule::ClassDecl) {
            return self.class_decl(decl, is_pub).map(Decl::Class);
        }
        if let Some(decl) = first_rule(body, RexRule::InstanceDecl) {
            return self.instance_decl(decl, is_pub).map(Decl::Instance);
        }
        Err(ParseError::new(body.span, "expected declaration"))
    }

    fn import_decl(
        &mut self,
        node: &CstNode<RexRule>,
        is_pub: bool,
    ) -> Result<ImportDecl, ParseError> {
        let path_node = expect_rule(node, RexRule::ImportPath)?;
        let (path, default_alias) = self.import_path(path_node)?;
        let clause = first_rule(node, RexRule::ImportClause)
            .map(|node| self.import_clause(node))
            .transpose()?;

        let alias = if let Some(alias_node) = first_rule(node, RexRule::ImportAlias) {
            if clause.is_some() {
                return Err(ParseError::new(
                    alias_node.span,
                    "cannot combine `as <alias>` with import clause `(...)`",
                ));
            }
            Symbol::intern(&ident_text(expect_token(alias_node, TokenKind::Ident)?)?)
        } else {
            default_alias
                .or_else(|| clause.as_ref().map(|_| Symbol::intern("_")))
                .ok_or_else(|| ParseError::new(node.span, "import requires `as <alias>`"))?
        };

        Ok(ImportDecl {
            span: node.span,
            is_pub,
            path,
            alias,
            clause,
        })
    }

    fn import_path(
        &mut self,
        node: &CstNode<RexRule>,
    ) -> Result<(ImportPath, Option<Symbol>), ParseError> {
        if let Some(dotted) = first_rule(node, RexRule::DottedImportPath) {
            let mut segments = Vec::new();
            for token in direct_tokens(dotted, TokenKind::Ident) {
                segments.push(Symbol::intern(&ident_text(token)?));
            }
            let alias = segments.last().cloned();
            return Ok((ImportPath { segments }, alias));
        }

        let relative = expect_rule(node, RexRule::RelativeImportPath)?;
        let mut segments = Vec::new();
        let prefix = expect_rule(relative, RexRule::RelativePrefix)?;
        for child in &prefix.children {
            if let Cst::Token(Token::DotDot(..)) = child {
                segments.push(Symbol::intern("super"));
            }
        }

        for child in &relative.children {
            match child {
                Cst::Token(token) if TokenKind::Ident.matches(token) => {
                    segments.push(Symbol::intern(&ident_text(token)?));
                }
                Cst::Node(segment) if segment.rule == RexRule::ImportPathSegment => {
                    let ident = expect_token(segment, TokenKind::Ident)?;
                    segments.push(Symbol::intern(&ident_text(ident)?));
                }
                _ => {}
            }
        }
        let alias = segments.last().cloned();
        Ok((ImportPath { segments }, alias))
    }

    fn import_clause(&mut self, node: &CstNode<RexRule>) -> Result<ImportClause, ParseError> {
        if direct_tokens(node, TokenKind::Mul).next().is_some() {
            return Ok(ImportClause::All);
        }

        let mut items = Vec::new();
        let mut local_names = HashSet::new();
        for item_node in child_rules(node, RexRule::ImportItem) {
            let name = self.value_name(expect_rule(item_node, RexRule::ValueName)?)?;
            let alias = direct_tokens(item_node, TokenKind::Ident)
                .last()
                .map(|token| ident_text(token).map(|name| Symbol::intern(&name)))
                .transpose()?;
            let local_name = alias.clone().unwrap_or_else(|| name.clone());
            if !local_names.insert(local_name.clone()) {
                return Err(ParseError::new(
                    item_node.span,
                    format!("duplicate imported name `{local_name}`"),
                ));
            }
            items.push(ImportItem { name, alias });
        }
        Ok(ImportClause::Items(items))
    }

    fn type_decl(&mut self, node: &CstNode<RexRule>, is_pub: bool) -> Result<TypeDecl, ParseError> {
        let name = Symbol::intern(&ident_text(expect_token(node, TokenKind::Ident)?)?);
        let params = child_rules(node, RexRule::TypeParam)
            .map(|param| {
                expect_token(param, TokenKind::Ident)
                    .and_then(ident_text)
                    .map(|name| rex_ast::TypeParam {
                        name: Symbol::intern(&name),
                        docs: None,
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let kind = if let Some(record) = first_rule(node, RexRule::TypeRecord) {
            TypeDeclKind::Alias(self.type_record(record)?)
        } else {
            TypeDeclKind::Adt(
                child_rules(node, RexRule::TypeVariant)
                    .map(|variant| self.type_variant(variant))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        };
        Ok(TypeDecl {
            span: node.span,
            is_pub,
            name,
            params,
            kind,
            docs: None,
        })
    }

    fn type_variant(&mut self, node: &CstNode<RexRule>) -> Result<TypeVariant, ParseError> {
        let name = Symbol::intern(&ident_text(expect_token(node, TokenKind::Ident)?)?);
        let args = child_rules(node, RexRule::TypeAtom)
            .map(|atom| {
                self.type_atom(atom)
                    .map(|typ| TypeVariantArg { typ, docs: None })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TypeVariant {
            name,
            args,
            docs: None,
        })
    }

    fn fn_decl(&mut self, node: &CstNode<RexRule>, is_pub: bool) -> Result<FnDecl, ParseError> {
        let name_token = expect_token(node, TokenKind::Ident)?;
        let name = Var::with_span(name_token.span(), ident_text(name_token)?);
        let type_params = self.generic_params_opt(node)?;

        if let Some(sig) = first_rule(node, RexRule::FnSignatureDecl) {
            let typ = self.type_expr(expect_rule(sig, RexRule::TypeExpr)?)?;
            let constraints = first_rule(sig, RexRule::WhereConstraints)
                .map(|node| self.where_constraints(node))
                .transpose()?
                .unwrap_or_default();
            let body = Arc::new(self.expr(expect_rule(sig, RexRule::Expr)?)?);
            let SignatureFnParts {
                params,
                ret,
                body,
                constraints: body_constraints,
            } = flatten_signature_fn(typ, body)?;
            let mut constraints = constraints;
            constraints.extend(body_constraints);
            return Ok(FnDecl {
                span: node.span,
                is_pub,
                name,
                type_params,
                params,
                ret,
                constraints,
                body,
                docs: None,
            });
        }

        let param_decl = expect_rule(node, RexRule::FnParamDecl)?;
        let params = self.fn_params(expect_rule(param_decl, RexRule::FnParams)?)?;
        let ret = self.type_expr(expect_rule(param_decl, RexRule::TypeExpr)?)?;
        let constraints = first_rule(param_decl, RexRule::WhereConstraints)
            .map(|node| self.where_constraints(node))
            .transpose()?
            .unwrap_or_default();
        let body = Arc::new(self.expr(expect_rule(param_decl, RexRule::Expr)?)?);
        Ok(FnDecl {
            span: node.span,
            is_pub,
            name,
            type_params,
            params,
            ret,
            constraints,
            body,
            docs: None,
        })
    }

    fn declare_fn_decl(
        &mut self,
        node: &CstNode<RexRule>,
        is_pub: bool,
    ) -> Result<DeclareFnDecl, ParseError> {
        let name_token = expect_token(node, TokenKind::Ident)?;
        let name = Var::with_span(name_token.span(), ident_text(name_token)?);
        let type_params = self.generic_params_opt(node)?;

        let (params, ret, constraints) =
            if let Some(param_sig) = first_rule(node, RexRule::DeclareParamSig) {
                (
                    self.fn_params(expect_rule(param_sig, RexRule::FnParams)?)?,
                    self.type_expr(expect_rule(param_sig, RexRule::TypeExpr)?)?,
                    first_rule(param_sig, RexRule::WhereConstraints)
                        .map(|node| self.where_constraints(node))
                        .transpose()?
                        .unwrap_or_default(),
                )
            } else {
                let bare = expect_rule(node, RexRule::BareFnSig)?;
                let sig = self.type_expr(expect_rule(bare, RexRule::TypeExpr)?)?;
                let constraints = first_rule(bare, RexRule::WhereConstraints)
                    .map(|node| self.where_constraints(node))
                    .transpose()?
                    .unwrap_or_default();
                let (params, ret) = flatten_decl_signature(sig);
                (params, ret, constraints)
            };

        Ok(DeclareFnDecl {
            span: node.span,
            is_pub,
            name,
            type_params,
            params,
            ret,
            constraints,
            docs: None,
        })
    }

    fn class_decl(
        &mut self,
        node: &CstNode<RexRule>,
        is_pub: bool,
    ) -> Result<ClassDecl, ParseError> {
        let name = Symbol::intern(&ident_text(expect_token(node, TokenKind::Ident)?)?);
        let params = child_rules(node, RexRule::TypeParam)
            .map(|param| {
                expect_token(param, TokenKind::Ident)
                    .and_then(ident_text)
                    .map(|name| Symbol::intern(&name))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let supers = first_rule(node, RexRule::SuperClause)
            .map(|node| self.type_constraints(expect_rule(node, RexRule::TypeConstraints)?))
            .transpose()?
            .unwrap_or_default();
        let methods = first_rule(node, RexRule::ClassBlock)
            .map(|block| {
                child_rules(block, RexRule::ClassMethod)
                    .map(|method| {
                        Ok(ClassMethodSig {
                            name: self.value_name(expect_rule(method, RexRule::ValueName)?)?,
                            type_params: self.generic_params_opt(method)?,
                            typ: self.type_expr(expect_rule(method, RexRule::TypeExpr)?)?,
                            docs: None,
                        })
                    })
                    .collect::<Result<Vec<_>, ParseError>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(ClassDecl {
            span: node.span,
            is_pub,
            name,
            params,
            supers,
            methods,
            docs: None,
        })
    }

    fn instance_decl(
        &mut self,
        node: &CstNode<RexRule>,
        is_pub: bool,
    ) -> Result<InstanceDecl, ParseError> {
        let class = self
            .name_ref(expect_rule(node, RexRule::NameRef)?)?
            .to_dotted_symbol();
        let type_params = self.generic_params_opt(node)?;
        let head = self.type_app(expect_rule(node, RexRule::TypeApp)?)?;
        let context = first_rule(node, RexRule::InstanceContext)
            .map(|node| self.type_constraints(expect_rule(node, RexRule::TypeConstraints)?))
            .transpose()?
            .unwrap_or_default();
        let methods = first_rule(node, RexRule::InstanceBlock)
            .map(|block| {
                child_rules(block, RexRule::InstanceMethod)
                    .map(|method| {
                        Ok(InstanceMethodImpl {
                            name: self.value_name(expect_rule(method, RexRule::ValueName)?)?,
                            type_params: self.generic_params_opt(method)?,
                            ann: first_rule(method, RexRule::TypeExpr)
                                .map(|ann| self.type_expr(ann))
                                .transpose()?,
                            body: Arc::new(self.expr(expect_rule(method, RexRule::Expr)?)?),
                        })
                    })
                    .collect::<Result<Vec<_>, ParseError>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(InstanceDecl {
            span: node.span,
            is_pub,
            type_params,
            class,
            head,
            context,
            methods,
            docs: None,
        })
    }

    fn generic_params_opt(&mut self, node: &CstNode<RexRule>) -> Result<Vec<Symbol>, ParseError> {
        let Some(params) = first_rule(node, RexRule::GenericParams) else {
            return Ok(Vec::new());
        };
        direct_tokens(params, TokenKind::Ident)
            .map(|token| ident_text(token).map(|name| Symbol::intern(&name)))
            .collect()
    }

    fn fn_params(&mut self, node: &CstNode<RexRule>) -> Result<Vec<(Var, TypeExpr)>, ParseError> {
        if let Some(group) = first_rule(node, RexRule::LegacyParamGroup) {
            return child_rules(group, RexRule::LegacyParam)
                .map(|param| self.legacy_param(param))
                .collect();
        }

        child_rules(node, RexRule::ArrowParam)
            .map(|param| {
                if let Some(paren) = first_rule(param, RexRule::ParenParam) {
                    return self.paren_param(paren);
                }
                self.named_param(expect_rule(param, RexRule::NamedParam)?)
            })
            .collect()
    }

    fn named_param(&mut self, node: &CstNode<RexRule>) -> Result<(Var, TypeExpr), ParseError> {
        let token = expect_token(node, TokenKind::Ident)?;
        let var = Var::with_span(token.span(), ident_text(token)?);
        let ann = self.type_app(expect_rule(node, RexRule::TypeApp)?)?;
        Ok((var, ann))
    }

    fn paren_param(&mut self, node: &CstNode<RexRule>) -> Result<(Var, TypeExpr), ParseError> {
        let token = expect_token(node, TokenKind::Ident)?;
        let var = Var::with_span(token.span(), ident_text(token)?);
        let ann = self.type_expr(expect_rule(node, RexRule::TypeExpr)?)?;
        Ok((var, ann))
    }

    fn legacy_param(&mut self, node: &CstNode<RexRule>) -> Result<(Var, TypeExpr), ParseError> {
        let token = expect_token(node, TokenKind::Ident)?;
        let var = Var::with_span(token.span(), ident_text(token)?);
        let ann = self.type_expr(expect_rule(node, RexRule::TypeExpr)?)?;
        Ok((var, ann))
    }

    fn where_constraints(
        &mut self,
        node: &CstNode<RexRule>,
    ) -> Result<Vec<TypeConstraint>, ParseError> {
        self.type_constraints(expect_rule(node, RexRule::TypeConstraints)?)
    }

    fn type_constraints(
        &mut self,
        node: &CstNode<RexRule>,
    ) -> Result<Vec<TypeConstraint>, ParseError> {
        child_rules(node, RexRule::TypeConstraint)
            .map(|constraint| {
                Ok(TypeConstraint::new(
                    self.name_ref(expect_rule(constraint, RexRule::NameRef)?)?,
                    self.type_app(expect_rule(constraint, RexRule::TypeApp)?)?,
                ))
            })
            .collect()
    }

    fn type_expr(&mut self, node: &CstNode<RexRule>) -> Result<TypeExpr, ParseError> {
        self.type_fun(expect_rule(node, RexRule::TypeFun)?)
    }

    fn type_fun(&mut self, node: &CstNode<RexRule>) -> Result<TypeExpr, ParseError> {
        self.check_type_depth(node.span)?;
        self.type_depth += 1;
        let lhs = self.type_app(expect_rule(node, RexRule::TypeApp)?)?;
        let result = if let Some(rhs) = first_rule(node, RexRule::TypeFun) {
            let rhs = self.type_fun(rhs)?;
            let span = Span::from_begin_end(lhs.span().begin, rhs.span().end);
            Ok(TypeExpr::Fun(span, Box::new(lhs), Box::new(rhs)))
        } else {
            Ok(lhs)
        };
        self.type_depth = self.type_depth.saturating_sub(1);
        result
    }

    fn type_app(&mut self, node: &CstNode<RexRule>) -> Result<TypeExpr, ParseError> {
        let mut atoms = child_rules(node, RexRule::TypeAtom);
        let Some(first) = atoms.next() else {
            return Err(ParseError::new(node.span, "expected type"));
        };
        let mut lhs = self.type_atom(first)?;
        for atom in atoms {
            let rhs = self.type_atom(atom)?;
            let span = Span::from_begin_end(lhs.span().begin, rhs.span().end);
            lhs = TypeExpr::App(span, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn type_atom(&mut self, node: &CstNode<RexRule>) -> Result<TypeExpr, ParseError> {
        if let Some(name) = first_rule(node, RexRule::NameRef) {
            return Ok(TypeExpr::Name(name.span, self.name_ref(name)?));
        }
        if let Some(paren) = first_rule(node, RexRule::TypeParen) {
            return self.type_paren(paren);
        }
        self.type_record(expect_rule(node, RexRule::TypeRecord)?)
    }

    fn type_paren(&mut self, node: &CstNode<RexRule>) -> Result<TypeExpr, ParseError> {
        if first_rule(node, RexRule::UnitType).is_some() {
            return Ok(TypeExpr::Tuple(node.span, Vec::new()));
        }
        if let Some(tuple) = first_rule(node, RexRule::TupleType) {
            let elems = child_rules(tuple, RexRule::TypeExpr)
                .map(|elem| self.type_expr(elem))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(TypeExpr::Tuple(tuple.span, elems));
        }
        let grouped = expect_rule(node, RexRule::GroupedType)?;
        self.type_expr(expect_rule(grouped, RexRule::TypeExpr)?)
    }

    fn type_record(&mut self, node: &CstNode<RexRule>) -> Result<TypeExpr, ParseError> {
        let fields = child_rules(node, RexRule::TypeField)
            .map(|field| {
                Ok(TypeField {
                    name: Symbol::intern(&ident_text(expect_token(field, TokenKind::Ident)?)?),
                    typ: self.type_expr(expect_rule(field, RexRule::TypeExpr)?)?,
                    docs: None,
                })
            })
            .collect::<Result<Vec<_>, ParseError>>()?;
        Ok(TypeExpr::Record(node.span, fields))
    }

    fn expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        self.check_expr_depth(node.span)?;
        self.expr_depth += 1;

        if let Some(expr) = self.try_grouped_tail_application_expr(node)? {
            self.expr_depth = self.expr_depth.saturating_sub(1);
            return Ok(expr);
        }

        let mut operands = Vec::new();
        let mut operators = Vec::new();
        for child in &node.children {
            match child {
                Cst::Node(child) if child.rule == RexRule::UnaryExpr => {
                    operands.push(self.unary_expr(child)?);
                }
                Cst::Node(child) if child.rule == RexRule::BinaryOp => {
                    operators.push(expect_binary_operator(child)?.clone());
                }
                _ => {}
            }
        }

        let result = if operands.is_empty() {
            Err(ParseError::new(node.span, "expected expression"))
        } else if operators.len() >= MAX_AST_DEPTH {
            Err(ParseError::new(
                node.span,
                format!("maximum AST depth exceeded (max {MAX_AST_DEPTH})"),
            ))
        } else {
            Ok(fold_binary_expr(operands, operators))
        };

        self.expr_depth = self.expr_depth.saturating_sub(1);
        result
    }

    fn try_grouped_tail_application_expr(
        &mut self,
        node: &CstNode<RexRule>,
    ) -> Result<Option<Expr>, ParseError> {
        let mut current = node;
        let mut steps = Vec::new();
        let mut peeled = 0usize;

        while let Some((prefix, tail)) = grouped_tail_application(current) {
            peeled += 1;
            if self.expr_depth + peeled > MAX_AST_DEPTH {
                return Err(ParseError::new(
                    current.span,
                    format!("maximum AST depth exceeded (max {MAX_AST_DEPTH})"),
                ));
            }

            if prefix.is_empty() {
                steps.push(GroupedApplicationStep::Span(current.span));
            } else {
                steps.push(GroupedApplicationStep::Apply {
                    terms: prefix,
                    span: current.span,
                });
            }
            current = tail;
        }

        if peeled == 0 {
            return Ok(None);
        }

        let mut expr = self.expr(current)?;
        for step in steps.into_iter().rev() {
            match step {
                GroupedApplicationStep::Span(span) => {
                    expr = expr.with_span(span);
                }
                GroupedApplicationStep::Apply { terms, span } => {
                    expr = self.apply_application_group(terms, expr)?.with_span(span);
                }
            }
        }
        Ok(Some(expr))
    }

    fn apply_application_group(
        &mut self,
        terms: Vec<&CstNode<RexRule>>,
        tail: Expr,
    ) -> Result<Expr, ParseError> {
        let mut terms = terms.into_iter();
        let Some(first) = terms.next() else {
            return Ok(tail);
        };

        let mut base = self.postfix_expr(first)?;
        let begin = base.span().begin;
        for term in terms {
            let arg = self.postfix_expr(term)?;
            let span = Span::from_begin_end(begin, arg.span().end);
            base = Expr::App(span, Arc::new(base), Arc::new(arg));
        }

        let span = Span::from_begin_end(begin, tail.span().end);
        Ok(Expr::App(span, Arc::new(base), Arc::new(tail)))
    }

    fn unary_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        let mut expr = self.application_expr(expect_rule(node, RexRule::ApplicationExpr)?)?;
        for ann in child_rules(node, RexRule::TypeExpr) {
            let ann = self.type_expr(ann)?;
            let span = Span::from_begin_end(expr.span().begin, ann.span().end);
            expr = Expr::Ann(span, Arc::new(expr), ann);
        }
        Ok(expr)
    }

    fn application_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        let mut terms = child_rules(node, RexRule::PostfixExpr);
        let Some(first) = terms.next() else {
            return Err(ParseError::new(node.span, "expected expression"));
        };
        let mut base = self.postfix_expr(first)?;
        let begin = base.span().begin;
        for term in terms {
            let arg = self.postfix_expr(term)?;
            let span = Span::from_begin_end(begin, arg.span().end);
            base = Expr::App(span, Arc::new(base), Arc::new(arg));
        }
        Ok(base)
    }

    fn postfix_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        let mut base = self.atom_expr(expect_rule(node, RexRule::AtomExpr)?)?;
        for field in child_rules(node, RexRule::FieldName) {
            let (name, end) = self.field_name(field)?;
            let span = Span::from_begin_end(base.span().begin, end);
            base = Expr::Project(span, Arc::new(base), name);
        }
        Ok(base)
    }

    fn field_name(&mut self, node: &CstNode<RexRule>) -> Result<(Symbol, Position), ParseError> {
        let token = first_token(node).ok_or_else(|| internal_err(node.span, "expected field"))?;
        match token {
            Token::Ident(name, span, ..) => Ok((Symbol::intern(name), span.end)),
            Token::Int(value, span) => Ok((Symbol::intern(&value.to_string()), span.end)),
            _ => Err(ParseError::new(
                token.span(),
                "expected field name after `.`",
            )),
        }
    }

    fn atom_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        if let Some(token) = first_token(node) {
            match token {
                Token::Bool(val, span, ..) => return Ok(Expr::Bool(*span, *val)),
                Token::Char(val, span, ..) => return Ok(Expr::Char(*span, *val)),
                Token::Float(val, span, ..) => return Ok(Expr::Float(*span, *val)),
                Token::Int(val, span, ..) => return Ok(Expr::Uint(*span, *val)),
                Token::String(val, span, ..) => return Ok(Expr::String(*span, val.clone())),
                _ => {}
            }
        }

        if let Some(paren) = first_rule(node, RexRule::ParenExpr) {
            return self.paren_expr(paren);
        }
        if let Some(list) = first_rule(node, RexRule::ListExpr) {
            return self.list_expr(list);
        }
        if let Some(brace) = first_rule(node, RexRule::BraceExpr) {
            return self.brace_expr(brace);
        }
        if let Some(hole) = first_rule(node, RexRule::HoleExpr) {
            return Ok(Expr::Hole(hole.span));
        }
        if let Some(ident) = first_rule(node, RexRule::IdentExpr) {
            let token = expect_token(ident, TokenKind::Ident)?;
            return Ok(Expr::Var(Var::with_span(token.span(), ident_text(token)?)));
        }
        if let Some(lambda) = first_rule(node, RexRule::LambdaExpr) {
            return self.lambda_expr(lambda);
        }
        if let Some(let_expr) = first_rule(node, RexRule::LetExpr) {
            return self.let_expr(let_expr);
        }
        if let Some(if_expr) = first_rule(node, RexRule::IfExpr) {
            return self.if_expr(if_expr);
        }
        if let Some(match_expr) = first_rule(node, RexRule::MatchExpr) {
            return self.match_expr(match_expr);
        }
        self.neg_expr(expect_rule(node, RexRule::NegExpr)?)
    }

    fn paren_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        if first_rule(node, RexRule::UnitExpr).is_some() {
            return Ok(Expr::Tuple(node.span, Vec::new()));
        }
        if let Some(operator) = first_rule(node, RexRule::OperatorNameExpr) {
            let token = direct_tokens(operator, TokenKind::ValueOperator)
                .next()
                .ok_or_else(|| internal_err(operator.span, "expected parenthesized operator"))?;
            let name = operator_token_name(token)
                .ok_or_else(|| internal_err(token.span(), "expected operator"))?;
            return Ok(Expr::Var(Var::with_span(operator.span, name)));
        }
        if let Some(tuple) = first_rule(node, RexRule::TupleExpr) {
            let items = child_rules(tuple, RexRule::Expr)
                .map(|expr| self.expr(expr).map(Arc::new))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Expr::Tuple(tuple.span, items));
        }
        let grouped = expect_rule(node, RexRule::GroupedExpr)?;
        Ok(self
            .expr(expect_rule(grouped, RexRule::Expr)?)?
            .with_span(grouped.span))
    }

    fn list_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        let items = child_rules(node, RexRule::Expr)
            .map(|expr| self.expr(expr).map(Arc::new))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Expr::List(node.span, items))
    }

    fn brace_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        if let Some(dict) = first_rule(node, RexRule::DictExpr) {
            return self.dict_expr(dict);
        }
        self.record_update_expr(expect_rule(node, RexRule::RecordUpdateExpr)?)
    }

    fn dict_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        let mut items = BTreeMap::new();
        for (idx, child) in node.children.iter().enumerate() {
            match child {
                Cst::Node(item) if item.rule == RexRule::DictItem => {
                    let key = Symbol::intern(&ident_text(expect_token(item, TokenKind::Ident)?)?);
                    let value = Arc::new(self.expr(expect_rule(item, RexRule::Expr)?)?);
                    items.insert(key, value);
                }
                Cst::Node(item) if item.rule == RexRule::BadDictItem => {
                    let span = next_token_after(node, idx)
                        .map(|token| token.span())
                        .unwrap_or(item.span);
                    self.errors.push(ParseError::new(span, "expected `=`"));
                }
                _ => {}
            }
        }
        Ok(Expr::Dict(node.span, items))
    }

    fn record_update_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        let base = self.expr(expect_rule(node, RexRule::Expr)?)?;
        let dict = self.dict_expr(expect_rule(node, RexRule::DictExpr)?)?;
        let updates = match &dict {
            Expr::Dict(_, updates) => updates.clone(),
            _ => BTreeMap::new(),
        };
        Ok(Expr::RecordUpdate(node.span, Arc::new(base), updates))
    }

    fn neg_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        let op = expect_token(node, TokenKind::Sub)?;
        let expr = self.expr(expect_rule(node, RexRule::Expr)?)?;
        Ok(Expr::App(
            node.span,
            Arc::new(Expr::Var(Var::with_span(op.span(), "negate"))),
            Arc::new(expr),
        ))
    }

    fn lambda_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        let mut params = VecDeque::new();
        for param in child_rules(node, RexRule::LambdaParam) {
            params.push_back(self.lambda_param(param)?);
        }
        let mut constraints = first_rule(node, RexRule::WhereConstraints)
            .map(|node| self.where_constraints(node))
            .transpose()?
            .unwrap_or_default();
        let body_node = child_rules(node, RexRule::Expr)
            .last()
            .ok_or_else(|| internal_err(node.span, "expected lambda body"))?;
        let mut body = self.expr(body_node)?;
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
        Ok(body.with_span_begin(node.span.begin))
    }

    fn lambda_param(
        &mut self,
        node: &CstNode<RexRule>,
    ) -> Result<(Span, Var, Option<TypeExpr>), ParseError> {
        let token = expect_token(node, TokenKind::Ident)?;
        let var = Var::with_span(token.span(), ident_text(token)?);
        let ann = first_rule(node, RexRule::TypeExpr)
            .map(|node| self.type_expr(node))
            .transpose()?;
        Ok((node.span, var, ann))
    }

    fn let_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        let body_node = child_rules(node, RexRule::Expr)
            .last()
            .ok_or_else(|| internal_err(node.span, "expected let body"))?;
        let body = self.expr(body_node)?;
        if direct_tokens(node, TokenKind::Rec).next().is_some() {
            let bindings = child_rules(node, RexRule::LetRecBinding)
                .map(|binding| self.let_rec_binding(binding))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Expr::LetRec(node.span, bindings, Arc::new(body)));
        }

        let mut decls = VecDeque::new();
        for binding in child_rules(node, RexRule::LetBinding) {
            decls.push_back(self.let_binding(binding)?);
        }

        let mut body = body;
        let mut body_span_end = body.span().end;
        while let Some((pat, type_params, ann, def)) = decls.pop_back() {
            match pat {
                Pattern::Var(var) => {
                    body = Expr::Let(
                        Span::from_begin_end(var.span.begin, body_span_end),
                        var,
                        type_params,
                        ann,
                        Arc::new(def),
                        Arc::new(body),
                    );
                }
                pat => {
                    if !type_params.is_empty() {
                        return Err(ParseError::new(
                            *pat.span(),
                            "type parameters require a named let binding",
                        ));
                    }
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
        Ok(body.with_span_begin(node.span.begin))
    }

    fn let_binding(
        &mut self,
        node: &CstNode<RexRule>,
    ) -> Result<(Pattern, Vec<Symbol>, Option<TypeExpr>, Expr), ParseError> {
        let mut pat = self.pattern(expect_rule(node, RexRule::Pattern)?)?;
        let type_params = self.generic_params_opt(node)?;
        let ann = first_rule(node, RexRule::TypeExpr)
            .map(|node| self.type_expr(node))
            .transpose()?;
        if ann.is_some()
            && let Some(var) = pattern_binding_var(&pat)
        {
            pat = Pattern::Var(var);
        }
        let expr = self.expr(expect_rule(node, RexRule::Expr)?)?;
        Ok((pat, type_params, ann, expr))
    }

    fn let_rec_binding(&mut self, node: &CstNode<RexRule>) -> Result<LetRecBinding, ParseError> {
        let pat = self.pattern(expect_rule(node, RexRule::Pattern)?)?;
        let Some(var) = pattern_binding_var(&pat) else {
            return Err(ParseError::new(
                *pat.span(),
                "let rec only supports variable bindings",
            ));
        };
        let type_params = self.generic_params_opt(node)?;
        let ann = first_rule(node, RexRule::TypeExpr)
            .map(|node| self.type_expr(node))
            .transpose()?;
        let expr = Arc::new(self.expr(expect_rule(node, RexRule::Expr)?)?);
        Ok((var, type_params, ann, expr))
    }

    fn if_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        let exprs = child_rules(node, RexRule::Expr).collect::<Vec<_>>();
        if exprs.len() != 3 {
            return Err(internal_err(node.span, "expected if expression parts"));
        }
        let cond = self.expr(exprs[0])?;
        let then = self.expr(exprs[1])?;
        let r#else = self.expr(exprs[2])?;
        Ok(Expr::Ite(
            node.span,
            Arc::new(cond),
            Arc::new(then),
            Arc::new(r#else),
        ))
    }

    fn match_expr(&mut self, node: &CstNode<RexRule>) -> Result<Expr, ParseError> {
        let scrutinee = self.expr(expect_rule(node, RexRule::Expr)?)?;
        let arms = child_rules(node, RexRule::MatchArm)
            .map(|arm| {
                Ok((
                    self.pattern(expect_rule(arm, RexRule::Pattern)?)?,
                    Arc::new(self.expr(expect_rule(arm, RexRule::Expr)?)?),
                ))
            })
            .collect::<Result<Vec<_>, ParseError>>()?;
        Ok(Expr::Match(node.span, Arc::new(scrutinee), arms))
    }

    fn pattern(&mut self, node: &CstNode<RexRule>) -> Result<Pattern, ParseError> {
        self.check_pattern_depth(node.span)?;
        self.pattern_depth += 1;
        let lhs = self.app_pattern(expect_rule(node, RexRule::AppPattern)?);
        let result = match lhs {
            Ok(lhs) => {
                if let Some(rhs) = child_rules(node, RexRule::Pattern).next() {
                    let rhs = self.pattern(rhs)?;
                    let span = Span::from_begin_end(lhs.span().begin, rhs.span().end);
                    Ok(Pattern::Cons(span, Box::new(lhs), Box::new(rhs)))
                } else {
                    Ok(lhs)
                }
            }
            Err(err) => Err(err),
        };
        self.pattern_depth = self.pattern_depth.saturating_sub(1);
        result
    }

    fn app_pattern(&mut self, node: &CstNode<RexRule>) -> Result<Pattern, ParseError> {
        if let Some(name_node) = first_rule(node, RexRule::NameRef) {
            let name = self.name_ref(name_node)?;
            let args = child_rules(node, RexRule::PatternAtom)
                .map(|arg| self.pattern_atom(arg))
                .collect::<Result<Vec<_>, _>>()?;
            return self.named_or_var_pattern(name_node.span, name, args);
        }
        self.pattern_atom(expect_rule(node, RexRule::PatternAtom)?)
    }

    fn named_or_var_pattern(
        &mut self,
        span: Span,
        name: NameRef,
        args: Vec<Pattern>,
    ) -> Result<Pattern, ParseError> {
        if name.as_ref() == "_" {
            if args.is_empty() {
                return Ok(Pattern::Wildcard(span));
            }
            let span = args.first().map(|arg| *arg.span()).unwrap_or(span);
            return Err(ParseError::new(
                span,
                "constructor patterns must start with an identifier",
            ));
        }

        if args.is_empty()
            && matches!(&name, NameRef::Unqualified(sym) if !is_uppercase_symbol(sym))
        {
            return Ok(Pattern::Var(Var {
                span,
                name: name.to_dotted_symbol(),
            }));
        }

        let end = args.last().map(|arg| arg.span().end).unwrap_or(span.end);
        Ok(Pattern::Named(
            Span::from_begin_end(span.begin, end),
            name,
            args,
        ))
    }

    fn pattern_atom(&mut self, node: &CstNode<RexRule>) -> Result<Pattern, ParseError> {
        if let Some(token) = first_token(node)
            && let Token::Ident(name, span, ..) = token
        {
            let name_ref = NameRef::Unqualified(Symbol::intern(name));
            return self.named_or_var_pattern(*span, name_ref, Vec::new());
        }
        if let Some(list) = first_rule(node, RexRule::ListPattern) {
            let elems = child_rules(list, RexRule::Pattern)
                .map(|pat| self.pattern(pat))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Pattern::List(list.span, elems));
        }
        if let Some(dict) = first_rule(node, RexRule::DictPattern) {
            let fields = child_rules(dict, RexRule::DictPatternField)
                .map(|field| self.dict_pattern_field(field))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Pattern::Dict(dict.span, fields));
        }
        self.paren_pattern(expect_rule(node, RexRule::ParenPattern)?)
    }

    fn dict_pattern_field(
        &mut self,
        node: &CstNode<RexRule>,
    ) -> Result<(Symbol, Pattern), ParseError> {
        let token = expect_token(node, TokenKind::Ident)?;
        let name = ident_text(token)?;
        let key = Symbol::intern(&name);
        let pat = first_rule(node, RexRule::Pattern)
            .map(|node| self.pattern(node))
            .transpose()?
            .unwrap_or_else(|| Pattern::Var(Var::with_span(token.span(), name)));
        Ok((key, pat))
    }

    fn paren_pattern(&mut self, node: &CstNode<RexRule>) -> Result<Pattern, ParseError> {
        let patterns = child_rules(node, RexRule::Pattern).collect::<Vec<_>>();
        match patterns.as_slice() {
            [] => Ok(Pattern::Tuple(node.span, Vec::new())),
            [only] => Ok(self.pattern(only)?.with_span(node.span)),
            many => many
                .iter()
                .map(|pat| self.pattern(pat))
                .collect::<Result<Vec<_>, _>>()
                .map(|items| Pattern::Tuple(node.span, items)),
        }
    }

    fn name_ref(&mut self, node: &CstNode<RexRule>) -> Result<NameRef, ParseError> {
        let segments = direct_tokens(node, TokenKind::Ident)
            .map(|token| ident_text(token).map(|name| Symbol::intern(&name)))
            .collect::<Result<Vec<_>, _>>()?;
        if segments.is_empty() {
            Err(ParseError::new(node.span, "expected identifier"))
        } else {
            Ok(NameRef::from_segments(segments))
        }
    }

    fn value_name(&mut self, node: &CstNode<RexRule>) -> Result<Symbol, ParseError> {
        let token = first_token(node).ok_or_else(|| ParseError::new(node.span, "expected name"))?;
        if let Ok(name) = ident_text(token) {
            return Ok(Symbol::intern(&name));
        }
        let name = operator_token_name(token)
            .ok_or_else(|| ParseError::new(token.span(), "expected name"))?;
        Ok(Symbol::intern(name))
    }

    fn check_expr_depth(&self, span: Span) -> Result<(), ParseError> {
        if self.expr_depth >= MAX_AST_DEPTH {
            return Err(ParseError::new(
                span,
                format!("maximum AST depth exceeded (max {MAX_AST_DEPTH})"),
            ));
        }
        Ok(())
    }

    fn check_type_depth(&self, span: Span) -> Result<(), ParseError> {
        if self.type_depth >= MAX_AST_DEPTH {
            return Err(ParseError::new(
                span,
                format!("maximum AST depth exceeded (max {MAX_AST_DEPTH})"),
            ));
        }
        Ok(())
    }

    fn check_pattern_depth(&self, span: Span) -> Result<(), ParseError> {
        if self.pattern_depth >= MAX_AST_DEPTH {
            return Err(ParseError::new(
                span,
                format!("maximum AST depth exceeded (max {MAX_AST_DEPTH})"),
            ));
        }
        Ok(())
    }
}

fn fold_binary_expr(mut operands: Vec<Expr>, operators: Vec<Token>) -> Expr {
    let lhs = operands.remove(0);
    let parts = operators.into_iter().zip(operands).collect::<Vec<_>>();
    let mut index = 0;
    fold_binary_from(lhs, &parts, &mut index)
}

fn fold_binary_from(lhs: Expr, parts: &[(Token, Expr)], index: &mut usize) -> Expr {
    if *index >= parts.len() {
        return lhs;
    }

    let (operator, rhs) = &parts[*index];
    *index += 1;
    let mut rhs = rhs.clone();
    let precedence = operator.precedence();

    let next_binary_expr_takes_precedence = parts.get(*index).is_some_and(|(next, _)| {
        if precedence > next.precedence() {
            false
        } else if precedence == next.precedence() {
            !is_left_associative_binary(operator)
        } else {
            true
        }
    });

    if next_binary_expr_takes_precedence {
        rhs = fold_binary_from(rhs, parts, index);
    }

    let combined = make_binary_expr(lhs, operator, rhs);
    fold_binary_from(combined, parts, index)
}

fn make_binary_expr(lhs: Expr, operator: &Token, rhs: Expr) -> Expr {
    let lhs_span = *lhs.span();
    let rhs_end = rhs.span().end;
    let op_span = operator.span();
    if matches!(operator, Token::ColonColon(..)) {
        let cons_span = Span::from_begin_end(lhs_span.begin, op_span.end);
        let outer_span = Span::from_begin_end(lhs_span.begin, rhs_end);
        return Expr::App(
            outer_span,
            Arc::new(Expr::App(
                cons_span,
                Arc::new(Expr::Var(Var::with_span(op_span, "Cons"))),
                Arc::new(lhs),
            )),
            Arc::new(rhs),
        );
    }

    let name = match operator {
        Token::Add(..) => Operator::Add.to_string(),
        Token::And(..) => Operator::And.to_string(),
        Token::Div(..) => Operator::Div.to_string(),
        Token::Eq(..) => Operator::Eq.to_string(),
        Token::Ne(..) => Operator::Ne.to_string(),
        Token::Ge(..) => Operator::Ge.to_string(),
        Token::Gt(..) => Operator::Gt.to_string(),
        Token::Le(..) => Operator::Le.to_string(),
        Token::Lt(..) => Operator::Lt.to_string(),
        Token::Mod(..) => Operator::Mod.to_string(),
        Token::Mul(..) => Operator::Mul.to_string(),
        Token::Or(..) => Operator::Or.to_string(),
        Token::Sub(..) => Operator::Sub.to_string(),
        _ => operator.to_string(),
    };
    let inner_span = Span::from_begin_end(lhs_span.begin, op_span.end);
    let outer_span = Span::from_begin_end(lhs_span.begin, rhs_end);
    Expr::App(
        outer_span,
        Arc::new(Expr::App(
            inner_span,
            Arc::new(Expr::Var(Var::with_span(op_span, name))),
            Arc::new(lhs),
        )),
        Arc::new(rhs),
    )
}

fn expect_binary_operator(node: &CstNode<RexRule>) -> Result<&Token, ParseError> {
    first_token(node)
        .filter(|token| TokenKind::BinaryOperator.matches(token))
        .ok_or_else(|| ParseError::new(node.span, "expected binary operator"))
}

fn parser_error_from_failure(failure: &Failure, tokens: &[Token], eof: Span) -> ParseError {
    let token = tokens
        .get(failure.pos.0)
        .cloned()
        .unwrap_or(Token::Eof(eof));
    let message = if let [expected] = failure.expected.iter().collect::<Vec<_>>().as_slice()
        && expected.starts_with("expected")
    {
        (*expected).clone()
    } else if matches!(token, Token::Eof(..)) {
        "unexpected EOF".to_string()
    } else {
        format!("unexpected {}", token)
    };
    ParseError::new(failure.span, message)
}

fn child_rules(
    node: &CstNode<RexRule>,
    rule: RexRule,
) -> impl DoubleEndedIterator<Item = &CstNode<RexRule>> {
    node.children.iter().filter_map(move |child| match child {
        Cst::Node(node) if node.rule == rule => Some(node.as_ref()),
        _ => None,
    })
}

fn first_rule(node: &CstNode<RexRule>, rule: RexRule) -> Option<&CstNode<RexRule>> {
    child_rules(node, rule).next()
}

fn grouped_tail_application(
    node: &CstNode<RexRule>,
) -> Option<(Vec<&CstNode<RexRule>>, &CstNode<RexRule>)> {
    if child_rules(node, RexRule::BinaryOp).next().is_some() {
        return None;
    }
    let unary = only_rule(node, RexRule::UnaryExpr)?;
    if child_rules(unary, RexRule::TypeExpr).next().is_some() {
        return None;
    }
    let application = only_rule(unary, RexRule::ApplicationExpr)?;
    let terms = child_rules(application, RexRule::PostfixExpr).collect::<Vec<_>>();
    let last = terms.last()?;
    let tail = grouped_expr_tail(last)?;
    Some((terms[..terms.len() - 1].to_vec(), tail))
}

fn grouped_expr_tail(node: &CstNode<RexRule>) -> Option<&CstNode<RexRule>> {
    if child_rules(node, RexRule::FieldName).next().is_some() {
        return None;
    }
    let atom = only_rule(node, RexRule::AtomExpr)?;
    let paren = only_rule(atom, RexRule::ParenExpr)?;
    let grouped = only_rule(paren, RexRule::GroupedExpr)?;
    only_rule(grouped, RexRule::Expr)
}

fn only_rule(node: &CstNode<RexRule>, rule: RexRule) -> Option<&CstNode<RexRule>> {
    let mut matches = child_rules(node, rule);
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn expect_rule(node: &CstNode<RexRule>, rule: RexRule) -> Result<&CstNode<RexRule>, ParseError> {
    first_rule(node, rule).ok_or_else(|| internal_err(node.span, "expected grammar node"))
}

fn direct_tokens(
    node: &CstNode<RexRule>,
    kind: TokenKind,
) -> impl DoubleEndedIterator<Item = &Token> {
    node.children.iter().filter_map(move |child| match child {
        Cst::Token(token) if kind.matches(token) => Some(token),
        _ => None,
    })
}

fn first_token(node: &CstNode<RexRule>) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        Cst::Token(token) => Some(token),
        _ => None,
    })
}

fn expect_token(node: &CstNode<RexRule>, kind: TokenKind) -> Result<&Token, ParseError> {
    direct_tokens(node, kind)
        .next()
        .ok_or_else(|| internal_err(node.span, "expected token"))
}

fn next_token_after(node: &CstNode<RexRule>, index: usize) -> Option<&Token> {
    node.children
        .iter()
        .skip(index + 1)
        .find_map(|child| match child {
            Cst::Token(token) => Some(token),
            _ => None,
        })
}

fn ident_text(token: &Token) -> Result<String, ParseError> {
    match token {
        Token::Ident(name, ..) => Ok(name.clone()),
        _ => Err(ParseError::new(token.span(), "expected identifier")),
    }
}

fn internal_err(span: Span, message: &'static str) -> ParseError {
    ParseError::new(span, format!("internal parser error: {message}"))
}

fn operator_token_name(token: &Token) -> Option<&'static str> {
    match token {
        Token::Add(..) => Some("+"),
        Token::And(..) => Some("&&"),
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

fn is_left_associative_binary(token: &Token) -> bool {
    matches!(
        token,
        Token::Add(..)
            | Token::And(..)
            | Token::Div(..)
            | Token::Mul(..)
            | Token::Mod(..)
            | Token::Or(..)
            | Token::Sub(..)
    )
}

fn is_uppercase_symbol(symbol: &Symbol) -> bool {
    symbol
        .as_ref()
        .chars()
        .next()
        .map(|ch| ch.is_uppercase())
        .unwrap_or(false)
}

struct SignatureFnParts {
    params: Vec<(Var, TypeExpr)>,
    ret: TypeExpr,
    body: Arc<Expr>,
    constraints: Vec<TypeConstraint>,
}

fn flatten_signature_fn(sig: TypeExpr, body: Arc<Expr>) -> Result<SignatureFnParts, ParseError> {
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
        return Err(ParseError::new(
            *ret.span(),
            "expected function type after `:`; use `let` for values",
        ));
    }

    let arity = param_tys.len();
    let mut body_constraints = Vec::new();
    if matches!(body.as_ref(), Expr::Lam(..)) {
        let mut lam_params = Vec::new();
        let mut cur = body.clone();
        while matches!(cur.as_ref(), Expr::Lam(..)) {
            let Expr::Lam(_, _, param, _, lam_constraints, next) = cur.as_ref() else {
                break;
            };
            body_constraints.extend(lam_constraints.iter().cloned());
            lam_params.push(param.clone());
            cur = next.clone();
        }
        if lam_params.len() != arity {
            return Err(ParseError::new(
                *body.span(),
                format!(
                    "lambda has {} parameter(s) but signature expects {}",
                    lam_params.len(),
                    arity
                ),
            ));
        }
        let params = lam_params.into_iter().zip(param_tys).collect();
        return Ok(SignatureFnParts {
            params,
            ret,
            body: cur,
            constraints: body_constraints,
        });
    }

    let var_span = *body.span();
    let vars: Vec<Var> = (0..arity)
        .map(|i| Var::with_span(var_span, format!("_arg{i}")))
        .collect();
    let mut applied = body.clone();
    for var in &vars {
        applied = Arc::new(Expr::App(
            Span::from_begin_end(applied.span().begin, applied.span().end),
            applied,
            Arc::new(Expr::Var(var.clone())),
        ));
    }
    let params = vars.into_iter().zip(param_tys).collect();
    Ok(SignatureFnParts {
        params,
        ret,
        body: applied,
        constraints: body_constraints,
    })
}

fn flatten_decl_signature(sig: TypeExpr) -> (Vec<(Var, TypeExpr)>, TypeExpr) {
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
    let params = param_tys
        .into_iter()
        .enumerate()
        .map(|(i, ann)| (Var::with_span(*ann.span(), format!("_arg{i}")), ann))
        .collect();
    (params, ret)
}

fn pattern_binding_var(pat: &Pattern) -> Option<Var> {
    match pat {
        Pattern::Var(var) => Some(var.clone()),
        Pattern::Named(span, NameRef::Unqualified(name), args) if args.is_empty() => Some(Var {
            span: *span,
            name: name.clone(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_pattern(source: &str) -> Pattern {
        let mut parser = PegParser::new(Token::tokenize(source).expect("tokenize pattern"));
        parser
            .parse_pattern_for_test()
            .expect("parse pattern")
            .reset_spans()
    }

    fn parse_pattern_err(source: &str) -> Vec<ParseError> {
        let mut parser = PegParser::new(Token::tokenize(source).expect("tokenize pattern"));
        parser
            .parse_pattern_for_test()
            .expect_err("parse should fail")
    }

    fn var(name: &str) -> Pattern {
        Pattern::Var(Var::new(name))
    }

    fn named(name: &str, args: Vec<Pattern>) -> Pattern {
        Pattern::Named(Span::default(), NameRef::from(name), args)
    }

    #[test]
    fn grammar_driven_pattern_parses_constructor_application() {
        assert_eq!(
            parse_pattern("Ok x (Err e)"),
            named("Ok", vec![var("x"), named("Err", vec![var("e")])])
        );
    }

    #[test]
    fn grammar_driven_pattern_parses_qualified_constructor_application() {
        assert_eq!(
            parse_pattern("Sample.Ok x"),
            named("Sample.Ok", vec![var("x")])
        );
    }

    #[test]
    fn grammar_driven_pattern_cons_is_right_associative() {
        assert_eq!(
            parse_pattern("x::y::zs"),
            Pattern::Cons(
                Span::default(),
                Box::new(var("x")),
                Box::new(Pattern::Cons(
                    Span::default(),
                    Box::new(var("y")),
                    Box::new(var("zs"))
                ))
            )
        );
    }

    #[test]
    fn grammar_driven_pattern_rejects_trailing_list_comma() {
        let errs = parse_pattern_err("[x,]");
        assert!(
            errs.iter().any(|err| err.message.contains("unexpected ]")),
            "expected trailing comma pattern error, got {errs:?}"
        );
    }
}
