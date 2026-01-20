type MyADT a b c = MyCtor1 | MyCtor2 a b | MyCtor3 { field1: c }

type MyOtherADT a b c = MyOtherCtor1 a b | MyOtherCtor2 a b | MyOtherCtor3 { field1: c } | MyOtherCtor4 { field1: c }

let
    v1 = MyCtor1,
    v2 = MyCtor2 1 2,
    v3 = MyCtor3 { field1 = 3 },

    v4 = MyOtherCtor1 "ay" "bee",
    v5 = MyOtherCtor2 "see" "dee",
    v6 = MyOtherCtor3 { field1 = "ee" },
    v7 = MyOtherCtor4 { field1 = "ef" }

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
            when MyCtor3 {field1} → field1,
        match v4
            when MyOtherCtor1 x y → x + y
            when MyOtherCtor2 _ _ → ""
            when MyOtherCtor3 {field1} → field1
            when MyOtherCtor4 {field1} → field1,
        match v5
            when MyOtherCtor1 _ _ → ""
            when MyOtherCtor2 x y → x + y
            when MyOtherCtor3 {field1} → field1
            when MyOtherCtor4 {field1} → field1,
        match v6
            when MyOtherCtor1 _ _ → ""
            when MyOtherCtor2 _ _ → ""
            when MyOtherCtor3 {field1} → field1
            when MyOtherCtor4 {field1} → field1,
        match v7
            when MyOtherCtor1 _ _ → ""      
            when MyOtherCtor2 _ _ → ""
            when MyOtherCtor3 {field1} → field1
            when MyOtherCtor4 {field1} → field1
    )
