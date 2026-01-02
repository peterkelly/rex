let
    inc = \x -> x + 1,
    ok = (Ok 1) is Result string i32,
    bad = (Err "bad") is Result string i32
in
    (map inc ok, map inc bad)
