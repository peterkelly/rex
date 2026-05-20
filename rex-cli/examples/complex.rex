type Box = Box { value: i32 };

type Tree 
    = Leaf { value: i32 }
    | Node { left: Tree, right: Tree };

fn foo x: i32 -> y: i32 -> i32 = x * 2;
fn bar x: i32 -> y: i32 -> i32 = x * 2;

let
    t = Node { 
        left = Node { 
            left = Leaf { value = 1 }, 
            right = Leaf { value = 2 } 
        }, 
        right = Leaf { value = 3 } 
    },
    v = match t with { case Leaf { value } -> value; case Node {} -> 0; },
    add = \ x y -> x + y,
    mk_box = \ x -> Box { value = x },
    unbox = \ b -> b.value,
    sum = \ xs ->
        match xs with {
            case [] -> 0;
            case x::xs -> x + sum xs;
        },
    pick = \ flag a b ->
        if flag then a else b,
    use_dict = \ d ->
        match d with {
            case {a, b} -> a + b;
            case {a} -> a;
            case {} -> 0;
        },
    nested = \ flag ->
        let
            base = pick flag 1 2,
            boxed = mk_box base,
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
        output = match r1 with { case {v, t, d} -> v + t + d; },
        alt = match r2 with { case {v, t, d} -> v + t + d; },
        opt = (Some output) is Option i32
    in
        match opt with {
            case Some x -> x + alt;
            case None -> foo 0 0;
        }
