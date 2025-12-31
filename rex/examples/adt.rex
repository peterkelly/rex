type MyADT a b c = MyCtor1 | MyCtor2 a b | MyCtor3 { field1: c }

let
    v1 = MyCtor1,
    v2 = MyCtor2 1 2,
    v3 = MyCtor3 { field1 = 3 }
in
    (
        match v1
            when MyCtor1 → 0
            when MyCtor2 _ _ → 1
            when MyCtor3 {field1} → field1,
        match v2
            when MyCtor1 → 0
            when MyCtor2 x y → x + y
            when MyCtor3 {field1} → field1,
        match v3
            when MyCtor1 → 0
            when MyCtor2 _ _ → 1
            when MyCtor3 {field1} → field1
    )
