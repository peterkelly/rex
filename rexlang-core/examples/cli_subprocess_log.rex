{- CLI example: logging + Show

Run:
  cargo run -p rexlang-cli -- run rexlang-core/examples/cli_subprocess_log.rex
  REX_LOG=rex_engine=debug cargo run -p rexlang-cli -- run rexlang-core/examples/cli_subprocess_log.rex

Notes:
  - info/debug/warn/error return a rendered string (via Show a)
    and also emit a tracing log event at the corresponding level.
-}

import std.io
import std.process

let _ = io.debug "spawning..." in
let p = process.spawn { cmd = "sh", args = ["-c", "printf hi"] } in
let _ = process.wait p in
let out = process.stdout p in
let msg = io.info out in
(msg, count out)
