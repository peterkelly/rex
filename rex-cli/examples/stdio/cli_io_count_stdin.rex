/* CLI example: counting bytes from stdin

Run:
  echo -n "hello" | cargo run -p rex-cli --bin rex -- rex-cli/examples/stdio/cli_io_count_stdin.rex
*/

import std.io;

map length (io.read_all 0)
