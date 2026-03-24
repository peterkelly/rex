{- CLI example: read_all + write_all

Run:
  echo -n "hello" | cargo run -p rex -- run rex/examples/cli_io_cat.rex

Notes:
  - read_all 0 reads all bytes from stdin (fd 0).
  - write_all 1 writes bytes to stdout (fd 1).
-}

import std.io

let bytes = io.read_all 0 in
let _ = io.write_all 1 bytes in
()
