let
    pair_with_self<a>: a -> (a, a) = \x -> (x, x),
    keep_first<a,b>: a -> b -> a = \x _ -> x,
    numbers: (i32, i32) = pair_with_self 7,
    words: (string, string) = pair_with_self "rex",
    picked: string = keep_first "name" 42
in
    (numbers, words, picked)
