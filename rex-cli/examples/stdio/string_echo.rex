/* CLI example: string stdin/stdout

Run:
  echo -n "hello" | cargo run -p rex-cli --bin rex -- rex-cli/examples/stdio/string_echo.rex

This uses the string-oriented std.io functions instead of the byte/fd helpers.
*/

import std.io;

bind (\text -> io.write_stdout ("echo: " + text)) io.read_stdin
