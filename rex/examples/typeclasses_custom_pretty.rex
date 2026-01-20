type Point = Point { x: i32, y: i32 }

instance Pretty Point
    pretty = \p -> "Point(" + pretty p.x + ", " + pretty p.y + ")"

(
    pretty [Point { x = 1, y = 2 }, Point { x = 3, y = 4 }],
    pretty [1, 2, 3],
    pretty []
)
