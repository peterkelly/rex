/* CLI example: write a string to stderr

Run:
  cargo run -p rex-cli --bin rex -- rex-cli/examples/stdio/stderr_message.rex
*/

import std.io;

io.write_stderr "this message was written to stderr"
