	let
	    inc = \x -> x + 1,
	    ok = (Ok 1) is Result i32 string,
	    bad = (Err "bad") is Result i32 string
	in
	    (map inc ok, map inc bad)
