/* CLI example: write, append, read, and remove a file

Run:
  cargo run -p rex-cli --bin rex -- rex-cli/examples/stdio/file_roundtrip.rex
*/

import std.io;

let path = "/tmp/rex-stdio-roundtrip.txt" in
bind (\_ ->
  bind (\_ ->
    bind (\contents ->
      bind (\_ -> pure contents)
           (io.remove_file path))
         (io.read_file path))
       (io.append_file path "world"))
     (io.write_file path "hello ")
