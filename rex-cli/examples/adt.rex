type MyADT a b c = MyCtor1 | MyCtor2 a b | MyCtor3 { field1: c };

type MyOtherADT a b c = MyOtherCtor1 a b | MyOtherCtor2 a b | MyOtherCtor3 { field1: c } | MyOtherCtor4 { field1: c };

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
        match v1 with {
            case MyCtor1 → 0;
            case MyCtor2 _ _ → 1;
            case MyCtor3 {field1} → field1;
        },
        match v2 with {
            case MyCtor1 → 0;
            case MyCtor2 x y → x + y;
            case MyCtor3 {field1} → field1;
        },
        match v3 with {
            case MyCtor1 → 0;
            case MyCtor2 _ _ → 1;
            case MyCtor3 {field1} → field1;
        },
        match v4 with {
            case MyOtherCtor1 x y → x + y;
            case MyOtherCtor2 _ _ → "";
            case MyOtherCtor3 {field1} → field1;
            case MyOtherCtor4 {field1} → field1;
        },
        match v5 with {
            case MyOtherCtor1 _ _ → "";
            case MyOtherCtor2 x y → x + y;
            case MyOtherCtor3 {field1} → field1;
            case MyOtherCtor4 {field1} → field1;
        },
        match v6 with {
            case MyOtherCtor1 _ _ → "";
            case MyOtherCtor2 _ _ → "";
            case MyOtherCtor3 {field1} → field1;
            case MyOtherCtor4 {field1} → field1;
        },
        match v7 with {
            case MyOtherCtor1 _ _ → "";
            case MyOtherCtor2 _ _ → "";
            case MyOtherCtor3 {field1} → field1;
            case MyOtherCtor4 {field1} → field1;
        }
    )
