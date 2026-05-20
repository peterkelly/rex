type Point = Point { x: i32, y: i32 };

instance Default Point where {
    default = Point { x = default, y = default };
}
fn new_point x: i32 -> Point = Point { x = x, y = default };

let
    p: Point = new_point 5,
    xs: List i32 = default,
    o: Option Point = default
in
    (p, xs, o)
