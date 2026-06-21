/* CLI example: logging + Show

Run:
  cargo run -p rex-cli --bin rex -- rex-cli/examples/stdio/cli_subprocess_log.rex
  REX_LOG=debug cargo run -p rex-cli --bin rex -- rex-cli/examples/stdio/cli_subprocess_log.rex

Notes:
  - info/debug/warn/error accept and return strings, and also emit a
    tracing log event at the corresponding level.
*/

import std.io;
import std.process;

let p = process.spawn (process.SpawnOptions {
  cmd = "sh",
  args = ["-c", "printf hi"]
}) in
let _ = process.wait p in
let out = process.stdout p in
bind (\_ ->
  bind (\msg -> pure (msg, count out))
       (io.info (show out)))
     (io.debug "spawning...")
