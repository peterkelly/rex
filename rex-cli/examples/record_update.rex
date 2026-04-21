type Foo = Bar { x: i32, y: i32, z: i32 }
type Sum = A { x: i32 } | B { x: i32 }

let
    foo: Foo = Bar { x = 1, y = 2, z = 3 },
    foo2 = { foo with { x = 6 } },
    sum: Sum = A { x = 1 },
    sum2 = match sum
        when A {x} -> { sum with { x = x + 1 } }
        when B {x} -> { sum with { x = x + 2 } }
in
    (foo2.x, match sum2 when A {x} -> x when B {x} -> x)
