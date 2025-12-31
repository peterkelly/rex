type Box = Box { value: i32 }

let
    add: i32 → i32 → i32 = λ (x : i32) (y : i32) → x + y,
    mk_box: i32 → Box = λ (x : i32) → Box { value = x },
    unbox: Box → i32 = λ (b : Box) → (b~value) is i32,
    sum: List i32 → i32 = λ (xs : List i32) →
        match xs
            when [] → 0
            when x:xs → x + sum xs,
    pick: bool → i32 → i32 → i32 = λ (flag : bool) (a : i32) (b : i32) →
        if flag then a else b,
    use_dict: Dict i32 → i32 = λ (d : Dict i32) →
        match d
            when {a, b} → a + b
            when {a} → a
            when {} → 0,
    nested: bool → Dict i32 = λ (flag : bool) →
        let
            base = (pick flag 1 2) is i32,
            boxed: Box = mk_box base,
            list: List i32 = [base, base + 1, base + 2],
            dict = ({a = base, b = base + 10}) is Dict i32,
            total: i32 = sum list,
            from_dict = (use_dict dict) is i32
        in
            {v = unbox boxed, t = total, d = from_dict}
in
    let
        r1: Dict i32 = nested true,
        r2: Dict i32 = nested false,
        output: i32 = (match r1 when {v, t, d} → v + t + d) is i32,
        alt: i32 = (match r2 when {v, t, d} → v + t + d) is i32,
        opt = (Some output) is Option i32
    in
        match opt
            when Some x → x + alt
            when None → 0
