class Default a where
    default : a

type Foo = Foo { x: i32, y: i32 } | Bar { z: f32 }

instance Default Foo where
    default = Bar { z = 0.0 }

instance Default (List a) where
    default = []

instance Default i32 where
    default = 0

fn reduce : (a -> a -> a) -> t a -> a where Foldable t, Default a =
    \f xs -> foldl f default xs

let 
    x: Foo = default,
    y = reduce (\acc x -> acc + x) [1, 2, 3, 4]
in
    (x, y)
