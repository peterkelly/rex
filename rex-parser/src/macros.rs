#[macro_export]
macro_rules! span {
    () => {
        ::rex_ast::Span::default()
    };

    ($begin_ln:literal : $begin_col:literal - $end_ln:literal : $end_col:literal) => {
        ::rex_ast::Span {
            begin: ::rex_ast::Position {
                line: $begin_ln,
                column: $begin_col,
            },
            end: ::rex_ast::Position {
                line: $end_ln,
                column: $end_col,
            },
        }
    };

    ($begin_ln:expr , $begin_col:expr, $end_ln:expr , $end_col:expr) => {
        ::rex_ast::Span {
            begin: ::rex_ast::Position {
                line: $begin_ln,
                column: $begin_col,
            },
            end: ::rex_ast::Position {
                line: $end_ln,
                column: $end_col,
            },
        }
    };
}
