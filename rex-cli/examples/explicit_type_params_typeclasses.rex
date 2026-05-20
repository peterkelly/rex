type Box a = Box a;

class Container c where {
    put<a> : a -> c a;
    get_or<a> : a -> c a -> a;
}

instance Container Box where {
    put = \x -> Box x;
    get_or = \fallback box ->
        match box with {
            case Box x -> x;
        };
}

type Pair a = Pair a a;

class Total a where {
    total : a -> i32;
}

instance<a> Total (Pair a) where {
    total = \pair ->
        match pair with {
            case Pair _ _ -> 2;
        };
}

let
    boxed_number: Box i32 = put 41,
    boxed_word: Box string = put "rex",
    count: i32 = total (Pair "left" "right")
in
    (get_or 0 boxed_number + 1, get_or "" boxed_word, count)
