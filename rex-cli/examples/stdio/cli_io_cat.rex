/* CLI example: monadic read_all + write_all

Run:
  echo -n "hello" | cargo run -p rex-cli --bin rex -- rex-cli/examples/stdio/cli_io_cat.rex

Notes:
  - read_all 0 builds an IO action that reads all bytes from stdin (fd 0).
  - write_all 1 builds an IO action that writes bytes to stdout (fd 1).
  - The CLI performs the top-level IO action.
*/

import std.io;

bind (\bytes -> io.write_all 1 bytes) (io.read_all 0)
