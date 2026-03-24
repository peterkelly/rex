//! The Span module defines source positions, and ranges of source positions,
//! in which nodes can be defined. It is used to link nodes back to their
//! origins in the source.

use std::fmt::{self, Display, Formatter};

/// A Position represents an arbitrary source position. It includes the line
/// number, and column number.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "lowercase")]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    /// Create a new Position.
    ///
    /// # Arguments
    /// * `line` The line number, where the first line of the file is at zero.
    /// * `column` The column number, where the first column of a line is at
    ///   zero.
    ///
    /// # Return
    /// A new Position.
    pub fn new(line: usize, column: usize) -> Position {
        Position { line, column }
    }

    /// # Return
    /// The line number of the Position.
    pub fn line(&self) -> usize {
        self.line
    }

    /// # Return
    /// The column number of the Position.
    pub fn column(&self) -> usize {
        self.column
    }
}

impl fmt::Display for Position {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "{}:{}", self.line, self.column)
    }
}

/// A Span represents an arbitrary source range. It includes the beginning and
/// ending Positions.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "lowercase")]
pub struct Span {
    pub begin: Position,
    pub end: Position,
}

impl Span {
    /// Create a new Span. The beginning and ending Positions will also be
    /// created.
    ///
    /// # Arguments
    /// * `begin_line` The line number for the beginning of the Span, where the
    ///   first line of the file is at zero.
    /// * `begin_column` The column number for the beginning of the Span, where
    ///   the first column of a line is at zero.
    /// * `end_line` The line number for the ending of the Span, where the first
    ///   line of the file is at zero.
    /// * `end_column` The column number for the ending of the Span, where the
    ///   first column of a line is at zero.
    ///
    /// # Return
    /// A new Span, with new beginning and ending Positions.
    pub fn new(begin_line: usize, begin_column: usize, end_line: usize, end_column: usize) -> Self {
        Self {
            begin: Position::new(begin_line, begin_column),
            end: Position::new(end_line, end_column),
        }
    }

    /// Create a new Span from beginning and ending positions.
    ///
    /// # Arguments
    /// * `begin` The beginning position.
    /// * `end` The ending position.
    ///
    /// # Return
    /// A new Span, with the beginning and ending Positions.
    pub fn from_begin_end<Pos>(begin: Pos, end: Pos) -> Self
    where
        Pos: Into<Position>,
    {
        Self {
            begin: begin.into(),
            end: end.into(),
        }
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            begin: self.begin,
            end: other.end,
        }
    }
}

impl Default for Span {
    fn default() -> Self {
        Self {
            begin: Position::new(usize::MIN, usize::MIN),
            end: Position::new(usize::MAX, usize::MAX),
        }
    }
}

impl Display for Span {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{} - {}", self.begin, self.end)
    }
}

pub trait Spanned {
    fn set_span_begin(&mut self, begin: Position) {
        self.span_mut().begin = begin;
    }
    fn set_span_end(&mut self, end: Position) {
        self.span_mut().end = end;
    }
    fn span(&self) -> &Span;
    fn span_mut(&mut self) -> &mut Span;
}
