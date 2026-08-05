// prelude typeclasses and instances
//
// this file is parsed by rex-engine; rex-typesystem consumes the parsed declarations
//
// design note:
// - class methods define the public surface (the names users call)
// - instance bodies in this file attach those names to rust-backed prim_ intrinsics
// - prim_ is the only magic: it is the equivalent of haskell / ghc primops

// numeric hierarchy
class AdditiveMonoid a where {
    zero : a;
    + : a -> a -> a;
}

class MultiplicativeMonoid a where {
    one : a;
    * : a -> a -> a;
}

class Semiring a <= AdditiveMonoid a, MultiplicativeMonoid a;

class Subtractive a <= Semiring a where {
    - : a -> a -> a;
}

class AdditiveGroup a <= Subtractive a where {
    negate : a -> a;
}

class Ring a <= AdditiveGroup a, MultiplicativeMonoid a;

class Divisive a <= MultiplicativeMonoid a where {
    / : a -> a -> a;
}

class Field a <= Ring a, Divisive a;

class Integral a where {
    % : a -> a -> a;
}

// equality and ordering
class Eq a where {
    == : a -> a -> Bool;
    != : a -> a -> Bool;
}

class Ord a <= Eq a where {
    cmp : a -> a -> Ordering;
    < : a -> a -> Bool;
    <= : a -> a -> Bool;
    > : a -> a -> Bool;
    >= : a -> a -> Bool;
}

// show printing
class Show a where {
    show : a -> String;
}

// default values
class Default a where {
    default : a;
}

// collection combinators
class Functor f where {
    map<a,b> : (a -> b) -> f a -> f b;
}

class Applicative f <= Functor f where {
    pure<a> : a -> f a;
    ap<a,b> : f (a -> b) -> f a -> f b;
}

class Monad m <= Applicative m where {
    // Monad's core operation is "bind".
    //
    // We keep the argument order as (a -> m b) first, then (m a), to match
    // the rest of Rex's collection API (map f xs, filter p xs, ...) and to
    // map directly to the host intrinsic prim_flat_map without extra
    // wrappers/allocations.
    bind<a,b> : (a -> m b) -> m a -> m b;
}

class Foldable t where {
    foldl<a,b> : (b -> a -> b) -> b -> t a -> b;
    foldr<a,b> : (a -> b -> b) -> b -> t a -> b;
    fold<a,b> : (b -> a -> b) -> b -> t a -> b;
}

class Length a where {
    length : a -> u64;
}

class Filterable f <= Functor f where {
    filter<a> : (a -> Bool) -> f a -> f a;
    filter_map<a,b> : (a -> Option b) -> f a -> f b;
}

class Sequence f <= Functor f, Foldable f where {
    take<a> : u64 -> f a -> f a;
    skip<a> : u64 -> f a -> f a;
    zip<a,b> : f a -> f b -> f (a, b);
    unzip<a,b> : f (a, b) -> (f a, f b);
}

class Alternative f <= Applicative f where {
    or_else<a> : (f a -> f a) -> f a -> f a;
}

// Indexable needs two parameters: the container type and the element type.
class Indexable t a where {
    get : u64 -> t -> a;
}

// AdditiveMonoid instances
instance<a> AdditiveMonoid (List a) where {
    zero = [];
    + = \xs ys -> foldr (\x out -> x :: out) ys xs;
}

instance AdditiveMonoid String where {
    zero = prim_zero;
    + = prim_add;
}

instance AdditiveMonoid u8 where {
    zero = prim_zero;
    + = prim_add;
}

instance AdditiveMonoid u16 where {
    zero = prim_zero;
    + = prim_add;
}

instance AdditiveMonoid u32 where {
    zero = prim_zero;
    + = prim_add;
}

instance AdditiveMonoid u64 where {
    zero = prim_zero;
    + = prim_add;
}

instance AdditiveMonoid i8 where {
    zero = prim_zero;
    + = prim_add;
}

instance AdditiveMonoid i16 where {
    zero = prim_zero;
    + = prim_add;
}

instance AdditiveMonoid i32 where {
    zero = prim_zero;
    + = prim_add;
}

instance AdditiveMonoid i64 where {
    zero = prim_zero;
    + = prim_add;
}

instance AdditiveMonoid f32 where {
    zero = prim_zero;
    + = prim_add;
}

instance AdditiveMonoid f64 where {
    zero = prim_zero;
    + = prim_add;
}

// MultiplicativeMonoid instances
instance MultiplicativeMonoid u8 where {
    one = prim_one;
    * = prim_mul;
}

instance MultiplicativeMonoid u16 where {
    one = prim_one;
    * = prim_mul;
}

instance MultiplicativeMonoid u32 where {
    one = prim_one;
    * = prim_mul;
}

instance MultiplicativeMonoid u64 where {
    one = prim_one;
    * = prim_mul;
}

instance MultiplicativeMonoid i8 where {
    one = prim_one;
    * = prim_mul;
}

instance MultiplicativeMonoid i16 where {
    one = prim_one;
    * = prim_mul;
}

instance MultiplicativeMonoid i32 where {
    one = prim_one;
    * = prim_mul;
}

instance MultiplicativeMonoid i64 where {
    one = prim_one;
    * = prim_mul;
}

instance MultiplicativeMonoid f32 where {
    one = prim_one;
    * = prim_mul;
}

instance MultiplicativeMonoid f64 where {
    one = prim_one;
    * = prim_mul;
}

// Semiring instances
instance Semiring u8;

instance Semiring u16;

instance Semiring u32;

instance Semiring u64;

instance Semiring i8;

instance Semiring i16;

instance Semiring i32;

instance Semiring i64;

instance Semiring f32;

instance Semiring f64;

// Subtractive instances
instance Subtractive u8 <= Semiring u8 where {
    - = prim_sub;
}

instance Subtractive u16 <= Semiring u16 where {
    - = prim_sub;
}

instance Subtractive u32 <= Semiring u32 where {
    - = prim_sub;
}

instance Subtractive u64 <= Semiring u64 where {
    - = prim_sub;
}

instance Subtractive i8 <= Semiring i8 where {
    - = prim_sub;
}

instance Subtractive i16 <= Semiring i16 where {
    - = prim_sub;
}

instance Subtractive i32 <= Semiring i32 where {
    - = prim_sub;
}

instance Subtractive i64 <= Semiring i64 where {
    - = prim_sub;
}

instance Subtractive f32 <= Semiring f32 where {
    - = prim_sub;
}

instance Subtractive f64 <= Semiring f64 where {
    - = prim_sub;
}

// AdditiveGroup and Ring instances
instance AdditiveGroup i8 <= Subtractive i8 where {
    negate = prim_negate;
}

instance AdditiveGroup i16 <= Subtractive i16 where {
    negate = prim_negate;
}

instance AdditiveGroup i32 <= Subtractive i32 where {
    negate = prim_negate;
}

instance AdditiveGroup i64 <= Subtractive i64 where {
    negate = prim_negate;
}

instance AdditiveGroup f32 <= Subtractive f32 where {
    negate = prim_negate;
}

instance AdditiveGroup f64 <= Subtractive f64 where {
    negate = prim_negate;
}

instance Ring i8 <= AdditiveGroup i8;

instance Ring i16 <= AdditiveGroup i16;

instance Ring i32 <= AdditiveGroup i32;

instance Ring i64 <= AdditiveGroup i64;

instance Ring f32 <= AdditiveGroup f32;

instance Ring f64 <= AdditiveGroup f64;

// Divisive and Field instances
instance Divisive u8 <= MultiplicativeMonoid u8 where {
    / = prim_div;
}

instance Divisive u16 <= MultiplicativeMonoid u16 where {
    / = prim_div;
}

instance Divisive u32 <= MultiplicativeMonoid u32 where {
    / = prim_div;
}

instance Divisive u64 <= MultiplicativeMonoid u64 where {
    / = prim_div;
}

instance Divisive i8 <= MultiplicativeMonoid i8 where {
    / = prim_div;
}

instance Divisive i16 <= MultiplicativeMonoid i16 where {
    / = prim_div;
}

instance Divisive i32 <= MultiplicativeMonoid i32 where {
    / = prim_div;
}

instance Divisive i64 <= MultiplicativeMonoid i64 where {
    / = prim_div;
}

instance Divisive f32 <= MultiplicativeMonoid f32 where {
    / = prim_div;
}

instance Divisive f64 <= MultiplicativeMonoid f64 where {
    / = prim_div;
}

instance Field f32 <= Ring f32, Divisive f32;

instance Field f64 <= Ring f64, Divisive f64;

// Integral instances
instance Integral u8 where {
    % = prim_mod;
}

instance Integral u16 where {
    % = prim_mod;
}

instance Integral u32 where {
    % = prim_mod;
}

instance Integral u64 where {
    % = prim_mod;
}

instance Integral i8 where {
    % = prim_mod;
}

instance Integral i16 where {
    % = prim_mod;
}

instance Integral i32 where {
    % = prim_mod;
}

instance Integral i64 where {
    % = prim_mod;
}

// Eq instances
instance Eq u8 where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq u16 where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq u32 where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq u64 where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq i8 where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq i16 where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq i32 where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq i64 where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq f32 where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq f64 where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq Bool where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq Char where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq String where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq UUID where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq Hash where {
    == = prim_eq;
    != = prim_ne;
}

instance Eq DateTime where {
    == = prim_eq;
    != = prim_ne;
}

instance<a> Eq (List a) <= Eq a where {
    == = prim_list_eq;
    != = prim_list_ne;
}

instance<a> Eq (Option a) <= Eq a where {
    == = \x y ->
        match x with {
            case Some a0 ->
                (match y with {
                    case Some b0 -> a0 == b0;
                    case None -> false;
                });
            case None ->
                (match y with {
                    case None -> true;
                    case Some _ -> false;
                });
        };
    != = \x y -> if x == y then false else true;
}

instance<a,e> Eq (Result a e) <= Eq a, Eq e where {
    == = \x y ->
        match x with {
            case Ok a0 ->
                (match y with {
                    case Ok b0 -> a0 == b0;
                    case Err _ -> false;
                });
            case Err e0 ->
                (match y with {
                    case Err e1 -> e0 == e1;
                    case Ok _ -> false;
                });
        };
    != = \x y -> if x == y then false else true;
}

// Ord instances
instance Ord u8 <= Eq u8 where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

instance Ord u16 <= Eq u16 where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

instance Ord u32 <= Eq u32 where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

instance Ord u64 <= Eq u64 where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

instance Ord i8 <= Eq i8 where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

instance Ord i16 <= Eq i16 where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

instance Ord i32 <= Eq i32 where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

instance Ord i64 <= Eq i64 where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

instance Ord f32 <= Eq f32 where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

instance Ord f64 <= Eq f64 where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

instance Ord Char <= Eq Char where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

instance Ord String <= Eq String where {
    cmp = prim_cmp;
    < = prim_lt;
    <= = prim_le;
    > = prim_gt;
    >= = prim_ge;
}

// Show instances
instance Show Bool where {
    show = prim_show;
}

instance Show u8 where {
    show = prim_show;
}

instance Show u16 where {
    show = prim_show;
}

instance Show u32 where {
    show = prim_show;
}

instance Show u64 where {
    show = prim_show;
}

instance Show i8 where {
    show = prim_show;
}

instance Show i16 where {
    show = prim_show;
}

instance Show i32 where {
    show = prim_show;
}

instance Show i64 where {
    show = prim_show;
}

instance Show f32 where {
    show = prim_show;
}

instance Show f64 where {
    show = prim_show;
}

instance Show Char where {
    show = prim_show;
}

instance Show String where {
    show = prim_show;
}

instance Show UUID where {
    show = prim_show;
}

instance Show Hash where {
    show = prim_show;
}

instance Show DateTime where {
    show = prim_show;
}

// Default instances
instance Default Bool where {
    default = false;
}

instance Default u8 where {
    default = prim_zero;
}

instance Default u16 where {
    default = prim_zero;
}

instance Default u32 where {
    default = prim_zero;
}

instance Default u64 where {
    default = prim_zero;
}

instance Default i8 where {
    default = prim_zero;
}

instance Default i16 where {
    default = prim_zero;
}

instance Default i32 where {
    default = prim_zero;
}

instance Default i64 where {
    default = prim_zero;
}

instance Default f32 where {
    default = prim_zero;
}

instance Default f64 where {
    default = prim_zero;
}

instance Default Char where {
    default = '\0';
}

instance Default String where {
    default = prim_zero;
}

instance<a> Default (List a) where {
    default = [];
}

instance<a> Default (Option a) where {
    default = None;
}

instance<a,e> Default (Result a e) <= Default a where {
    default = Ok default;
}

instance<a> Show (List a) <= Show a where {
    show = \xs ->
        match xs with {
            case [] -> "[]";
            case x::xs1 ->
                let
                    step = \out y -> out + ", " + show y
                in
                    "[" + foldl step (show x) xs1 + "]";
        };
}

instance<a> Show (Option a) <= Show a where {
    show = \x ->
        match x with {
            case Some a0 -> "Some(" + show a0 + ")";
            case None -> "None";
        };
}

instance<a,e> Show (Result a e) <= Show a, Show e where {
    show = \x ->
        match x with {
            case Ok a0 -> "Ok(" + show a0 + ")";
            case Err e0 -> "Err(" + show e0 + ")";
        };
}

// Functor / Applicative / Monad / Foldable / Filterable / Sequence / Alternative instances
instance Functor List where {
    map = prim_map;
}

instance Functor Option where {
    map = prim_map;
}

instance<e> Functor (Result e) where {
    map = prim_map;
}

instance Functor Dict where {
    map = prim_dict_map;
}

instance Applicative List <= Functor List where {
    pure = \x -> [x];
    ap = \ff xx -> prim_flat_map (\f -> prim_map f xx) ff;
}

instance Applicative Option <= Functor Option where {
    pure = \x -> Some x;
    ap = \ff xx ->
        match ff with {
            case Some f -> map f xx;
            case None -> None;
        };
}

instance<e> Applicative (Result e) <= Functor (Result e) where {
    pure = \x -> Ok x;
    ap = \rf rx ->
        match rf with {
            case Ok f -> map f rx;
            case Err err -> Err err;
        };
}

instance Monad List <= Applicative List where {
    bind = prim_flat_map;
}

instance Monad Option <= Applicative Option where {
    bind = prim_flat_map;
}

instance<e> Monad (Result e) <= Applicative (Result e) where {
    bind = prim_flat_map;
}

instance Foldable List where {
    foldl = prim_foldl;
    foldr = prim_foldr;
    fold = prim_fold;
}

instance Foldable Option where {
    foldl = prim_foldl;
    foldr = prim_foldr;
    fold = prim_fold;
}

instance<a> Length (List a) where {
    length = prim_list_length;
}

instance<a> Length (Dict a) where {
    length = prim_dict_length;
}

instance Length String where {
    length = prim_string_length;
}

instance Filterable List <= Functor List where {
    filter = prim_filter;
    filter_map = prim_filter_map;
}

instance Filterable Option <= Functor Option where {
    filter = prim_filter;
    filter_map = prim_filter_map;
}

instance Filterable Dict <= Functor Dict where {
    filter = prim_dict_filter;
    filter_map = prim_dict_filter_map;
}

instance Sequence List <= Functor List, Foldable List where {
    take = prim_take;
    skip = prim_skip;
    zip = prim_zip;
    unzip = prim_unzip;
}

instance Alternative List <= Applicative List where {
    or_else = prim_or_else;
}

instance Alternative Option <= Applicative Option where {
    or_else = prim_or_else;
}

instance<e> Alternative (Result e) <= Applicative (Result e) where {
    or_else = prim_or_else;
}

// Indexable instances
instance<a> Indexable (List a, a) where {
    get = prim_get;
}


0
