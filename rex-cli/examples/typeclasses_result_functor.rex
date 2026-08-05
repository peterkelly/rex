	let
	    inc = \x -> x + 1,
	    ok = (Ok 1) is Result i32 String,
	    bad = (Err "bad") is Result i32 String
	in
	    (map inc ok, map inc bad)
