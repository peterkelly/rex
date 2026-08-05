class MyEq a where {
    eq : a -> a -> Bool;
}
class MyOrd a <= MyEq a where {
    my_cmp : a -> a -> i32;
}
type Color = Red | Green | Blue;

instance MyEq Color where {
    eq = \x y ->
        match x with {
            case Red ->
                let r = match y with { case Red -> true; case _ -> false; } in r;
            case Green ->
                let r = match y with { case Green -> true; case _ -> false; } in r;
            case Blue ->
                let r = match y with { case Blue -> true; case _ -> false; } in r;
        };
}
instance MyOrd Color <= MyEq Color where {
    my_cmp = \x y ->
        if eq x y then 0 else
        match x with {
            case Red -> -1;
            case Green -> if eq y Red then 1 else -1;
            case Blue -> 1;
        };
}
let
    a = eq Red Blue,
    b = eq Blue Blue,
    c = my_cmp Red Green,
    d = my_cmp Blue Red
in
    (a, b, c, d)
