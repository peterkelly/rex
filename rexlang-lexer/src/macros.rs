#[macro_export]
macro_rules! span {
    () => {
        $crate::span::Span::default()
    };

    ($begin_ln:literal : $begin_col:literal - $end_ln:literal : $end_col:literal) => {
        $crate::span::Span {
            begin: $crate::span::Position {
                line: $begin_ln,
                column: $begin_col,
            },
            end: $crate::span::Position {
                line: $end_ln,
                column: $end_col,
            },
        }
    };

    ($begin_ln:expr , $begin_col:expr, $end_ln:expr , $end_col:expr) => {
        $crate::span::Span {
            begin: $crate::span::Position {
                line: $begin_ln,
                column: $begin_col,
            },
            end: $crate::span::Position {
                line: $end_ln,
                column: $end_col,
            },
        }
    };
}
