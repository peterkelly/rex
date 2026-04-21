{- CLI example: subprocess + wait + stdout/stderr

Run:
  cargo run -p rex -- run rex/examples/cli_subprocess_capture.rex

This spawns a subprocess, waits for it to exit, then forwards its captured
stdout/stderr to the CLI stdout/stderr.
-}

import std.process
import std.io

let p = process.spawn { cmd = "sh", args = ["-c", "printf hi; printf err 1>&2; exit 7"] } in
let code = process.wait p in
let _ = io.write_all 1 (process.stdout p) in
let _ = io.write_all 2 (process.stderr p) in
code
