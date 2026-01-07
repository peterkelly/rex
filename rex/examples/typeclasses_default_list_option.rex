class Default a where
    default : a

type Point = Point { x: i32, y: i32 }

instance Default i32 where
    default = 0

instance Default Point where
    default = Point { x = default, y = default }

instance Default (List a) <= Default a where
    default = [default, default]

instance Default (Option a) <= Default a where
    default = Some default

fn new_point : i32 -> Point = \x -> Point { x = x, y = default }

let
    p: Point = new_point 5,
    xs: List i32 = default,
    o: Option Point = default
in
    (p, xs, o)
