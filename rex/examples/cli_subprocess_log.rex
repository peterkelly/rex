{- CLI example: logging + Pretty

Run:
  cargo run -p rex -- run rex/examples/cli_subprocess_log.rex

Notes:
  - info/debug/warn/error return a rendered string (via Pretty a)
    and also emit a tracing log event at the corresponding level.
-}

let _ = debug "spawning..." in
let p = subprocess { cmd = "sh", args = ["-c", "printf hi"] } in
let _ = wait p in
let out = stdout p in
let msg = info out in
(msg, count out)
