class Pretty a
    pretty : a -> string

instance Pretty i32
    pretty = \_ -> "<i32>"

instance Pretty (List a) <= Pretty a
    pretty = \xs ->
        let
            step = \out x ->
                if out == "["
                    then out + pretty x
                    else out + ", " + pretty x,
            out = foldl step "[" xs
        in
            out + "]"

(pretty [1, 2, 3], pretty [])
