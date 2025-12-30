let
    id = \x -> x,
    add = \x y -> x + y,
    xs = [1, 2, 3],
    ys = map (\x -> x + 1) xs,
    total = sum ys,
    safeDiv = \a b -> if b == 0.0 then None else Some (a / b),
    noneToZero = \x -> match x when None -> 0.0 when Some y -> y,
    res = safeDiv 10.0 2.0,
    res2 = safeDiv 10.0 0.0,
    a = noneToZero res,
    b = noneToZero res2,
    listHead = match xs when [] -> 0 when x:xs2 -> x,
    classify = \n -> if n < 2 then Err n else Ok n,
    mapped = map classify xs,
    filtered = filter_map (\x -> match x when Ok v -> Some v when Err _ -> None) mapped,
    countOk = count filtered
in
    (id (add 40 2), total, a, b, listHead, countOk)
