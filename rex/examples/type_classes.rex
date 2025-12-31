let
    use_classes = \ (x: t) (y: f a) (z: a) where Indexable (t, a), Foldable f ->
        let
            first = get 0 x,
            total = foldl (\acc _ -> acc) z y
        in
            (first, total, z)
in
    use_classes [10, 20, 30] [1, 2, 3] 0
