
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
            case Ok v -> Ok v;
            case Err msg -> Err (DecodeError { message = msg });
        }
      );

instance Show Value where {
    show = stringify;
}
fn fail : string -> Result a DecodeError
    = \msg -> Err (DecodeError { message = msg });

fn kind : Value -> string
    = (\v -> match v with {
        case Null -> "null";
        case Bool _ -> "bool";
        case String _ -> "string";
        case Number _ -> "number";
        case Array _ -> "array";
        case Object _ -> "object";
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
	            case Bool b -> Ok b;
	            case _ -> Err (expected "bool" v);
            };
	}
instance EncodeJson string where {
    encode_json = \s -> String s;
}
instance DecodeJson string where {
	    decode_json = \v ->
	        match v with {
	            case String s -> Ok s;
	            case _ -> Err (expected "string" v);
            };
	}
instance EncodeJson f64 where {
    encode_json = \n -> Number n;
}
instance DecodeJson f64 where {
	    decode_json = \v ->
	        match v with {
	            case Number n -> Ok n;
	            case _ -> Err (expected "number" v);
            };
	}
instance EncodeJson f32 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson f32 where {
	    decode_json = \v ->
	        match v with {
	            case Number n -> (
	                match (prim_f64_to_f32 n) with {
	                    case Some x -> Ok x;
	                    case None -> fail "expected finite f64 representable as f32";
	                }
	              );
	            case _ -> Err (expected "number" v);
            };
	}
instance EncodeJson u8 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson u8 where {
	    decode_json = \v ->
	        match v with {
	            case Number n -> (
	                match (prim_f64_to_u8 n) with {
	                    case Some x -> Ok x;
	                    case None -> fail "expected integer number representable as u8";
	                }
	              );
	            case _ -> Err (expected "number" v);
            };
	}
instance EncodeJson u16 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson u16 where {
	    decode_json = \v ->
	        match v with {
	            case Number n -> (
	                match (prim_f64_to_u16 n) with {
	                    case Some x -> Ok x;
	                    case None -> fail "expected integer number representable as u16";
                    }
              );
            case _ -> Err (expected "number" v);
        };
	}
instance EncodeJson u32 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson u32 where {
	    decode_json = \v ->
	        match v with {
            case Number n -> (
                match (prim_f64_to_u32 n) with {
                    case Some x -> Ok x;
                    case None -> fail "expected integer number representable as u32";
                }
              );
            case _ -> Err (expected "number" v);
        };
	}
instance EncodeJson u64 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson u64 where {
	    decode_json = \v ->
	        match v with {
            case Number n -> (
                match (prim_f64_to_u64 n) with {
                    case Some x -> Ok x;
                    case None -> fail "expected integer number representable as u64";
                }
              );
            case _ -> Err (expected "number" v);
        };
	}
instance EncodeJson i8 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson i8 where {
	    decode_json = \v ->
	        match v with {
            case Number n -> (
                match (prim_f64_to_i8 n) with {
                    case Some x -> Ok x;
                    case None -> fail "expected integer number representable as i8";
                }
              );
            case _ -> Err (expected "number" v);
        };
	}
instance EncodeJson i16 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson i16 where {
	    decode_json = \v ->
	        match v with {
            case Number n -> (
                match (prim_f64_to_i16 n) with {
                    case Some x -> Ok x;
                    case None -> fail "expected integer number representable as i16";
                }
              );
            case _ -> Err (expected "number" v);
        };
	}
instance EncodeJson i32 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson i32 where {
	    decode_json = \v ->
	        match v with {
            case Number n -> (
                match (prim_f64_to_i32 n) with {
                    case Some x -> Ok x;
                    case None -> fail "expected integer number representable as i32";
                }
              );
            case _ -> Err (expected "number" v);
        };
	}
instance EncodeJson i64 where {
    encode_json = \n -> Number (prim_to_f64 n);
}
instance DecodeJson i64 where {
	    decode_json = \v ->
	        match v with {
            case Number n -> (
                match (prim_f64_to_i64 n) with {
                    case Some x -> Ok x;
                    case None -> fail "expected integer number representable as i64";
                }
              );
            case _ -> Err (expected "number" v);
        };
	}
instance EncodeJson uuid where {
    encode_json = \u -> String (show u);
}
instance DecodeJson uuid where {
	    decode_json = \v ->
	        match v with {
            case String s -> (
                match (prim_parse_uuid s) with {
                    case Some u -> Ok u;
                    case None -> fail "expected uuid string";
                }
              );
            case _ -> Err (expected "string" v);
        };
	}
instance EncodeJson datetime where {
    encode_json = \d -> String (show d);
}
instance DecodeJson datetime where {
	    decode_json = \v ->
	        match v with {
            case String s -> (
                match (prim_parse_datetime s) with {
                    case Some d -> Ok d;
                    case None -> fail "expected RFC3339 datetime string";
                }
              );
            case _ -> Err (expected "string" v);
        };
	}
instance EncodeJson (Option a) <= EncodeJson a where {
    encode_json = \opt ->
        match opt with {
            case Some x -> to_json x;
            case None -> Null;
        };
}
instance DecodeJson (Option a) <= DecodeJson a where {
	    decode_json = \v ->
	        match v with {
	            case Null -> Ok None;
	            case _ ->
	                match (from_json v) with {
	                    case Ok x -> Ok (Some x);
	                    case Err e -> Err e;
                    };
            };
	}
instance EncodeJson (Result a e) <= EncodeJson a, EncodeJson e where {
	    encode_json = \r ->
	        match r with {
	            case Ok x -> Object { ok = to_json x };
	            case Err e0 -> Object { err = to_json e0 };
            };
	}
instance DecodeJson (Result a e) <= DecodeJson a, DecodeJson e where {
	    decode_json = \v ->
	        match v with {
	            case Object d -> (
	                match d with {
	                    case {ok, err} -> fail "expected object with exactly one of {ok} or {err}";
	                    case {ok} -> (
	                        match (from_json ok) with {
	                            case Ok x -> Ok (Ok x);
	                            case Err e -> Err e;
                            }
	                      );
	                    case {err} -> (
	                        match (from_json err) with {
	                            case Ok e0 -> Ok (Err e0);
	                            case Err e -> Err e;
                            }
	                      );
	                    case {} -> fail "expected object with {ok} or {err}";
                    }
	              );
	            case _ -> Err (expected "object" v);
            };
	}
instance EncodeJson (List a) <= EncodeJson a where {
    encode_json = \xs ->
        Array (prim_array_from_list (map (\x -> to_json x) xs));
}
instance DecodeJson (List a) <= DecodeJson a where {
	    decode_json = \v ->
	        match v with {
	            case Array xs ->
	                let step = \x acc -> match acc with {
	                        case Err e -> Err e;
	                        case Ok out ->
	                            match (from_json x) with {
	                                case Err e2 -> Err e2;
	                                case Ok y -> Ok (Cons y out);
                                };
                        }
	                in
	                    foldr step (Ok []) xs;
	            case _ -> Err (expected "array" v);
            };
	}
instance EncodeJson (Array a) <= EncodeJson a where {
    encode_json = \xs -> Array (map (\x -> to_json x) xs);
}
instance DecodeJson (Array a) <= DecodeJson a where {
	    decode_json = \v ->
	        match v with {
	            case Array xs ->
	                let step = \x acc -> match acc with {
	                        case Err e -> Err e;
	                        case Ok out ->
	                            match (from_json x) with {
	                                case Err e2 -> Err e2;
	                                case Ok y -> Ok (Cons y out);
                                };
                        }
	                in
	                    (
                        match (foldr step (Ok []) xs) with {
                            case Err e -> Err e;
                            case Ok ys -> Ok (prim_array_from_list ys);
                        }
                    );
            case _ -> Err (expected "array" v);
        };
	}
instance EncodeJson (Dict a) <= EncodeJson a where {
    encode_json = \d -> Object (prim_dict_map (\x -> to_json x) d);
}
instance DecodeJson (Dict a) <= DecodeJson a where {
	    decode_json = \v ->
	        match v with {
	            case Object d -> prim_dict_traverse_result (\x -> from_json x) d;
	            case _ -> Err (expected "object" v);
            };
	}
