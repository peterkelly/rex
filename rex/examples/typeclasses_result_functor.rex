class Functor f where
    fmap : (a -> b) -> f a -> f b

instance Functor (Result e) where
    fmap = \f x ->
        match x
            when Ok v -> Ok (f v)
            when Err err -> Err err

let
    inc = \x -> x + 1,
    ok = (Ok 1) is Result string i32,
    bad = (Err "bad") is Result string i32
in
    (fmap inc ok, fmap inc bad)

