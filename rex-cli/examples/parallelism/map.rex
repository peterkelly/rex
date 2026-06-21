import test (do_something);

fn make_list (from: i32) -> (to: i32) -> List i32 =
    if from >= to then
        []
    else
        from :: (make_list (from + 1) to);

let
    items = make_list 0 10
in
    length (map do_something items)
