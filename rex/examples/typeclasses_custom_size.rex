class Size a
    size : a -> i32

instance Size (List t)
    size = \xs -> foldl (\acc _ -> acc + 1) 0 xs

(size [1, 2, 3], size [], size [42])
