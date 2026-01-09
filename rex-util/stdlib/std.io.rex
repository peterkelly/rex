
pub declare fn debug (x: a) -> string where Pretty a
pub declare fn info (x: a) -> string where Pretty a
pub declare fn warn (x: a) -> string where Pretty a
pub declare fn error (x: a) -> string where Pretty a

pub declare fn write_all (fd: i32) -> (contents: Array u8) -> ()
pub declare fn read_all (fd: i32) -> Array u8
