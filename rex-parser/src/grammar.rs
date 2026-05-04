use crate::formal::{
    Grammar, Peg, TokenKind, and, choice, cut, label, not, opt, rep, rep1, rule, seq, tok,
};

pub(crate) const AST_BOUNDARY: &str = include_str!("AST_BOUNDARY.md");
pub(crate) const REX_PEG_GRAMMAR: &str = include_str!("grammar.peg");

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

pub(crate) fn rex_grammar() -> Grammar<RexRule> {
    use RexRule as R;
    use TokenKind as T;

    Grammar::new(
        R::Program,
        vec![
            (
                R::Program,
                seq([rep(rule(R::Decl)), opt(rule(R::Expr)), tok(T::Eof)]),
            ),
            (R::Decl, choice([rule(R::PublicDecl), rule(R::PrivateDecl)])),
            (R::PublicDecl, seq([tok(T::Pub), rule(R::DeclBody)])),
            (R::PrivateDecl, rule(R::DeclBody)),
            (
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
            (
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
            (
                R::ImportPath,
                choice([
                    rule(R::RemoteImportPath),
                    rule(R::RelativeImportPath),
                    rule(R::DottedImportPath),
                ]),
            ),
            (R::RemoteImportPath, tok(T::HttpsUrl)),
            (
                R::DottedImportPath,
                seq([
                    tok(T::Ident),
                    rep(seq([tok(T::Dot), tok(T::Ident)])),
                    opt(rule(R::HashSuffix)),
                ]),
            ),
            (
                R::RelativeImportPath,
                seq([
                    rule(R::RelativePrefix),
                    tok(T::Ident),
                    rep(rule(R::ImportPathSegment)),
                    opt(rule(R::HashSuffix)),
                ]),
            ),
            (
                R::RelativePrefix,
                rep1(choice([
                    seq([tok(T::Dot), tok(T::Div)]),
                    seq([tok(T::DotDot), tok(T::Div)]),
                ])),
            ),
            (
                R::ImportPathSegment,
                seq([choice([tok(T::Dot), tok(T::Div)]), tok(T::Ident)]),
            ),
            (
                R::HashSuffix,
                seq([tok(T::HashTag), choice([tok(T::Ident), tok(T::Int)])]),
            ),
            (
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
            (
                R::ImportItem,
                seq([rule(R::ValueName), opt(seq([tok(T::As), tok(T::Ident)]))]),
            ),
            (R::ImportAlias, seq([tok(T::As), tok(T::Ident)])),
            (
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
            (R::TypeParam, tok(T::Ident)),
            (R::TypeVariant, seq([tok(T::Ident), rep(rule(R::TypeAtom))])),
            (
                R::FnDecl,
                seq([
                    tok(T::Fn),
                    cut(seq([
                        tok(T::Ident),
                        choice([rule(R::FnSignatureDecl), rule(R::FnParamDecl)]),
                    ])),
                ]),
            ),
            (
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
            (
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
            (
                R::FnParams,
                choice([
                    seq([
                        rule(R::ArrowParam),
                        rep(seq([tok(T::ArrowR), rule(R::ArrowParam)])),
                    ]),
                    rule(R::LegacyParamGroup),
                ]),
            ),
            (
                R::ArrowParam,
                choice([rule(R::ParenParam), rule(R::NamedParam)]),
            ),
            (
                R::NamedParam,
                seq([tok(T::Ident), tok(T::Colon), rule(R::TypeApp)]),
            ),
            (
                R::ParenParam,
                seq([
                    tok(T::ParenL),
                    tok(T::Ident),
                    tok(T::Colon),
                    rule(R::TypeExpr),
                    tok(T::ParenR),
                ]),
            ),
            (
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
            (
                R::LegacyParam,
                seq([tok(T::Ident), tok(T::Colon), rule(R::TypeExpr)]),
            ),
            (
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
            (
                R::DeclareParamSig,
                seq([
                    rule(R::FnParams),
                    tok(T::ArrowR),
                    rule(R::TypeExpr),
                    opt(rule(R::WhereConstraints)),
                ]),
            ),
            (
                R::BareFnSig,
                seq([rule(R::TypeExpr), opt(rule(R::WhereConstraints))]),
            ),
            (
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
            (R::SuperClause, seq([tok(T::Le), rule(R::TypeConstraints)])),
            (
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
            (
                R::ClassMethod,
                seq([rule(R::ValueName), tok(T::Colon), rule(R::TypeExpr)]),
            ),
            (
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
            (
                R::InstanceContext,
                seq([tok(T::Le), rule(R::TypeConstraints)]),
            ),
            (
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
            (
                R::InstanceMethod,
                seq([rule(R::ValueName), tok(T::Assign), rule(R::Expr)]),
            ),
            (
                R::WhereConstraints,
                seq([tok(T::Where), rule(R::TypeConstraints)]),
            ),
            (
                R::TypeConstraints,
                seq([
                    rule(R::TypeConstraint),
                    rep(seq([tok(T::Comma), rule(R::TypeConstraint)])),
                ]),
            ),
            (R::TypeConstraint, seq([rule(R::NameRef), rule(R::TypeApp)])),
            (R::TypeExpr, rule(R::TypeFun)),
            (
                R::TypeFun,
                seq([
                    rule(R::TypeApp),
                    opt(seq([tok(T::ArrowR), rule(R::TypeFun)])),
                ]),
            ),
            (R::TypeApp, rep1(rule(R::TypeAtom))),
            (
                R::TypeAtom,
                choice([rule(R::NameRef), rule(R::TypeParen), rule(R::TypeRecord)]),
            ),
            (
                R::TypeParen,
                choice([rule(R::UnitType), rule(R::TupleType), rule(R::GroupedType)]),
            ),
            (R::UnitType, seq([tok(T::ParenL), tok(T::ParenR)])),
            (
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
            (
                R::GroupedType,
                seq([tok(T::ParenL), rule(R::TypeExpr), tok(T::ParenR)]),
            ),
            (
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
            (
                R::TypeField,
                seq([tok(T::Ident), tok(T::Colon), rule(R::TypeExpr)]),
            ),
            (
                R::Expr,
                seq([
                    rule(R::UnaryExpr),
                    rep(seq([rule(R::BinaryOp), cut(rule(R::UnaryExpr))])),
                ]),
            ),
            (R::BinaryOp, tok(T::BinaryOperator)),
            (
                R::UnaryExpr,
                seq([
                    rule(R::ApplicationExpr),
                    rep(seq([tok(T::Is), rule(R::TypeExpr)])),
                ]),
            ),
            (
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
            (
                R::PostfixExpr,
                seq([
                    rule(R::AtomExpr),
                    rep(seq([tok(T::Dot), rule(R::FieldName)])),
                ]),
            ),
            (R::FieldName, choice([tok(T::Ident), tok(T::Int)])),
            (
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
            (R::HoleExpr, tok(T::Question)),
            (R::IdentExpr, tok(T::Ident)),
            (
                R::BraceExpr,
                choice([rule(R::DictExpr), rule(R::RecordUpdateExpr)]),
            ),
            (
                R::ParenExpr,
                choice([
                    rule(R::UnitExpr),
                    rule(R::OperatorNameExpr),
                    rule(R::TupleExpr),
                    rule(R::GroupedExpr),
                ]),
            ),
            (R::UnitExpr, seq([tok(T::ParenL), tok(T::ParenR)])),
            (
                R::OperatorNameExpr,
                seq([tok(T::ParenL), tok(T::ValueOperator), tok(T::ParenR)]),
            ),
            (
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
            (
                R::GroupedExpr,
                seq([tok(T::ParenL), rule(R::Expr), tok(T::ParenR)]),
            ),
            (
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
            (
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
            (
                R::DictItem,
                seq([tok(T::Ident), tok(T::Assign), rule(R::Expr)]),
            ),
            (R::BadDictItem, seq([tok(T::Ident), not(tok(T::Assign))])),
            (
                R::RecordUpdateExpr,
                seq([
                    tok(T::BraceL),
                    rule(R::Expr),
                    label("expected `with`", tok(T::With)),
                    rule(R::DictExpr),
                    tok(T::BraceR),
                ]),
            ),
            (R::NegExpr, seq([tok(T::Sub), cut(rule(R::Expr))])),
            (
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
            (
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
            (
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
            (
                R::LetBinding,
                seq([
                    rule(R::Pattern),
                    opt(seq([tok(T::Colon), rule(R::TypeExpr)])),
                    tok(T::Assign),
                    rule(R::Expr),
                ]),
            ),
            (
                R::LetRecBinding,
                seq([
                    rule(R::Pattern),
                    opt(seq([tok(T::Colon), rule(R::TypeExpr)])),
                    tok(T::Assign),
                    rule(R::Expr),
                ]),
            ),
            (
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
            (
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
            (
                R::MatchArm,
                seq([
                    tok(T::When),
                    rule(R::Pattern),
                    tok(T::ArrowR),
                    rule(R::Expr),
                    label("expected `;` after match arm expression", tok(T::SemiColon)),
                ]),
            ),
            (
                R::Pattern,
                seq([
                    rule(R::AppPattern),
                    opt(seq([tok(T::ColonColon), rule(R::Pattern)])),
                ]),
            ),
            (
                R::AppPattern,
                choice([
                    seq([rule(R::NameRef), rep(rule(R::PatternAtom))]),
                    rule(R::PatternAtom),
                ]),
            ),
            (
                R::PatternAtom,
                choice([
                    tok(T::Ident),
                    rule(R::ListPattern),
                    rule(R::DictPattern),
                    rule(R::ParenPattern),
                ]),
            ),
            (
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
            (
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
            (
                R::DictPatternField,
                seq([tok(T::Ident), opt(seq([tok(T::Colon), rule(R::Pattern)]))]),
            ),
            (
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
            (
                R::NameRef,
                seq([tok(T::Ident), rep(seq([tok(T::Dot), tok(T::Ident)]))]),
            ),
            (R::ValueName, choice([tok(T::Ident), tok(T::ValueOperator)])),
        ],
    )
}

#[allow(dead_code)]
fn _peg_type_check(_: Peg<RexRule>) {}
