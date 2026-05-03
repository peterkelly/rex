
{- JSON values and typeclass-based conversion.

   Note: Rex class method names are global, so we intentionally use
   encode_json/decode_json as the method names and expose
   module-scoped to_json/from_json wrappers.
-}

pub type Value
    = Null
    | Bool bool
    | String string
    | Number f64
    | Array (Array Value)
    | Object (Dict Value);

pub type DecodeError = DecodeError { message: string };

pub class EncodeJson a where {
    encode_json : a -> Value;
}
pub class DecodeJson a where {
    decode_json : Value -> Result a DecodeError;
}
pub fn to_json : a -> Value where EncodeJson a
    = encode_json;

pub fn from_json : Value -> Result a DecodeError where DecodeJson a
    = decode_json;

pub fn stringify : Value -> string
    = prim_json_stringify;

pub fn parse : string -> Result Value DecodeError
    = (\s ->
        match (prim_json_parse s) with {
            when Ok v -> Ok v;
            when Err msg -> Err (DecodeError { message = msg });
        }
      );

instance Show Value where {
    show = stringify;
}
fn fail : string -> Result a DecodeError
    = \msg -> Err (DecodeError { message = msg });

fn kind : Value -> string
    = (\v -> match v with {
        when Null -> "null";
        when Bool _ -> "bool";
        when String _ -> "string";
        when Number _ -> "number";
        when Array _ -> "array";
        when Object _ -> "object";
    }
      );

fn expected : string -> Value -> DecodeError
    = \want got -> DecodeError { message = "expected " + want + ", got " + kind got };

instance EncodeJson Value where {
    encode_json = \v -> v;
}
instance DecodeJson Value where {
	    decode_json = \v -> Ok v;
	}
instance EncodeJson bool where {
    encode_json = \b -> Bool b;
}
instance DecodeJson bool where {
	    decode_json = \v ->
	        match v with {
	            when Bool b -> Ok b;
	            when _ -> Err (expected "bool" v);
            };
	}
instance EncodeJson string where {
    encode_json = \s -> String s;
}
instance DecodeJson string where {
	    decode_json = \v ->
	        match v with {
	            when String s -> Ok s;
	            when _ -> Err (expected "string" v);
            };
	}
instance EncodeJson f64 where {
    encode_json = \n -> Number n;
}
instance DecodeJson f64 where {
	    decode_json = \v ->
	        match v with {
	            when Number n -> Ok n;
	            when _ -> Err (expected "number" v);
            };
	}
instance EncodeJson f32 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson f32 where {
	    decode_json = \v ->
	        match v with {
	            when Number n -> (
	                match (prim_f64_to_f32 n) with {
	                    when Some x -> Ok x;
	                    when None -> fail "expected finite f64 representable as f32";
	                }
	              );
	            when _ -> Err (expected "number" v);
            };
	}
instance EncodeJson u8 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson u8 where {
	    decode_json = \v ->
	        match v with {
	            when Number n -> (
	                match (prim_f64_to_u8 n) with {
	                    when Some x -> Ok x;
	                    when None -> fail "expected integer number representable as u8";
	                }
	              );
	            when _ -> Err (expected "number" v);
            };
	}
instance EncodeJson u16 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson u16 where {
	    decode_json = \v ->
	        match v with {
	            when Number n -> (
	                match (prim_f64_to_u16 n) with {
	                    when Some x -> Ok x;
	                    when None -> fail "expected integer number representable as u16";
                    }
              );
            when _ -> Err (expected "number" v);
        };
	}
instance EncodeJson u32 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson u32 where {
	    decode_json = \v ->
	        match v with {
            when Number n -> (
                match (prim_f64_to_u32 n) with {
                    when Some x -> Ok x;
                    when None -> fail "expected integer number representable as u32";
                }
              );
            when _ -> Err (expected "number" v);
        };
	}
instance EncodeJson u64 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson u64 where {
	    decode_json = \v ->
	        match v with {
            when Number n -> (
                match (prim_f64_to_u64 n) with {
                    when Some x -> Ok x;
                    when None -> fail "expected integer number representable as u64";
                }
              );
            when _ -> Err (expected "number" v);
        };
	}
instance EncodeJson i8 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson i8 where {
	    decode_json = \v ->
	        match v with {
            when Number n -> (
                match (prim_f64_to_i8 n) with {
                    when Some x -> Ok x;
                    when None -> fail "expected integer number representable as i8";
                }
              );
            when _ -> Err (expected "number" v);
        };
	}
instance EncodeJson i16 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson i16 where {
	    decode_json = \v ->
	        match v with {
            when Number n -> (
                match (prim_f64_to_i16 n) with {
                    when Some x -> Ok x;
                    when None -> fail "expected integer number representable as i16";
                }
              );
            when _ -> Err (expected "number" v);
        };
	}
instance EncodeJson i32 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson i32 where {
	    decode_json = \v ->
	        match v with {
            when Number n -> (
                match (prim_f64_to_i32 n) with {
                    when Some x -> Ok x;
                    when None -> fail "expected integer number representable as i32";
                }
              );
            when _ -> Err (expected "number" v);
        };
	}
instance EncodeJson i64 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson i64 where {
	    decode_json = \v ->
	        match v with {
            when Number n -> (
                match (prim_f64_to_i64 n) with {
                    when Some x -> Ok x;
                    when None -> fail "expected integer number representable as i64";
                }
              );
            when _ -> Err (expected "number" v);
        };
	}
instance EncodeJson uuid where {
    encode_json = \u -> String (show u);
}
instance DecodeJson uuid where {
	    decode_json = \v ->
	        match v with {
            when String s -> (
                match (prim_parse_uuid s) with {
                    when Some u -> Ok u;
                    when None -> fail "expected uuid string";
                }
              );
            when _ -> Err (expected "string" v);
        };
	}
instance EncodeJson datetime where {
    encode_json = \d -> String (show d);
}
instance DecodeJson datetime where {
	    decode_json = \v ->
	        match v with {
            when String s -> (
                match (prim_parse_datetime s) with {
                    when Some d -> Ok d;
                    when None -> fail "expected RFC3339 datetime string";
                }
              );
            when _ -> Err (expected "string" v);
        };
	}
instance EncodeJson (Option a) <= EncodeJson a where {
    encode_json = \opt ->
        match opt with {
            when Some x -> to_json x;
            when None -> Null;
        };
}
instance DecodeJson (Option a) <= DecodeJson a where {
	    decode_json = \v ->
	        match v with {
	            when Null -> Ok None;
	            when _ ->
	                match (from_json v) with {
	                    when Ok x -> Ok (Some x);
	                    when Err e -> Err e;
                    };
            };
	}
instance EncodeJson (Result a e) <= EncodeJson a, EncodeJson e where {
	    encode_json = \r ->
	        match r with {
	            when Ok x -> Object { ok = to_json x };
	            when Err e0 -> Object { err = to_json e0 };
            };
	}
instance DecodeJson (Result a e) <= DecodeJson a, DecodeJson e where {
	    decode_json = \v ->
	        match v with {
	            when Object d -> (
	                match d with {
	                    when {ok, err} -> fail "expected object with exactly one of {ok} or {err}";
	                    when {ok} -> (
	                        match (from_json ok) with {
	                            when Ok x -> Ok (Ok x);
	                            when Err e -> Err e;
                            }
	                      );
	                    when {err} -> (
	                        match (from_json err) with {
	                            when Ok e0 -> Ok (Err e0);
	                            when Err e -> Err e;
                            }
	                      );
	                    when {} -> fail "expected object with {ok} or {err}";
                    }
	              );
	            when _ -> Err (expected "object" v);
            };
	}
instance EncodeJson (List a) <= EncodeJson a where {
    encode_json = \xs ->
        Array (prim_array_from_list (map (\x -> to_json x) xs));
}
instance DecodeJson (List a) <= DecodeJson a where {
	    decode_json = \v ->
	        match v with {
	            when Array xs ->
	                let step = \x acc -> match acc with {
	                        when Err e -> Err e;
	                        when Ok out ->
	                            match (from_json x) with {
	                                when Err e2 -> Err e2;
	                                when Ok y -> Ok (Cons y out);
                                };
                        }
	                in
	                    foldr step (Ok []) xs;
	            when _ -> Err (expected "array" v);
            };
	}
instance EncodeJson (Array a) <= EncodeJson a where {
    encode_json = \xs -> Array (map (\x -> to_json x) xs);
}
instance DecodeJson (Array a) <= DecodeJson a where {
	    decode_json = \v ->
	        match v with {
	            when Array xs ->
	                let step = \x acc -> match acc with {
	                        when Err e -> Err e;
	                        when Ok out ->
	                            match (from_json x) with {
	                                when Err e2 -> Err e2;
	                                when Ok y -> Ok (Cons y out);
                                };
                        }
	                in
	                    (
                        match (foldr step (Ok []) xs) with {
                            when Err e -> Err e;
                            when Ok ys -> Ok (prim_array_from_list ys);
                        }
                    );
            when _ -> Err (expected "array" v);
        };
	}
instance EncodeJson (Dict a) <= EncodeJson a where {
    encode_json = \d -> Object (prim_dict_map (\x -> to_json x) d);
}
instance DecodeJson (Dict a) <= DecodeJson a where {
	    decode_json = \v ->
	        match v with {
	            when Object d -> prim_dict_traverse_result (\x -> from_json x) d;
	            when _ -> Err (expected "object" v);
            };
	}
