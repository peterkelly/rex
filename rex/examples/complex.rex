type Box = Box { value: i32 }

let
    add = λ x y → x + y,
    mk_box = λ x → Box { value = x },
    unbox = λ b → b~value,
    sum = λ xs →
        match xs
            when [] → 0
            when x:xs → x + sum xs,
    pick = λ flag a b →
        if flag then a else b,
    use_dict = λ d →
        match d
            when {a, b} → a + b
            when {a} → a
            when {} → 0,
    nested = λ flag →
        let
            base = pick flag 1 2,
            boxed mk_box base,
            list = [base, base + 1, base + 2],
            dict = ({a = base, b = base + 10}) is Dict i32,
            total = sum list,
            from_dict = use_dict dict
        in
            {v = unbox boxed, t = total, d = from_dict}
in
    let
        r1 = nested true,
        r2 = nested false,
        output = match r1 when {v, t, d} → v + t + d,
        alt = match r2 when {v, t, d} → v + t + d,
        opt = (Some output) is Option i32
    in
        match opt
            when Some x → x + alt
            when None → 0
