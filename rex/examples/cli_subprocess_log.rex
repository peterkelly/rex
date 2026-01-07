{- CLI example: logging + Pretty

Run:
  cargo run -p rex -- run rex/examples/cli_subprocess_log.rex
  REX_LOG=rex_engine=debug cargo run -p rex -- run rex/examples/cli_subprocess_log.rex

Notes:
  - info/debug/warn/error return a rendered string (via Pretty a)
    and also emit a tracing log event at the corresponding level.
-}

declare fn error a -> string where Pretty a
declare fn warn a -> string where Pretty a
declare fn info a -> string where Pretty a
declare fn debug a -> string where Pretty a

type Process = Process i64

declare fn subprocess { cmd: string, args: List string } -> Process
declare fn wait Process -> Process
declare fn stdout Process -> Array u8

let _   = debug "spawning..." in
let p   = subprocess { cmd = "sh", args = ["-c", "printf hi"] } in
let _   = wait p in
let out = stdout p in
let msg = info out in
(msg, count out)
