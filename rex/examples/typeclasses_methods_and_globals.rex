fn inc (x: i32) -> i32 = x + 1

class Bump a where
    bump : a -> a

instance Bump i32 where
    bump = inc

class Builder a where
    make_adder : a -> i32 -> i32

instance Builder i32 where
    make_adder = \n x -> x + n

let
    forty_two = bump 41,
    add5 = make_adder 5,
    also_forty_two = add5 37
in
    (forty_two, also_forty_two)

