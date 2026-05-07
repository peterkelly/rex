use std::fmt::{self, Display, Formatter};

use rex_ast::span::Span;

use crate::lexer::LexicalError;

#[derive(Clone, Debug, PartialEq)]
pub struct ParserErr {
    pub span: Span,
    pub message: String,
}

impl ParserErr {
    pub fn new(span: Span, message: impl Into<String>) -> ParserErr {
        ParserErr {
            span,
            message: message.into(),
        }
    }

    pub(crate) fn from_lexical_error(err: LexicalError) -> ParserErr {
        let span = match &err {
            LexicalError::UnexpectedToken(span) | LexicalError::InvalidLiteral { span, .. } => {
                *span
            }
            LexicalError::Internal(_) => Span::default(),
        };
        ParserErr::new(span, format!("lex error: {err}"))
    }
}

impl Display for ParserErr {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.span.begin, self.message)
    }
}

impl std::error::Error for ParserErr {}
