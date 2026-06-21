/* CLI example: list entries in the current directory

Run:
  cargo run -p rex-cli --bin rex -- rex-cli/examples/stdio/list_current_dir.rex
*/

import std.io;

fn join_lines entries: List string -> string =
  foldl (\out y -> if out == "" then y else out + "\n" + y) "" entries;

bind (\cwd -> map join_lines (io.read_dir cwd)) io.current_dir
