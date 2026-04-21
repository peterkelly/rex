use std::fmt::{self, Display, Formatter};

use rex_lexer::span::Span;

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
}

impl Display for ParserErr {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.span.begin, self.message)
    }
}
