type Foo = Foo { x: i32, y: i32 } | Bar { z: f32 };

instance Default Foo where {
    default = Bar { z = 0.0 };
}
fn reduce<a,t> f: (a -> a -> a) -> xs: t a -> a where Foldable t, Default a =
    foldl f default xs;

let 
    x: Foo = default,
    y: i32 = reduce (\acc x -> acc + x) [1, 2, 3, 4]
in
    (x, y)
