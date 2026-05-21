import test (do_something);

fn make_list (from: i32) -> (to: i32) -> List i32 =
    if from >= to then
        []
    else
        from :: (make_list (from + 1) to);

fn custom_map<a, b> (f : a -> b) -> items : List a -> List b =
    match items with {
        case [] -> [];
        case (x :: xs) -> (f x) :: (custom_map f xs);
    };

let
    items = make_list 0 10
in
    count (custom_map do_something items)
