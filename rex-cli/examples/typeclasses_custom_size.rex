class Size a where {
    size : a -> i32;
}
instance<t> Size (List t) where {
    size = \xs -> foldl (\acc _ -> acc + 1) 0 xs;
}
(size [1, 2, 3], size [], size [42])
