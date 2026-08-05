fn sum3<a> x: a -> y: a -> z: a -> a where AdditiveMonoid a =
    x + y + z;

fn choose<a,b> left: a -> right: b -> a =
    left;

let
    total: f32 = sum3 10.0 20.0 12.0,
    picked: String = choose "left" 99
in
    (total, picked)
