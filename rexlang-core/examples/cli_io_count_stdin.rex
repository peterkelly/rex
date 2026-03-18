{- CLI example: counting bytes from stdin

Run:
  echo -n "hello" | cargo run -p rexlang-cli -- run rexlang-core/examples/cli_io_count_stdin.rex
-}

import std.io

let bytes = io.read_all 0 in
count bytes
