class Default a where
    default : a

type Foo = Foo { x: i32, y: i32 } | Bar { z: f32 }

instance Default Foo where
    default = Bar { z = 0.0 }

let 
    x: Foo = default
in
    x
