import test (do_something, is_even);

fn make_list (from: i32) -> (to: i32) -> List i32 =
    if from >= to then
        []
    else
        from :: (make_list (from + 1) to);

let
    items = make_list 0 10,
    keep_and_process = \x ->
        if is_even x then
            Some (do_something x)
        else
            None
in
    count (filter_map keep_and_process items)
