let
    inc = \x -> x + 1,
    a = map inc (Some 1),
    b = ap (Some inc) (Some 2),
    c = ap (None is Option (i32 -> i32)) (Some 3),
    d = pure 4 is Option i32
in
    (a, b, c, d)
