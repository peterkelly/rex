//! Source-level syntax tree types.
//!
//! Most embedders do not need to construct these types by hand. They are useful
//! when building tools around Rex source, accepting pre-parsed modules from a
//! custom importer, or compiling an expression after calling [`parse`].
//!
//! [`parse`]: crate::parser::parse

/// A Rex type class declaration from source.
pub use rex_ast::ClassDecl;

/// A method signature inside a Rex type class declaration.
pub use rex_ast::ClassMethodSig;

/// A parsed Rex source unit containing top-level declarations and an optional body expression.
pub use rex_ast::CompilationUnit;

/// A top-level Rex declaration.
pub use rex_ast::Decl;

/// A declaration for a host-provided function whose implementation is supplied by Rust.
pub use rex_ast::DeclareFnDecl;

/// A Rex expression node.
pub use rex_ast::Expr;

/// A Rex function or value declaration.
pub use rex_ast::FnDecl;

/// A stable identifier attached to parsed syntax nodes.
pub use rex_ast::Id;

/// The item list or wildcard in an `import` declaration.
pub use rex_ast::ImportClause;

/// A Rex `import` declaration.
pub use rex_ast::ImportDecl;

/// A single imported name and optional alias.
pub use rex_ast::ImportItem;

/// The module path named by an `import` declaration.
pub use rex_ast::ImportPath;

/// A Rex type class instance declaration.
pub use rex_ast::InstanceDecl;

/// A method implementation inside a Rex type class instance.
pub use rex_ast::InstanceMethodImpl;

/// A source name that may be unqualified or module-qualified.
pub use rex_ast::NameRef;

/// A pattern used in `match`, function arguments, and bindings.
pub use rex_ast::Pattern;

/// A line and column position in Rex source.
pub use rex_ast::Position;

/// A persistent mapping from variable names to expressions.
pub use rex_ast::Scope;

/// A source range used for diagnostics and tooling.
pub use rex_ast::Span;

/// Trait implemented by syntax nodes that carry a [`Span`].
pub use rex_ast::Spanned;

/// Interned Rex identifier text.
pub use rex_ast::Symbol;

/// A type class constraint in a type signature or instance head.
pub use rex_ast::TypeConstraint;

/// A Rex algebraic data type declaration.
pub use rex_ast::TypeDecl;

/// A source-level Rex type expression.
pub use rex_ast::TypeExpr;

/// A named field inside a structural record type expression.
pub use rex_ast::TypeField;

/// A documented generic parameter on a type declaration.
pub use rex_ast::TypeParam;

/// A constructor variant inside an algebraic data type declaration.
pub use rex_ast::TypeVariant;

/// A documented constructor argument inside an algebraic data type declaration.
pub use rex_ast::TypeVariantArg;

/// A variable occurrence or binder in the source syntax tree.
pub use rex_ast::Var;
