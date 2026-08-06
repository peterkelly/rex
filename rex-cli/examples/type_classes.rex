
let
    use_classes<f,a> = \ (x: List a) (y: f a) (z: a) where Foldable f ->
        let
            first = unwrap (list_get 0 x),
            total = foldl (\acc _ -> acc) z y
        in
            (first, total, z)
in
    let result: (i32, i32, i32) = use_classes [10, 20, 30] [1, 2, 3] 0 in result
