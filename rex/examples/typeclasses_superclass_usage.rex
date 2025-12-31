class MyEq a where
    eq : a -> a -> bool

class MyOrd a <= MyEq a where
    cmp : a -> a -> i32

type Color = Red | Green | Blue

instance MyEq Color where
    eq = \x y ->
        match x
            when Red ->
                let r = match y when Red -> true when _ -> false in r
            when Green ->
                let r = match y when Green -> true when _ -> false in r
            when Blue ->
                let r = match y when Blue -> true when _ -> false in r

instance MyOrd Color <= MyEq Color where
    cmp = \x y ->
        if eq x y then 0 else
        match x
            when Red -> -1
            when Green -> if eq y Red then 1 else -1
            when Blue -> 1

let
    a = eq Red Blue,
    b = eq Blue Blue,
    c = cmp Red Green,
    d = cmp Blue Red
in
    (a, b, c, d)
