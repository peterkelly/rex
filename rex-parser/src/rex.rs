#[cfg(test)]
use std::collections::BTreeSet;

#[cfg(test)]
use crate::{
    grammar::Peg,
    peg_syntax::{self, SyntaxExpr, SyntaxGrammar},
};

use crate::grammar::{
    Grammar, Item as GrammarItem, Peg as GrammarPeg, TokenKind, and, choice, cut, label, not, opt,
    rep, rep1, rule, seq, tok,
};

pub(crate) const AST_BOUNDARY: &str = include_str!("AST_BOUNDARY.md");
// `rex.rs` is the executable source of truth. This checked-in `.peg` text
// is a review/debugging artifact, and unit tests prove it is the exact
// canonical rendering of the Rust grammar below.
pub(crate) const REX_PEG_GRAMMAR: &str = include_str!("rex.peg");

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum RexRule {
    Program,
    Decl,
    PublicDecl,
    PrivateDecl,
    DeclBody,
    ImportDecl,
    ImportPath,
    RemoteImportPath,
    DottedImportPath,
    RelativeImportPath,
    RelativePrefix,
    ImportPathSegment,
    HashSuffix,
    ImportClause,
    ImportItem,
    ImportAlias,
    TypeDecl,
    TypeParam,
    TypeVariant,
    FnDecl,
    FnSignatureDecl,
    FnParamDecl,
    FnParams,
    ArrowParam,
    NamedParam,
    ParenParam,
    LegacyParamGroup,
    LegacyParam,
    DeclareFnDecl,
    DeclareParamSig,
    BareFnSig,
    ClassDecl,
    SuperClause,
    ClassBlock,
    ClassMethod,
    InstanceDecl,
    InstanceContext,
    InstanceBlock,
    InstanceMethod,
    WhereConstraints,
    TypeConstraints,
    TypeConstraint,
    TypeExpr,
    TypeFun,
    TypeApp,
    TypeAtom,
    TypeParen,
    UnitType,
    TupleType,
    GroupedType,
    TypeRecord,
    TypeField,
    Expr,
    BinaryOp,
    UnaryExpr,
    ApplicationExpr,
    PostfixExpr,
    FieldName,
    AtomExpr,
    HoleExpr,
    IdentExpr,
    BraceExpr,
    ParenExpr,
    UnitExpr,
    OperatorNameExpr,
    TupleExpr,
    GroupedExpr,
    ListExpr,
    DictExpr,
    DictItem,
    BadDictItem,
    RecordUpdateExpr,
    NegExpr,
    LambdaExpr,
    LambdaParam,
    LetExpr,
    LetBinding,
    LetRecBinding,
    IfExpr,
    MatchExpr,
    MatchArm,
    Pattern,
    AppPattern,
    PatternAtom,
    ListPattern,
    DictPattern,
    DictPatternField,
    ParenPattern,
    NameRef,
    ValueName,
}

#[cfg(test)]
impl RexRule {
    pub(crate) const ALL: &'static [Self] = &[
        Self::Program,
        Self::Decl,
        Self::PublicDecl,
        Self::PrivateDecl,
        Self::DeclBody,
        Self::ImportDecl,
        Self::ImportPath,
        Self::RemoteImportPath,
        Self::DottedImportPath,
        Self::RelativeImportPath,
        Self::RelativePrefix,
        Self::ImportPathSegment,
        Self::HashSuffix,
        Self::ImportClause,
        Self::ImportItem,
        Self::ImportAlias,
        Self::TypeDecl,
        Self::TypeParam,
        Self::TypeVariant,
        Self::FnDecl,
        Self::FnSignatureDecl,
        Self::FnParamDecl,
        Self::FnParams,
        Self::ArrowParam,
        Self::NamedParam,
        Self::ParenParam,
        Self::LegacyParamGroup,
        Self::LegacyParam,
        Self::DeclareFnDecl,
        Self::DeclareParamSig,
        Self::BareFnSig,
        Self::ClassDecl,
        Self::SuperClause,
        Self::ClassBlock,
        Self::ClassMethod,
        Self::InstanceDecl,
        Self::InstanceContext,
        Self::InstanceBlock,
        Self::InstanceMethod,
        Self::WhereConstraints,
        Self::TypeConstraints,
        Self::TypeConstraint,
        Self::TypeExpr,
        Self::TypeFun,
        Self::TypeApp,
        Self::TypeAtom,
        Self::TypeParen,
        Self::UnitType,
        Self::TupleType,
        Self::GroupedType,
        Self::TypeRecord,
        Self::TypeField,
        Self::Expr,
        Self::BinaryOp,
        Self::UnaryExpr,
        Self::ApplicationExpr,
        Self::PostfixExpr,
        Self::FieldName,
        Self::AtomExpr,
        Self::HoleExpr,
        Self::IdentExpr,
        Self::BraceExpr,
        Self::ParenExpr,
        Self::UnitExpr,
        Self::OperatorNameExpr,
        Self::TupleExpr,
        Self::GroupedExpr,
        Self::ListExpr,
        Self::DictExpr,
        Self::DictItem,
        Self::BadDictItem,
        Self::RecordUpdateExpr,
        Self::NegExpr,
        Self::LambdaExpr,
        Self::LambdaParam,
        Self::LetExpr,
        Self::LetBinding,
        Self::LetRecBinding,
        Self::IfExpr,
        Self::MatchExpr,
        Self::MatchArm,
        Self::Pattern,
        Self::AppPattern,
        Self::PatternAtom,
        Self::ListPattern,
        Self::DictPattern,
        Self::DictPatternField,
        Self::ParenPattern,
        Self::NameRef,
        Self::ValueName,
    ];

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|rule| rule.to_string() == name)
    }
}

#[cfg(test)]
impl std::fmt::Display for RexRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GrammarLoadError {
    pub(crate) message: String,
}

#[cfg(test)]
impl GrammarLoadError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

// Test-only loader for the checked `.peg` mirror. It deliberately converts text
// back into the same `Grammar<RexRule>` shape used by the parser, so the sync
// test catches structural drift instead of merely checking that both grammars
// happen to accept the same examples.
#[cfg(test)]
pub(crate) fn rex_grammar_from_peg(source: &str) -> Result<Grammar<RexRule>, GrammarLoadError> {
    let syntax = peg_syntax::parse(source).map_err(|err| GrammarLoadError::new(err.message))?;
    syntax_to_rex_grammar(&syntax)
}

#[cfg(test)]
fn syntax_to_rex_grammar(syntax: &SyntaxGrammar) -> Result<Grammar<RexRule>, GrammarLoadError> {
    let first_rule = syntax
        .rules()
        .next()
        .ok_or_else(|| GrammarLoadError::new("expected rule definition"))?;
    let start = resolve_rule_name(&first_rule.name)?;
    let expected = rex_grammar();

    if start != expected.start() {
        return Err(GrammarLoadError::new(format!(
            "expected start rule `{}`, found `{start}`",
            expected.start()
        )));
    }

    let mut seen = BTreeSet::new();
    let mut items = Vec::with_capacity(syntax.items.len());

    for item in &syntax.items {
        match item {
            peg_syntax::SyntaxItem::Rule(syntax_rule) => {
                let rule = resolve_rule_name(&syntax_rule.name)?;
                if !seen.insert(rule) {
                    return Err(GrammarLoadError::new(format!(
                        "duplicate rule definition `{rule}`"
                    )));
                }

                items.push(GrammarItem::Rule(
                    rule,
                    resolve_expr(&syntax_rule.expression)?,
                ));
            }
            peg_syntax::SyntaxItem::Comment(comment) => {
                items.push(GrammarItem::Comment(comment.clone()));
            }
        }
    }

    for (expected_rule, _) in expected.rules() {
        if !seen.contains(&expected_rule) {
            return Err(GrammarLoadError::new(format!(
                "missing rule definition `{expected_rule}`"
            )));
        }
    }

    Ok(Grammar::from_items(start, items))
}

#[cfg(test)]
fn resolve_rule_name(name: &str) -> Result<RexRule, GrammarLoadError> {
    RexRule::from_name(name)
        .ok_or_else(|| GrammarLoadError::new(format!("unknown rule name `{name}`")))
}

#[cfg(test)]
fn resolve_expr(expression: &SyntaxExpr) -> Result<Peg<RexRule>, GrammarLoadError> {
    match expression {
        SyntaxExpr::Name(name) => resolve_symbol(name),
        SyntaxExpr::Seq(items) => resolve_sequence(items),
        SyntaxExpr::Choice(items) => resolve_choice(items),
        SyntaxExpr::Optional(item) => Ok(opt(resolve_expr(item)?)),
        SyntaxExpr::Repeat(item) => Ok(rep(resolve_expr(item)?)),
        SyntaxExpr::Repeat1(item) => Ok(rep1(resolve_expr(item)?)),
        SyntaxExpr::And(item) => Ok(and(resolve_expr(item)?)),
        SyntaxExpr::Not(item) => Ok(not(resolve_expr(item)?)),
        SyntaxExpr::Cut(item) => Ok(cut(resolve_expr(item)?)),
        SyntaxExpr::Label(message, item) => Ok(label(message.clone(), resolve_expr(item)?)),
    }
}

#[cfg(test)]
fn resolve_symbol(name: &str) -> Result<Peg<RexRule>, GrammarLoadError> {
    match (RexRule::from_name(name), TokenKind::from_name(name)) {
        (Some(_), Some(_)) => Err(GrammarLoadError::new(format!(
            "ambiguous grammar symbol `{name}` resolves as both rule and token"
        ))),
        (Some(rule_name), None) => Ok(rule(rule_name)),
        (None, Some(kind)) => Ok(tok(kind)),
        (None, None) => Err(GrammarLoadError::new(format!(
            "unknown grammar symbol `{name}`"
        ))),
    }
}

#[cfg(test)]
fn resolve_sequence(items: &[SyntaxExpr]) -> Result<Peg<RexRule>, GrammarLoadError> {
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

#[cfg(test)]
fn resolve_choice(items: &[SyntaxExpr]) -> Result<Peg<RexRule>, GrammarLoadError> {
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

fn grammar_comment(text: &str) -> GrammarItem<RexRule> {
    GrammarItem::Comment(text.to_string())
}

fn grammar_rule(rule: RexRule, expression: GrammarPeg<RexRule>) -> GrammarItem<RexRule> {
    GrammarItem::Rule(rule, expression)
}

pub(crate) fn rex_grammar() -> Grammar<RexRule> {
    use RexRule as R;
    use TokenKind as T;

    Grammar::from_items(
        R::Program,
        vec![
            grammar_comment("Program"),
            grammar_rule(
                R::Program,
                seq([rep(rule(R::Decl)), opt(rule(R::Expr)), tok(T::Eof)]),
            ),
            grammar_comment("Declarations"),
            grammar_rule(R::Decl, choice([rule(R::PublicDecl), rule(R::PrivateDecl)])),
            grammar_rule(R::PublicDecl, seq([tok(T::Pub), rule(R::DeclBody)])),
            grammar_rule(R::PrivateDecl, rule(R::DeclBody)),
            grammar_rule(
                R::DeclBody,
                choice([
                    rule(R::ImportDecl),
                    rule(R::TypeDecl),
                    rule(R::FnDecl),
                    rule(R::DeclareFnDecl),
                    rule(R::ClassDecl),
                    rule(R::InstanceDecl),
                ]),
            ),
            grammar_comment("Imports"),
            grammar_rule(
                R::ImportDecl,
                seq([
                    tok(T::Import),
                    cut(seq([
                        rule(R::ImportPath),
                        opt(rule(R::ImportClause)),
                        opt(rule(R::ImportAlias)),
                        label("expected `;` after import declaration", tok(T::SemiColon)),
                    ])),
                ]),
            ),
            grammar_rule(
                R::ImportPath,
                choice([
                    rule(R::RemoteImportPath),
                    rule(R::RelativeImportPath),
                    rule(R::DottedImportPath),
                ]),
            ),
            grammar_rule(R::RemoteImportPath, tok(T::HttpsUrl)),
            grammar_rule(
                R::DottedImportPath,
                seq([
                    tok(T::Ident),
                    rep(seq([tok(T::Dot), tok(T::Ident)])),
                    opt(rule(R::HashSuffix)),
                ]),
            ),
            grammar_rule(
                R::RelativeImportPath,
                seq([
                    rule(R::RelativePrefix),
                    tok(T::Ident),
                    rep(rule(R::ImportPathSegment)),
                    opt(rule(R::HashSuffix)),
                ]),
            ),
            grammar_rule(
                R::RelativePrefix,
                rep1(choice([
                    seq([tok(T::Dot), tok(T::Div)]),
                    seq([tok(T::DotDot), tok(T::Div)]),
                ])),
            ),
            grammar_rule(
                R::ImportPathSegment,
                seq([choice([tok(T::Dot), tok(T::Div)]), tok(T::Ident)]),
            ),
            grammar_rule(
                R::HashSuffix,
                seq([tok(T::HashTag), choice([tok(T::Ident), tok(T::Int)])]),
            ),
            grammar_rule(
                R::ImportClause,
                choice([
                    seq([tok(T::ParenL), tok(T::Mul), tok(T::ParenR)]),
                    seq([
                        tok(T::ParenL),
                        rule(R::ImportItem),
                        rep(seq([tok(T::Comma), rule(R::ImportItem)])),
                        tok(T::ParenR),
                    ]),
                ]),
            ),
            grammar_rule(
                R::ImportItem,
                seq([rule(R::ValueName), opt(seq([tok(T::As), tok(T::Ident)]))]),
            ),
            grammar_rule(R::ImportAlias, seq([tok(T::As), tok(T::Ident)])),
            grammar_comment("Type declarations"),
            grammar_rule(
                R::TypeDecl,
                seq([
                    tok(T::Type),
                    cut(seq([
                        tok(T::Ident),
                        rep(rule(R::TypeParam)),
                        tok(T::Assign),
                        rule(R::TypeVariant),
                        rep(seq([tok(T::Pipe), rule(R::TypeVariant)])),
                        label("expected `;` after type declaration", tok(T::SemiColon)),
                    ])),
                ]),
            ),
            grammar_rule(R::TypeParam, tok(T::Ident)),
            grammar_rule(R::TypeVariant, seq([tok(T::Ident), rep(rule(R::TypeAtom))])),
            grammar_comment("Function declarations"),
            grammar_rule(
                R::FnDecl,
                seq([
                    tok(T::Fn),
                    cut(seq([
                        tok(T::Ident),
                        choice([rule(R::FnSignatureDecl), rule(R::FnParamDecl)]),
                    ])),
                ]),
            ),
            grammar_rule(
                R::FnSignatureDecl,
                seq([
                    tok(T::Colon),
                    rule(R::TypeExpr),
                    opt(rule(R::WhereConstraints)),
                    tok(T::Assign),
                    rule(R::Expr),
                    label("expected `;` after function body", tok(T::SemiColon)),
                ]),
            ),
            grammar_rule(
                R::FnParamDecl,
                seq([
                    rule(R::FnParams),
                    tok(T::ArrowR),
                    rule(R::TypeExpr),
                    opt(rule(R::WhereConstraints)),
                    tok(T::Assign),
                    rule(R::Expr),
                    label("expected `;` after function body", tok(T::SemiColon)),
                ]),
            ),
            grammar_rule(
                R::FnParams,
                choice([
                    seq([
                        rule(R::ArrowParam),
                        rep(seq([tok(T::ArrowR), rule(R::ArrowParam)])),
                    ]),
                    rule(R::LegacyParamGroup),
                ]),
            ),
            grammar_rule(
                R::ArrowParam,
                choice([rule(R::ParenParam), rule(R::NamedParam)]),
            ),
            grammar_rule(
                R::NamedParam,
                seq([tok(T::Ident), tok(T::Colon), rule(R::TypeApp)]),
            ),
            grammar_rule(
                R::ParenParam,
                seq([
                    tok(T::ParenL),
                    tok(T::Ident),
                    tok(T::Colon),
                    rule(R::TypeExpr),
                    tok(T::ParenR),
                ]),
            ),
            grammar_rule(
                R::LegacyParamGroup,
                seq([
                    tok(T::ParenL),
                    opt(seq([
                        rule(R::LegacyParam),
                        rep(seq([tok(T::Comma), rule(R::LegacyParam)])),
                    ])),
                    tok(T::ParenR),
                ]),
            ),
            grammar_rule(
                R::LegacyParam,
                seq([tok(T::Ident), tok(T::Colon), rule(R::TypeExpr)]),
            ),
            grammar_rule(
                R::DeclareFnDecl,
                seq([
                    tok(T::Declare),
                    cut(seq([
                        tok(T::Fn),
                        tok(T::Ident),
                        opt(tok(T::Colon)),
                        choice([rule(R::DeclareParamSig), rule(R::BareFnSig)]),
                        label(
                            "expected `;` after declare fn declaration",
                            tok(T::SemiColon),
                        ),
                    ])),
                ]),
            ),
            grammar_rule(
                R::DeclareParamSig,
                seq([
                    rule(R::FnParams),
                    tok(T::ArrowR),
                    rule(R::TypeExpr),
                    opt(rule(R::WhereConstraints)),
                ]),
            ),
            grammar_rule(
                R::BareFnSig,
                seq([rule(R::TypeExpr), opt(rule(R::WhereConstraints))]),
            ),
            grammar_comment("Type classes and instances"),
            grammar_rule(
                R::ClassDecl,
                seq([
                    tok(T::Class),
                    cut(seq([
                        tok(T::Ident),
                        rep(rule(R::TypeParam)),
                        opt(rule(R::SuperClause)),
                        label(
                            "expected `where { ... }` or `;` after class header",
                            choice([rule(R::ClassBlock), tok(T::SemiColon)]),
                        ),
                    ])),
                ]),
            ),
            grammar_rule(R::SuperClause, seq([tok(T::Le), rule(R::TypeConstraints)])),
            grammar_rule(
                R::ClassBlock,
                seq([
                    tok(T::Where),
                    label(
                        "expected `{` after `where` in class declaration",
                        tok(T::BraceL),
                    ),
                    opt(seq([
                        rule(R::ClassMethod),
                        rep(seq([tok(T::SemiColon), rule(R::ClassMethod)])),
                        opt(tok(T::SemiColon)),
                    ])),
                    tok(T::BraceR),
                ]),
            ),
            grammar_rule(
                R::ClassMethod,
                seq([rule(R::ValueName), tok(T::Colon), rule(R::TypeExpr)]),
            ),
            grammar_rule(
                R::InstanceDecl,
                seq([
                    tok(T::Instance),
                    cut(seq([
                        rule(R::NameRef),
                        rule(R::TypeApp),
                        opt(rule(R::InstanceContext)),
                        label(
                            "expected `where { ... }` or `;` after instance header",
                            choice([rule(R::InstanceBlock), tok(T::SemiColon)]),
                        ),
                    ])),
                ]),
            ),
            grammar_rule(
                R::InstanceContext,
                seq([tok(T::Le), rule(R::TypeConstraints)]),
            ),
            grammar_rule(
                R::InstanceBlock,
                seq([
                    tok(T::Where),
                    label(
                        "expected `{` after `where` in instance declaration",
                        tok(T::BraceL),
                    ),
                    opt(seq([
                        rule(R::InstanceMethod),
                        rep(seq([tok(T::SemiColon), rule(R::InstanceMethod)])),
                        opt(tok(T::SemiColon)),
                    ])),
                    tok(T::BraceR),
                ]),
            ),
            grammar_rule(
                R::InstanceMethod,
                seq([rule(R::ValueName), tok(T::Assign), rule(R::Expr)]),
            ),
            grammar_rule(
                R::WhereConstraints,
                seq([tok(T::Where), rule(R::TypeConstraints)]),
            ),
            grammar_rule(
                R::TypeConstraints,
                seq([
                    rule(R::TypeConstraint),
                    rep(seq([tok(T::Comma), rule(R::TypeConstraint)])),
                ]),
            ),
            grammar_rule(R::TypeConstraint, seq([rule(R::NameRef), rule(R::TypeApp)])),
            grammar_comment("Type expressions"),
            grammar_rule(R::TypeExpr, rule(R::TypeFun)),
            grammar_rule(
                R::TypeFun,
                seq([
                    rule(R::TypeApp),
                    opt(seq([tok(T::ArrowR), rule(R::TypeFun)])),
                ]),
            ),
            grammar_rule(R::TypeApp, rep1(rule(R::TypeAtom))),
            grammar_rule(
                R::TypeAtom,
                choice([rule(R::NameRef), rule(R::TypeParen), rule(R::TypeRecord)]),
            ),
            grammar_rule(
                R::TypeParen,
                choice([rule(R::UnitType), rule(R::TupleType), rule(R::GroupedType)]),
            ),
            grammar_rule(R::UnitType, seq([tok(T::ParenL), tok(T::ParenR)])),
            grammar_rule(
                R::TupleType,
                seq([
                    tok(T::ParenL),
                    rule(R::TypeExpr),
                    tok(T::Comma),
                    cut(seq([
                        rule(R::TypeExpr),
                        rep(seq([tok(T::Comma), cut(rule(R::TypeExpr))])),
                        tok(T::ParenR),
                    ])),
                ]),
            ),
            grammar_rule(
                R::GroupedType,
                seq([tok(T::ParenL), rule(R::TypeExpr), tok(T::ParenR)]),
            ),
            grammar_rule(
                R::TypeRecord,
                seq([
                    tok(T::BraceL),
                    opt(seq([
                        rule(R::TypeField),
                        rep(seq([tok(T::Comma), cut(rule(R::TypeField))])),
                    ])),
                    tok(T::BraceR),
                ]),
            ),
            grammar_rule(
                R::TypeField,
                seq([tok(T::Ident), tok(T::Colon), rule(R::TypeExpr)]),
            ),
            grammar_comment("Expressions"),
            grammar_rule(
                R::Expr,
                seq([
                    rule(R::UnaryExpr),
                    rep(seq([rule(R::BinaryOp), cut(rule(R::UnaryExpr))])),
                ]),
            ),
            grammar_rule(R::BinaryOp, tok(T::BinaryOperator)),
            grammar_rule(
                R::UnaryExpr,
                seq([
                    rule(R::ApplicationExpr),
                    rep(seq([tok(T::Is), rule(R::TypeExpr)])),
                ]),
            ),
            grammar_rule(
                R::ApplicationExpr,
                seq([
                    rule(R::PostfixExpr),
                    rep(seq([
                        and(choice([
                            tok(T::ParenL),
                            tok(T::BracketL),
                            tok(T::BraceL),
                            tok(T::Bool),
                            tok(T::Float),
                            tok(T::Int),
                            tok(T::String),
                            tok(T::Question),
                            tok(T::Ident),
                            tok(T::BackSlash),
                            tok(T::Let),
                            tok(T::If),
                            tok(T::Match),
                        ])),
                        cut(rule(R::PostfixExpr)),
                    ])),
                ]),
            ),
            grammar_rule(
                R::PostfixExpr,
                seq([
                    rule(R::AtomExpr),
                    rep(seq([tok(T::Dot), rule(R::FieldName)])),
                ]),
            ),
            grammar_rule(R::FieldName, choice([tok(T::Ident), tok(T::Int)])),
            grammar_rule(
                R::AtomExpr,
                choice([
                    seq([and(tok(T::ParenL)), cut(rule(R::ParenExpr))]),
                    seq([and(tok(T::BracketL)), cut(rule(R::ListExpr))]),
                    seq([and(tok(T::BraceL)), cut(rule(R::BraceExpr))]),
                    tok(T::Bool),
                    tok(T::Float),
                    tok(T::Int),
                    tok(T::String),
                    rule(R::HoleExpr),
                    rule(R::IdentExpr),
                    seq([and(tok(T::BackSlash)), cut(rule(R::LambdaExpr))]),
                    seq([and(tok(T::Let)), cut(rule(R::LetExpr))]),
                    seq([and(tok(T::If)), cut(rule(R::IfExpr))]),
                    seq([and(tok(T::Match)), cut(rule(R::MatchExpr))]),
                    seq([and(tok(T::Sub)), cut(rule(R::NegExpr))]),
                ]),
            ),
            grammar_rule(R::HoleExpr, tok(T::Question)),
            grammar_rule(R::IdentExpr, tok(T::Ident)),
            grammar_rule(
                R::BraceExpr,
                choice([rule(R::DictExpr), rule(R::RecordUpdateExpr)]),
            ),
            grammar_rule(
                R::ParenExpr,
                choice([
                    rule(R::UnitExpr),
                    rule(R::OperatorNameExpr),
                    rule(R::TupleExpr),
                    rule(R::GroupedExpr),
                ]),
            ),
            grammar_rule(R::UnitExpr, seq([tok(T::ParenL), tok(T::ParenR)])),
            grammar_rule(
                R::OperatorNameExpr,
                seq([tok(T::ParenL), tok(T::ValueOperator), tok(T::ParenR)]),
            ),
            grammar_rule(
                R::TupleExpr,
                seq([
                    tok(T::ParenL),
                    rule(R::Expr),
                    tok(T::Comma),
                    opt(seq([
                        rule(R::Expr),
                        rep(seq([tok(T::Comma), rule(R::Expr)])),
                        opt(tok(T::Comma)),
                    ])),
                    tok(T::ParenR),
                ]),
            ),
            grammar_rule(
                R::GroupedExpr,
                seq([tok(T::ParenL), rule(R::Expr), tok(T::ParenR)]),
            ),
            grammar_rule(
                R::ListExpr,
                seq([
                    tok(T::BracketL),
                    opt(seq([
                        rule(R::Expr),
                        rep(seq([tok(T::Comma), rule(R::Expr)])),
                        opt(tok(T::Comma)),
                    ])),
                    tok(T::BracketR),
                ]),
            ),
            grammar_rule(
                R::DictExpr,
                seq([
                    tok(T::BraceL),
                    choice([
                        tok(T::BraceR),
                        seq([
                            and(seq([tok(T::Ident), tok(T::Assign)])),
                            rule(R::DictItem),
                            rep(seq([
                                tok(T::Comma),
                                choice([rule(R::DictItem), rule(R::BadDictItem)]),
                            ])),
                            opt(tok(T::Comma)),
                            tok(T::BraceR),
                        ]),
                    ]),
                ]),
            ),
            grammar_rule(
                R::DictItem,
                seq([tok(T::Ident), tok(T::Assign), rule(R::Expr)]),
            ),
            grammar_rule(R::BadDictItem, seq([tok(T::Ident), not(tok(T::Assign))])),
            grammar_rule(
                R::RecordUpdateExpr,
                seq([
                    tok(T::BraceL),
                    rule(R::Expr),
                    label("expected `with`", tok(T::With)),
                    rule(R::DictExpr),
                    tok(T::BraceR),
                ]),
            ),
            grammar_rule(R::NegExpr, seq([tok(T::Sub), cut(rule(R::Expr))])),
            grammar_rule(
                R::LambdaExpr,
                seq([
                    tok(T::BackSlash),
                    cut(seq([
                        rep(rule(R::LambdaParam)),
                        opt(rule(R::WhereConstraints)),
                        tok(T::ArrowR),
                        rule(R::Expr),
                    ])),
                ]),
            ),
            grammar_rule(
                R::LambdaParam,
                choice([
                    seq([tok(T::Ident), opt(seq([tok(T::Colon), rule(R::TypeExpr)]))]),
                    seq([
                        tok(T::ParenL),
                        tok(T::Ident),
                        tok(T::Colon),
                        rule(R::TypeExpr),
                        tok(T::ParenR),
                    ]),
                ]),
            ),
            grammar_rule(
                R::LetExpr,
                seq([
                    tok(T::Let),
                    cut(seq([
                        choice([
                            seq([
                                tok(T::Rec),
                                rule(R::LetRecBinding),
                                rep(seq([tok(T::Comma), rule(R::LetRecBinding)])),
                            ]),
                            seq([
                                rule(R::LetBinding),
                                rep(seq([tok(T::Comma), rule(R::LetBinding)])),
                            ]),
                        ]),
                        tok(T::In),
                        rule(R::Expr),
                    ])),
                ]),
            ),
            grammar_rule(
                R::LetBinding,
                seq([
                    rule(R::Pattern),
                    opt(seq([tok(T::Colon), rule(R::TypeExpr)])),
                    tok(T::Assign),
                    rule(R::Expr),
                ]),
            ),
            grammar_rule(
                R::LetRecBinding,
                seq([
                    rule(R::Pattern),
                    opt(seq([tok(T::Colon), rule(R::TypeExpr)])),
                    tok(T::Assign),
                    rule(R::Expr),
                ]),
            ),
            grammar_rule(
                R::IfExpr,
                seq([
                    tok(T::If),
                    cut(seq([
                        rule(R::Expr),
                        tok(T::Then),
                        rule(R::Expr),
                        tok(T::Else),
                        rule(R::Expr),
                    ])),
                ]),
            ),
            grammar_rule(
                R::MatchExpr,
                seq([
                    tok(T::Match),
                    cut(seq([
                        label(
                            "expected `with {` after match scrutinee",
                            seq([rule(R::Expr), tok(T::With)]),
                        ),
                        label(
                            "expected `{` after `with` in match expression",
                            tok(T::BraceL),
                        ),
                        rep1(rule(R::MatchArm)),
                        tok(T::BraceR),
                    ])),
                ]),
            ),
            grammar_rule(
                R::MatchArm,
                seq([
                    tok(T::Case),
                    rule(R::Pattern),
                    tok(T::ArrowR),
                    rule(R::Expr),
                    label("expected `;` after match arm expression", tok(T::SemiColon)),
                ]),
            ),
            grammar_comment("Patterns"),
            grammar_rule(
                R::Pattern,
                seq([
                    rule(R::AppPattern),
                    opt(seq([tok(T::ColonColon), rule(R::Pattern)])),
                ]),
            ),
            grammar_rule(
                R::AppPattern,
                choice([
                    seq([rule(R::NameRef), rep(rule(R::PatternAtom))]),
                    rule(R::PatternAtom),
                ]),
            ),
            grammar_rule(
                R::PatternAtom,
                choice([
                    tok(T::Ident),
                    rule(R::ListPattern),
                    rule(R::DictPattern),
                    rule(R::ParenPattern),
                ]),
            ),
            grammar_rule(
                R::ListPattern,
                seq([
                    tok(T::BracketL),
                    opt(seq([
                        rule(R::Pattern),
                        rep(seq([tok(T::Comma), cut(rule(R::Pattern))])),
                    ])),
                    tok(T::BracketR),
                ]),
            ),
            grammar_rule(
                R::DictPattern,
                seq([
                    tok(T::BraceL),
                    opt(seq([
                        rule(R::DictPatternField),
                        rep(seq([tok(T::Comma), cut(rule(R::DictPatternField))])),
                    ])),
                    tok(T::BraceR),
                ]),
            ),
            grammar_rule(
                R::DictPatternField,
                seq([tok(T::Ident), opt(seq([tok(T::Colon), rule(R::Pattern)]))]),
            ),
            grammar_rule(
                R::ParenPattern,
                choice([
                    seq([tok(T::ParenL), tok(T::ParenR)]),
                    seq([
                        tok(T::ParenL),
                        rule(R::Pattern),
                        tok(T::Comma),
                        cut(seq([
                            rule(R::Pattern),
                            rep(seq([tok(T::Comma), cut(rule(R::Pattern))])),
                            tok(T::ParenR),
                        ])),
                    ]),
                    seq([tok(T::ParenL), rule(R::Pattern), tok(T::ParenR)]),
                ]),
            ),
            grammar_comment("Names"),
            grammar_rule(
                R::NameRef,
                seq([tok(T::Ident), rep(seq([tok(T::Dot), tok(T::Ident)]))]),
            ),
            grammar_rule(R::ValueName, choice([tok(T::Ident), tok(T::ValueOperator)])),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::grammar_to_string;

    fn assert_rex_grammar_sync() {
        let rust_grammar = rex_grammar();
        let rendered = grammar_to_string(&rust_grammar);
        let loaded = rex_grammar_from_peg(REX_PEG_GRAMMAR).unwrap_or_else(|err| {
            panic!("checked-in rex.peg did not parse as the Rex grammar: {err:?}");
        });

        assert_eq!(
            REX_PEG_GRAMMAR, rendered,
            "checked-in rex.peg must be regenerated from rex_grammar()"
        );
        assert_eq!(
            loaded, rust_grammar,
            "checked-in rex.peg must resolve to the same Grammar<RexRule> as rex_grammar()"
        );
        assert_eq!(
            grammar_to_string(&loaded),
            REX_PEG_GRAMMAR,
            "loaded grammar must render back to canonical rex.peg text"
        );
    }

    #[test]
    fn rex_grammar_is_structurally_comparable() {
        let grammar = rex_grammar();

        assert_eq!(grammar, rex_grammar());
        assert_eq!(grammar.start(), RexRule::Program);
        assert_eq!(
            grammar.expression(RexRule::PrivateDecl),
            Some(&rule(RexRule::DeclBody))
        );
    }

    #[test]
    fn rex_grammar_rules_iterate_in_stable_order() {
        let grammar = rex_grammar();
        let rules = grammar.rules().map(|(rule, _)| rule).collect::<Vec<_>>();
        let mut sorted = rules.clone();

        sorted.sort();

        assert_eq!(rules, sorted);
        assert_eq!(rules.first(), Some(&RexRule::Program));
        assert_eq!(rules.last(), Some(&RexRule::ValueName));
    }

    #[test]
    fn grammar_symbols_have_stable_print_names() {
        assert_eq!(RexRule::Program.to_string(), "Program");
        assert_eq!(TokenKind::Import.to_string(), "IMPORT");
        assert_eq!(TokenKind::ArrowR.to_string(), "ARROW_R");
        assert_eq!(TokenKind::from_name("PAREN_L"), Some(TokenKind::ParenL));
        assert_eq!(TokenKind::from_name("ParenL"), None);
    }

    #[test]
    fn rex_grammar_renders_deterministically() {
        let rendered = grammar_to_string(&rex_grammar());

        assert_eq!(rendered, grammar_to_string(&rex_grammar()));
        assert!(rendered.starts_with("\n# Program\n\nProgram            <- Decl* Expr? EOF\n"));
        assert!(rendered.contains("\n# Declarations\n\nDecl               <-"));
        assert!(rendered.contains("\n# Expressions\n\nExpr               <-"));
        assert!(rendered.contains(
            "ImportDecl         <- IMPORT cut(ImportPath ImportClause? ImportAlias? label("
        ));
        assert!(rendered.contains("ValueName          <- IDENT / VALUE_OPERATOR\n"));
        assert!(!rendered.contains("'import'"));
    }

    #[test]
    fn rex_grammar_file_is_synced_with_rust_grammar() {
        assert_rex_grammar_sync();
    }

    #[test]
    fn rex_grammar_resolution_rejects_unknown_symbols() {
        let source = REX_PEG_GRAMMAR.replacen(
            "Program            <- Decl*",
            "Program            <- Missing*",
            1,
        );
        let err = rex_grammar_from_peg(&source).unwrap_err();

        assert_eq!(err.message, "unknown grammar symbol `Missing`");
    }

    #[test]
    fn rex_grammar_resolution_rejects_duplicate_rules() {
        let source = format!("{REX_PEG_GRAMMAR}Program <- EOF\n");
        let err = rex_grammar_from_peg(&source).unwrap_err();

        assert_eq!(err.message, "duplicate rule definition `Program`");
    }

    #[test]
    fn rex_grammar_resolution_rejects_missing_rules() {
        let source = REX_PEG_GRAMMAR.replace("ValueName          <- IDENT / VALUE_OPERATOR\n", "");
        let err = rex_grammar_from_peg(&source).unwrap_err();

        assert_eq!(err.message, "missing rule definition `ValueName`");
    }

    #[test]
    fn rex_grammar_resolution_rejects_wrong_start_rule() {
        let source = REX_PEG_GRAMMAR.replacen("Program            <-", "Decl               <-", 1);
        let err = rex_grammar_from_peg(&source).unwrap_err();

        assert_eq!(err.message, "expected start rule `Program`, found `Decl`");
    }
}
