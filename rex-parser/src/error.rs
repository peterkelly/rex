use std::fmt::{self, Display, Formatter};

use rex_ast::Span;

use crate::lexer::LexicalError;

#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub span: Span,
    pub message: String,
}

impl ParseError {
    pub fn new(span: Span, message: impl Into<String>) -> ParseError {
        ParseError {
            span,
            message: message.into(),
        }
    }

    pub(crate) fn from_lexical_error(err: LexicalError) -> ParseError {
        let span = match &err {
            LexicalError::UnexpectedToken(span) | LexicalError::InvalidLiteral { span, .. } => {
                *span
            }
            LexicalError::Internal(_) => Span::default(),
        };
        ParseError::new(span, format!("lex error: {err}"))
    }
}

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.span.begin, self.message)
    }
}

impl std::error::Error for ParseError {}
