{- CLI example: counting bytes from stdin

Run:
  echo -n "hello" | cargo run -p rex -- run rex/examples/cli_io_count_stdin.rex
-}

declare fn read_all fd: i32 -> Array u8

let bytes = read_all 0 in
count bytes
