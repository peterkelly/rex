{- CLI example: subprocess + wait + stdout/stderr

Run:
  cargo run -p rex -- run rex/examples/cli_subprocess_capture.rex

This spawns a subprocess, waits for it to exit, then forwards its captured
stdout/stderr to the CLI stdout/stderr.
-}

let p = subprocess { cmd = "sh", args = ["-c", "printf hi; printf err 1>&2; exit 7"] } in
let code = wait p in
let _ = write_all 1 (stdout p) in
let _ = write_all 2 (stderr p) in
code
