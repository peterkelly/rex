// Stress a deep lexical environment with many closures that retain it.
//
// Each closure produced by make_closure captures the environment at the bottom
// of the nested let chain, plus its own n parameter. Evaluating closures keeps
// many deep environment owners live at once, which is useful when profiling
// environment lookup and GC behavior.

fn range lo: i32 -> hi: i32 -> List i32 =
    if lo > hi then Empty else Cons lo (range (lo + 1) hi);

let seed: i32 = 0 in
let a001 = seed + 1 in
let a002 = a001 + 1 in
let a003 = a002 + 1 in
let a004 = a003 + 1 in
let a005 = a004 + 1 in
let a006 = a005 + 1 in
let a007 = a006 + 1 in
let a008 = a007 + 1 in
let a009 = a008 + 1 in
let a010 = a009 + 1 in
let a011 = a010 + 1 in
let a012 = a011 + 1 in
let a013 = a012 + 1 in
let a014 = a013 + 1 in
let a015 = a014 + 1 in
let a016 = a015 + 1 in
let a017 = a016 + 1 in
let a018 = a017 + 1 in
let a019 = a018 + 1 in
let a020 = a019 + 1 in
let a021 = a020 + 1 in
let a022 = a021 + 1 in
let a023 = a022 + 1 in
let a024 = a023 + 1 in
let a025 = a024 + 1 in
let a026 = a025 + 1 in
let a027 = a026 + 1 in
let a028 = a027 + 1 in
let a029 = a028 + 1 in
let a030 = a029 + 1 in
let a031 = a030 + 1 in
let a032 = a031 + 1 in
let a033 = a032 + 1 in
let a034 = a033 + 1 in
let a035 = a034 + 1 in
let a036 = a035 + 1 in
let a037 = a036 + 1 in
let a038 = a037 + 1 in
let a039 = a038 + 1 in
let a040 = a039 + 1 in
let a041 = a040 + 1 in
let a042 = a041 + 1 in
let a043 = a042 + 1 in
let a044 = a043 + 1 in
let a045 = a044 + 1 in
let a046 = a045 + 1 in
let a047 = a046 + 1 in
let a048 = a047 + 1 in
let a049 = a048 + 1 in
let a050 = a049 + 1 in
let a051 = a050 + 1 in
let a052 = a051 + 1 in
let a053 = a052 + 1 in
let a054 = a053 + 1 in
let a055 = a054 + 1 in
let a056 = a055 + 1 in
let a057 = a056 + 1 in
let a058 = a057 + 1 in
let a059 = a058 + 1 in
let a060 = a059 + 1 in
let a061 = a060 + 1 in
let a062 = a061 + 1 in
let a063 = a062 + 1 in
let a064 = a063 + 1 in
let a065 = a064 + 1 in
let a066 = a065 + 1 in
let a067 = a066 + 1 in
let a068 = a067 + 1 in
let a069 = a068 + 1 in
let a070 = a069 + 1 in
let a071 = a070 + 1 in
let a072 = a071 + 1 in
let a073 = a072 + 1 in
let a074 = a073 + 1 in
let a075 = a074 + 1 in
let a076 = a075 + 1 in
let a077 = a076 + 1 in
let a078 = a077 + 1 in
let a079 = a078 + 1 in
let a080 = a079 + 1 in
let a081 = a080 + 1 in
let a082 = a081 + 1 in
let a083 = a082 + 1 in
let a084 = a083 + 1 in
let a085 = a084 + 1 in
let a086 = a085 + 1 in
let a087 = a086 + 1 in
let a088 = a087 + 1 in
let a089 = a088 + 1 in
let a090 = a089 + 1 in
let a091 = a090 + 1 in
let a092 = a091 + 1 in
let a093 = a092 + 1 in
let a094 = a093 + 1 in
let a095 = a094 + 1 in
let a096 = a095 + 1 in
let make_closure: i32 -> i32 -> i32 =
    \n ->
        \x ->
            x + n
            + a001 + a016 + a032 + a048
            + a064 + a080 + a096
in
let nums: List i32 = range 1 5000 in
let closures: List (i32 -> i32) = map make_closure nums in
let results: List i32 = map (\f -> f 1) closures in
sum results
