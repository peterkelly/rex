import test (do_something);

let
    items = ({a = 1, b = 2, c = 3, d = 4}) is Dict i32,
    mapped = map do_something items
in
    match mapped with {
        case {a, b, c, d} -> a + b + c + d;
    }
