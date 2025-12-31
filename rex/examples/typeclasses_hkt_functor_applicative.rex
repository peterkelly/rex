class Functor f where
    fmap : (a -> b) -> f a -> f b

class Applicative f <= Functor f where
    pure : a -> f a
    ap : f (a -> b) -> f a -> f b

instance Functor Option where
    fmap = \f x ->
        match x
            when Some v -> Some (f v)
            when None -> None

instance Applicative Option <= Functor Option where
    pure = \x -> Some x
    ap = \ff xx ->
        match ff
            when Some f -> fmap f xx
            when None -> None

let
    inc = \x -> x + 1,
    a = fmap inc (Some 1),
    b = ap (Some inc) (Some 2),
    c = ap (None is Option (i32 -> i32)) (Some 3),
    d = pure 4 is Option i32
in
    (a, b, c, d)

