use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use span::{Span, Spanned};

pub mod macros;
pub mod span;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Precedence(u8);

impl Precedence {
    pub fn new(prec: u8) -> Precedence {
        Precedence(prec)
    }

    pub fn lowest() -> Precedence {
        Precedence(0)
    }

    pub fn highest() -> Precedence {
        Precedence(255)
    }

    pub fn next(&self) -> Precedence {
        Precedence::new(self.0 + 1)
    }
}

/// A Token represents a lexical token in the Arvo programming language. It
/// includes the values of literals, identifiers, and comments.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Token {
    // Reserved keywords
    As(Span),
    Else(Span),
    For(Span),
    If(Span),
    Then(Span),

    // Operators
    Add(Span),
    And(Span),
    Concat(Span),
    Div(Span),
    Dot(Span),
    Eq(Span),
    Ne(Span),
    Ge(Span),
    Gt(Span),
    Le(Span),
    Lt(Span),
    Mod(Span),
    Mul(Span),
    Or(Span),
    Sub(Span),

    // Symbols
    ArrowL(Span),
    ArrowR(Span),
    Assign(Span),
    BackSlash(Span),
    BraceL(Span),
    BraceR(Span),
    BracketL(Span),
    BracketR(Span),
    Colon(Span),
    ColonColon(Span),
    Comma(Span),
    CommentL(Span),
    CommentR(Span),
    DotDot(Span),
    HashTag(Span),
    In(Span),
    Let(Span),
    ParenL(Span),
    ParenR(Span),
    Pipe(Span),
    Question(Span),
    SemiColon(Span),
    Whitespace(Span),
    WhitespaceNewline(Span),

    // Literals
    Bool(bool, Span),
    Float(f64, Span),
    Int(u64, Span),
    Null(Span),
    String(String, Span),

    // Idents
    Ident(String, Span),

    // Eof
    Eof(Span),
}

#[derive(Debug, thiserror::Error)]
pub enum LexicalError {
    #[error("Unexpected token {0}")]
    UnexpectedToken(Span),
}

impl Token {
    pub fn tokenize(input: &str) -> Result<Tokens, LexicalError> {
        let mut line = 1;
        let mut column = 1;
        let mut tokens = Vec::new();

        for capture in Token::regex().captures_iter(input) {
            let begin_line = line;
            let begin_column = column;
            column += capture[0].to_string().chars().count();
            let span = Span::new(begin_line, begin_column, line, column);
            if &capture[0] == "\n" {
                line += 1;
                column = 1;
            }

            // be careful of the ordering of these groups
            let token =
                // Reserved keywords
                if capture.name("As").is_some() {
                    Token::As(span)
                } else if capture.name("Else").is_some() {
                    Token::Else(span)
                } else if capture.name("For").is_some() {
                    Token::For(span)
                } else if capture.name("If").is_some() {
                    Token::If(span)
                } else if capture.name("In").is_some() {
                    Token::In(span)
                } else if capture.name("Then").is_some() {
                    Token::Then(span)
                }

                // Symbols
                else if capture.name("ArrowL").is_some() {
                    Token::ArrowL(span)
                } else if capture.name("ArrowR").is_some() {
                    Token::ArrowR(span)
                } else if capture.name("BackSlash").is_some() {
                    Token::BackSlash(span)
                } else if capture.name("CommentL").is_some() {
                    Token::CommentL(span)
                } else if capture.name("CommentR").is_some() {
                    Token::CommentR(span)
                } else if capture.name("BraceL").is_some() {
                    Token::BraceL(span)
                } else if capture.name("BraceR").is_some() {
                    Token::BraceR(span)
                } else if capture.name("BracketL").is_some() {
                    Token::BracketL(span)
                } else if capture.name("BracketR").is_some() {
                    Token::BracketR(span)
                } else if capture.name("ColonColon").is_some() {
                    Token::ColonColon(span)
                } else if capture.name("Colon").is_some() {
                    Token::Colon(span)
                } else if capture.name("Comma").is_some() {
                    Token::Comma(span)
                } else if capture.name("DotDot").is_some() {
                    Token::DotDot(span)
                } else if capture.name("HashTag").is_some() {
                    Token::HashTag(span)
                } else if capture.name("In").is_some() {
                    Token::In(span)
                } else if capture.name("Let").is_some() {
                    Token::Let(span)
                } else if capture.name("ParenL").is_some() {
                    Token::ParenL(span)
                } else if capture.name("ParenR").is_some() {
                    Token::ParenR(span)
                } else if capture.name("Pipe").is_some() {
                    Token::Pipe(span)
                } else if capture.name("Question").is_some() {
                    Token::Question(span)
                } else if capture.name("SemiColon").is_some() {
                    Token::SemiColon(span)
                } else if capture.name("Whitespace").is_some() {
                    Token::Whitespace(span)
                } else if capture.name("WhitespaceNewline").is_some() {
                    Token::WhitespaceNewline(span)
                }

                // Operators
                else if capture.name("Concat").is_some() {
                    Token::Concat(span)
                } else if capture.name("Add").is_some() {
                    Token::Add(span)
                } else if capture.name("And").is_some() {
                    Token::And(span)
                }else if capture.name("Div").is_some() {
                    Token::Div(span)
                } else if capture.name("Dot").is_some() {
                    Token::Dot(span)
                } else if capture.name("Equal").is_some() {
                    Token::Eq(span)
                } else if capture.name("NotEqual").is_some() {
                    Token::Ne(span)
                } else if capture.name("GreaterThanEq").is_some() {
                    Token::Ge(span)
                } else if capture.name("GreaterThan").is_some() {
                    Token::Gt(span)
                } else if capture.name("LessThanEq").is_some() {
                    Token::Le(span)
                } else if capture.name("LessThan").is_some() {
                    Token::Lt(span)
                } else if capture.name("Mod").is_some() {
                    Token::Mod(span)
                } else if capture.name("Mul").is_some() {
                    Token::Mul(span)
                } else if capture.name("Or").is_some() {
                    Token::Or(span)
                } else if capture.name("Sub").is_some() {
                    Token::Sub(span)
                } else if capture.name("Assign").is_some() {
                    Token::Assign(span) // NOTE: We have moved this here to capture the "Equal" group first
                }

                // Literals
                else if capture.name("Bool").is_some() {
                    Token::Bool(bool::from_str(capture.name("Bool").unwrap().as_str()).unwrap(), span)
                } else if capture.name("Float").is_some() {
                    Token::Float(f64::from_str(capture.name("Float").unwrap().as_str()).unwrap(), span)
                } else if capture.name("Int").is_some() {
                    Token::Int(u64::from_str(capture.name("Int").unwrap().as_str()).unwrap(), span)
                } else if capture.name("Null").is_some() {
                    Token::Null(span)
                } else if capture.name("DoubleString").is_some() {
                    Token::String(capture.name("DoubleString").unwrap().as_str().to_string(), span)
                } else if capture.name("SingleString").is_some() {
                    Token::String(capture.name("SingleString").unwrap().as_str().to_string(), span)
                }

                // Idents
                else if capture.name("Ident").is_some() {
                    Token::Ident(capture.name("Ident").unwrap().as_str().to_string(), span)
                }

                // Other
                else {
                    return Err(LexicalError::UnexpectedToken(span));
                };
            tokens.push(token)
        }

        // Filter whitespace
        Ok(Tokens {
            items: tokens
                .into_iter()
                .filter(|token| !matches!(*token, Token::Whitespace(..)))
                .collect(),
            eof: Span::new(line, column, line, column),
        })
    }

    /// Get the regular expression that can capture all Tokens. The regular
    /// expression can capture Unicode characters.
    ///
    /// # Return
    /// An unwrapped Regex object.
    pub fn regex() -> regex::Regex {
        regex::Regex::from_str(concat!(
            // Reserved keywords (with word boundaries)
            r"(?P<As>\bas\b)|",
            r"(?P<Else>\belse\b)|",
            r"(?P<For>\bfor\b)|",
            r"(?P<If>\bif\b)|",
            r"(?P<Then>\bthen\b)|",
            // Symbols
            r"(?P<ArrowL><-|←)|",
            r"(?P<ArrowR>->|→)|",
            r"(?P<BackSlash>\\|λ)|",
            r"(?P<CommentL>\{-)|",
            r"(?P<CommentR>-\})|",
            r"(?P<BraceL>\{)|",
            r"(?P<BraceR>\})|",
            r"(?P<BracketL>\[)|",
            r"(?P<BracketR>\])|",
            r"(?P<ColonColon>::)|", // Must go before :
            r"(?P<Colon>:)|",
            r"(?P<Comma>,)|",
            r"(?P<DotDot>\.\.)|",
            r"(?P<HashTag>\#)|",
            r"(?P<In>\bin\b)|",   // Added word boundaries
            r"(?P<Let>\blet\b)|", // Added word boundaries
            r"(?P<LambdaR>->)|",
            r"(?P<ParenL>\()|",
            r"(?P<ParenR>\))|",
            // r"(?P<Pipe>\|)|",
            r"(?P<Question>\?)|",
            r"(?P<SemiColon>;)|",
            r"(?P<Whitespace>( |\t))|",
            r"(?P<WhitespaceNewline>(\n|\r))|",
            // Operators
            r"(?P<Concat>\+\+)|",
            r"(?P<Add>\+)|",
            r"(?P<And>&&)|",
            r"(?P<Div>/)|",
            r"(?P<Dot>\.)|",
            r"(?P<Equal>==)|",
            r"(?P<Assign>=)|", // Must come after `==`
            r"(?P<NotEqual>!=)|",
            r"(?P<LessThanEq><=)|",
            r"(?P<LessThan><)|",
            r"(?P<GreaterThanEq>>=)|",
            r"(?P<GreaterThan>>)|",
            r"(?P<Mod>%)|",
            r"(?P<Mul>\*)|",
            r"(?P<Or>\|\|)|",
            r"(?P<Sub>-)|",
            // Literals (with word boundaries for bool and null)
            r"(?P<Bool>\b(true|false)\b)|",
            r"(?P<Float>[0-9]+\.[0-9]+)|",
            r"(?P<Int>[0-9]+)|",
            r"(?P<Null>\bnull\b)|",
            r#""(?P<DoubleString>(\\"|[^"])*)"|"#,
            r#"'(?P<SingleString>(\\'|[^'])*)'|"#,
            // Idents
            r"(?P<Ident>[_a-zA-Z]([:_a-zA-Z]|[0-9])*)|",
            // Unexpected
            r"(.)",
        ))
        .unwrap()
    }

    pub fn precedence(&self) -> Precedence {
        use Token::*;

        match self {
            Or(..) => Precedence(1),
            And(..) => Precedence(2),
            Eq(..) | Ne(..) | Lt(..) | Le(..) | Gt(..) | Ge(..) => Precedence(3),
            Add(..) | Sub(..) | Concat(..) => Precedence(4),
            Mul(..) | Div(..) | Mod(..) => Precedence(5),
            Dot(..) => Precedence(6),
            Ident(..) => Precedence::highest(),
            _ => Precedence::lowest(),
        }
    }

    pub fn is_whitespace(&self) -> bool {
        matches!(self, Token::Whitespace(..) | Token::WhitespaceNewline(..))
    }
}

impl Spanned for Token {
    fn span(&self) -> &Span {
        use Token::*;

        match self {
            // Reserved keywords
            As(span, ..) => span,
            Else(span, ..) => span,
            For(span, ..) => span,
            If(span, ..) => span,
            Then(span, ..) => span,

            // Symbols
            ArrowL(span, ..) => span,
            ArrowR(span, ..) => span,
            Assign(span, ..) => span,
            BackSlash(span, ..) => span,
            BraceL(span, ..) => span,
            BraceR(span, ..) => span,
            BracketL(span, ..) => span,
            BracketR(span, ..) => span,
            Colon(span, ..) => span,
            ColonColon(span, ..) => span,
            Comma(span, ..) => span,
            CommentL(span, ..) => span,
            CommentR(span, ..) => span,
            Dot(span, ..) => span,
            DotDot(span, ..) => span,
            HashTag(span, ..) => span,
            In(span, ..) => span,
            Let(span, ..) => span,
            ParenL(span, ..) => span,
            ParenR(span, ..) => span,
            Pipe(span, ..) => span,
            Question(span, ..) => span,
            SemiColon(span, ..) => span,
            Whitespace(span, ..) => span,
            WhitespaceNewline(span, ..) => span,

            // Operators
            Add(span, ..) => span,
            And(span, ..) => span,
            Concat(span, ..) => span,
            Div(span, ..) => span,
            Eq(span, ..) => span,
            Ne(span, ..) => span,
            Ge(span, ..) => span,
            Gt(span, ..) => span,
            Le(span, ..) => span,
            Lt(span, ..) => span,
            Mod(span, ..) => span,
            Mul(span, ..) => span,
            Or(span, ..) => span,
            Sub(span, ..) => span,

            // Literals
            Bool(_, span, ..) => span,
            Float(_, span, ..) => span,
            Int(_, span, ..) => span,
            Null(span, ..) => span,
            String(_, span, ..) => span,

            // Idents
            Ident(_, span, ..) => span,

            // Eof
            Eof(span) => span,
        }
    }

    fn span_mut(&mut self) -> &mut Span {
        use Token::*;

        match self {
            // Reserved keywords
            As(span, ..) => span,
            Else(span, ..) => span,
            For(span, ..) => span,
            If(span, ..) => span,
            Then(span, ..) => span,

            // Symbols
            ArrowL(span, ..) => span,
            ArrowR(span, ..) => span,
            Assign(span, ..) => span,
            BackSlash(span, ..) => span,
            BraceL(span, ..) => span,
            BraceR(span, ..) => span,
            BracketL(span, ..) => span,
            BracketR(span, ..) => span,
            Colon(span, ..) => span,
            ColonColon(span, ..) => span,
            Comma(span, ..) => span,
            CommentL(span, ..) => span,
            CommentR(span, ..) => span,
            Dot(span, ..) => span,
            DotDot(span, ..) => span,
            HashTag(span, ..) => span,
            In(span, ..) => span,
            Let(span, ..) => span,
            ParenL(span, ..) => span,
            ParenR(span, ..) => span,
            Pipe(span, ..) => span,
            Question(span, ..) => span,
            SemiColon(span, ..) => span,
            Whitespace(span, ..) => span,
            WhitespaceNewline(span, ..) => span,

            // Operators
            Add(span, ..) => span,
            And(span, ..) => span,
            Concat(span, ..) => span,
            Div(span, ..) => span,
            Eq(span, ..) => span,
            Ne(span, ..) => span,
            Ge(span, ..) => span,
            Gt(span, ..) => span,
            Le(span, ..) => span,
            Lt(span, ..) => span,
            Mod(span, ..) => span,
            Mul(span, ..) => span,
            Or(span, ..) => span,
            Sub(span, ..) => span,

            // Literals
            Bool(_, span, ..) => span,
            Float(_, span, ..) => span,
            Int(_, span, ..) => span,
            Null(span, ..) => span,
            String(_, span, ..) => span,

            // Idents
            Ident(_, span, ..) => span,

            // Eof
            Eof(span) => span,
        }
    }
}

impl Display for Token {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        use Token::*;

        match self {
            // Reserved keywords
            As(..) => write!(f, "as"),
            Else(..) => write!(f, "else"),
            For(..) => write!(f, "for"),
            If(..) => write!(f, "if"),
            Then(..) => write!(f, "then"),

            // Symbols
            ArrowL(..) => write!(f, "<-"),
            ArrowR(..) => write!(f, "->"),
            Assign(..) => write!(f, "="),
            BackSlash(..) => write!(f, "\\"),
            BraceL(..) => write!(f, "{{"),
            BraceR(..) => write!(f, "}}"),
            BracketL(..) => write!(f, "["),
            BracketR(..) => write!(f, "]"),
            Colon(..) => write!(f, ":"),
            ColonColon(..) => write!(f, "::"),
            Comma(..) => write!(f, ","),
            CommentL(..) => write!(f, "{{-"),
            CommentR(..) => write!(f, "-}}"),
            Dot(..) => write!(f, "."),
            DotDot(..) => write!(f, ".."),
            HashTag(..) => write!(f, "#"),
            In(..) => write!(f, "in"),
            Let(..) => write!(f, "let"),
            ParenL(..) => write!(f, "("),
            ParenR(..) => write!(f, ")"),
            Pipe(..) => write!(f, "|"),
            Question(..) => write!(f, "?"),
            SemiColon(..) => write!(f, ";"),
            Whitespace(..) => write!(f, " "),
            WhitespaceNewline(..) => writeln!(f),

            // Operators
            Add(..) => write!(f, "+"),
            And(..) => write!(f, "&&"),
            Concat(..) => write!(f, "++"),
            Div(..) => write!(f, "/"),
            Eq(..) => write!(f, "=="),
            Ne(..) => write!(f, "!="),
            Gt(..) => write!(f, ">"),
            Ge(..) => write!(f, ">="),
            Lt(..) => write!(f, "<"),
            Le(..) => write!(f, "<="),
            Mod(..) => write!(f, "%"),
            Mul(..) => write!(f, "*"),
            Or(..) => write!(f, "||"),
            Sub(..) => write!(f, "-"),

            // Literals
            Bool(x, ..) => write!(f, "{}", x),
            Float(x, ..) => write!(f, "{}", x),
            Int(x, ..) => write!(f, "{}", x),
            Null(..) => write!(f, "null"),
            String(x, ..) => write!(f, "{}", x),

            // Idents
            Ident(ident, ..) => write!(f, "{}", ident),

            // Eof
            Eof(..) => write!(f, "EOF"),
        }
    }
}

pub struct Tokens {
    pub items: Vec<Token>,
    pub eof: Span,
}
