class Head a where
    head_or : a -> List a -> a

instance Head i32 where
    head_or = \fallback xs ->
        match xs
            when [] -> fallback
            when x:rest -> x

let
    a = head_or 0 [1, 2, 3],
    b = head_or 7 []
in
    (a, b)

