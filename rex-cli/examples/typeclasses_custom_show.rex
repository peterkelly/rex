type Point = Point { x: i32, y: i32 }

instance Show Point
    show = \p -> "Point(" + show p.x + ", " + show p.y + ")"

(
    show [Point { x = 1, y = 2 }, Point { x = 3, y = 4 }],
    show [1, 2, 3],
    show []
)
