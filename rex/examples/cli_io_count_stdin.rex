{- CLI example: counting bytes from stdin

Run:
  echo -n "hello" | cargo run -p rex -- run rex/examples/cli_io_count_stdin.rex
-}

let bytes = read_all 0 in
count bytes

