{- prelude typeclasses and instances

   this file is parsed and injected by typesystem with_prelude

   design note:
   - class methods define the public surface (the names users call)
   - instance bodies in this file attach those names to rust-backed prim_ intrinsics
   - prim_ is the only magic: it is the equivalent of haskell / ghc primops
-}

{- numeric hierarchy -}
class AdditiveMonoid a
    zero : a
    + : a -> a -> a

class MultiplicativeMonoid a
    one : a
    * : a -> a -> a

class Semiring a <= AdditiveMonoid a, MultiplicativeMonoid a

class AdditiveGroup a <= Semiring a
    negate : a -> a
    - : a -> a -> a

class Ring a <= AdditiveGroup a, MultiplicativeMonoid a

class Field a <= Ring a
    / : a -> a -> a

class Integral a
    % : a -> a -> a

{- equality and ordering -}
class Eq a
    == : a -> a -> bool
    != : a -> a -> bool

class Ord a <= Eq a
    cmp : a -> a -> i32
    < : a -> a -> bool
    <= : a -> a -> bool
    > : a -> a -> bool
    >= : a -> a -> bool

{- show printing -}
class Show a
    show : a -> string

{- collection combinators -}
class Functor f
    map : (a -> b) -> f a -> f b

class Applicative f <= Functor f
    pure : a -> f a
    ap : f (a -> b) -> f a -> f b

class Monad m <= Applicative m
    {- Monad's core operation is "bind".

       We keep the argument order as (a -> m b) first, then (m a), to match
       the rest of Rex's collection API (map f xs, filter p xs, ...) and to
       map directly to the host intrinsic prim_flat_map without extra
       wrappers/allocations.
    -}
    bind : (a -> m b) -> m a -> m b

class Foldable t
    foldl : (b -> a -> b) -> b -> t a -> b
    foldr : (a -> b -> b) -> b -> t a -> b
    fold : (b -> a -> b) -> b -> t a -> b

class Filterable f <= Functor f
    filter : (a -> bool) -> f a -> f a
    filter_map : (a -> Option b) -> f a -> f b

class Sequence f <= Functor f, Foldable f
    take : i32 -> f a -> f a
    skip : i32 -> f a -> f a
    zip : f a -> f b -> f (a, b)
    unzip : f (a, b) -> (f a, f b)

class Alternative f <= Applicative f
    or_else : (f a -> f a) -> f a -> f a

{- Indexable needs two parameters: the container type and the element type. -}
class Indexable t a
    get : i32 -> t -> a

{- AdditiveMonoid instances -}
instance AdditiveMonoid string
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid u8
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid u16
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid u32
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid u64
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid i8
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid i16
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid i32
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid i64
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid f32
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid f64
    zero = prim_zero
    + = prim_add

{- MultiplicativeMonoid instances -}
instance MultiplicativeMonoid u8
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid u16
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid u32
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid u64
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid i8
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid i16
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid i32
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid i64
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid f32
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid f64
    one = prim_one
    * = prim_mul

{- Semiring instances -}
instance Semiring u8
instance Semiring u16
instance Semiring u32
instance Semiring u64
instance Semiring i8
instance Semiring i16
instance Semiring i32
instance Semiring i64
instance Semiring f32
instance Semiring f64

{- AdditiveGroup and Ring instances -}
instance AdditiveGroup i8 <= Semiring i8
    negate = prim_negate
    - = prim_sub
instance AdditiveGroup i16 <= Semiring i16
    negate = prim_negate
    - = prim_sub
instance AdditiveGroup i32 <= Semiring i32
    negate = prim_negate
    - = prim_sub
instance AdditiveGroup i64 <= Semiring i64
    negate = prim_negate
    - = prim_sub
instance AdditiveGroup f32 <= Semiring f32
    negate = prim_negate
    - = prim_sub
instance AdditiveGroup f64 <= Semiring f64
    negate = prim_negate
    - = prim_sub

instance Ring i8 <= AdditiveGroup i8
instance Ring i16 <= AdditiveGroup i16
instance Ring i32 <= AdditiveGroup i32
instance Ring i64 <= AdditiveGroup i64
instance Ring f32 <= AdditiveGroup f32
instance Ring f64 <= AdditiveGroup f64

{- Field instances -}
instance Field f32 <= Ring f32
    / = prim_div
instance Field f64 <= Ring f64
    / = prim_div

{- Integral instances -}
instance Integral u8
    % = prim_mod
instance Integral u16
    % = prim_mod
instance Integral u32
    % = prim_mod
instance Integral u64
    % = prim_mod
instance Integral i8
    % = prim_mod
instance Integral i16
    % = prim_mod
instance Integral i32
    % = prim_mod
instance Integral i64
    % = prim_mod

{- Eq instances -}
instance Eq u8
    == = prim_eq
    != = prim_ne
instance Eq u16
    == = prim_eq
    != = prim_ne
instance Eq u32
    == = prim_eq
    != = prim_ne
instance Eq u64
    == = prim_eq
    != = prim_ne
instance Eq i8
    == = prim_eq
    != = prim_ne
instance Eq i16
    == = prim_eq
    != = prim_ne
instance Eq i32
    == = prim_eq
    != = prim_ne
instance Eq i64
    == = prim_eq
    != = prim_ne
instance Eq f32
    == = prim_eq
    != = prim_ne
instance Eq f64
    == = prim_eq
    != = prim_ne
instance Eq bool
    == = prim_eq
    != = prim_ne
instance Eq string
    == = prim_eq
    != = prim_ne
instance Eq uuid
    == = prim_eq
    != = prim_ne
instance Eq datetime
    == = prim_eq
    != = prim_ne

instance Eq (List a) <= Eq a
    == = \xs ys ->
        match xs
            when [] ->
                (match ys
                    when [] -> true
                    when _ -> false)
            when x::xs1 ->
                (match ys
                    when y::ys1 -> if x == y then xs1 == ys1 else false
                    when [] -> false)
    != = \xs ys -> if xs == ys then false else true
instance Eq (Option a) <= Eq a
    == = \x y ->
        match x
            when Some a0 ->
                (match y
                    when Some b0 -> a0 == b0
                    when None -> false)
            when None ->
                (match y
                    when None -> true
                    when Some _ -> false)
    != = \x y -> if x == y then false else true
instance Eq (Array a) <= Eq a
    == = prim_array_eq
    != = prim_array_ne
instance Eq (Result a e) <= Eq a, Eq e
    == = \x y ->
        match x
            when Ok a0 ->
                (match y
                    when Ok b0 -> a0 == b0
                    when Err _ -> false)
            when Err e0 ->
                (match y
                    when Err e1 -> e0 == e1
                    when Ok _ -> false)
    != = \x y -> if x == y then false else true

{- Ord instances -}
instance Ord u8 <= Eq u8
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord u16 <= Eq u16
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord u32 <= Eq u32
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord u64 <= Eq u64
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord i8 <= Eq i8
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord i16 <= Eq i16
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord i32 <= Eq i32
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord i64 <= Eq i64
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord f32 <= Eq f32
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord f64 <= Eq f64
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord string <= Eq string
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge

{- Show instances -}
instance Show bool
    show = prim_show
instance Show u8
    show = prim_show
instance Show u16
    show = prim_show
instance Show u32
    show = prim_show
instance Show u64
    show = prim_show
instance Show i8
    show = prim_show
instance Show i16
    show = prim_show
instance Show i32
    show = prim_show
instance Show i64
    show = prim_show
instance Show f32
    show = prim_show
instance Show f64
    show = prim_show
instance Show string
    show = prim_show
instance Show uuid
    show = prim_show
instance Show datetime
    show = prim_show

instance Show (List a) <= Show a
    show = \xs ->
        match xs
            when [] -> "[]"
            when x::xs1 ->
                let
                    step = \out y -> out + ", " + show y
                in
                    "[" + foldl step (show x) xs1 + "]"

instance Show (Array a) <= Show a
    show = \xs ->
        let
            step = \out x ->
                if out == "<array "
                    then out + show x
                    else out + ", " + show x,
            out = foldl step "<array " xs
        in
            out + ">"

instance Show (Option a) <= Show a
    show = \x ->
        match x
            when Some a0 -> "Some(" + show a0 + ")"
            when None -> "None"

instance Show (Result a e) <= Show a, Show e
    show = \x ->
        match x
            when Ok a0 -> "Ok(" + show a0 + ")"
            when Err e0 -> "Err(" + show e0 + ")"

{- Functor / Applicative / Monad / Foldable / Filterable / Sequence / Alternative instances -}
instance Functor List
    map = prim_map
instance Functor Option
    map = prim_map
instance Functor Array
    map = prim_map
instance Functor (Result e)
    map = prim_map

instance Applicative List <= Functor List
    pure = \x -> [x]
    ap = \ff xx -> prim_flat_map (\f -> prim_map f xx) ff
instance Applicative Option <= Functor Option
    pure = \x -> Some x
    ap = \ff xx ->
        match ff
            when Some f -> map f xx
            when None -> None
instance Applicative Array <= Functor Array
    pure = prim_array_singleton
    ap = \ff xx -> prim_flat_map (\f -> prim_map f xx) ff
instance Applicative (Result e) <= Functor (Result e)
    pure = \x -> Ok x
    ap = \rf rx ->
        match rf
            when Ok f -> map f rx
            when Err err -> Err err

instance Monad List <= Applicative List
    bind = prim_flat_map
instance Monad Option <= Applicative Option
    bind = prim_flat_map
instance Monad Array <= Applicative Array
    bind = prim_flat_map
instance Monad (Result e) <= Applicative (Result e)
    bind = prim_flat_map

instance Foldable List
    foldl = prim_foldl
    foldr = prim_foldr
    fold = prim_fold
instance Foldable Option
    foldl = prim_foldl
    foldr = prim_foldr
    fold = prim_fold
instance Foldable Array
    foldl = prim_foldl
    foldr = prim_foldr
    fold = prim_fold

instance Filterable List <= Functor List
    filter = prim_filter
    filter_map = prim_filter_map
instance Filterable Option <= Functor Option
    filter = prim_filter
    filter_map = prim_filter_map
instance Filterable Array <= Functor Array
    filter = prim_filter
    filter_map = prim_filter_map

instance Sequence List <= Functor List, Foldable List
    take = prim_take
    skip = prim_skip
    zip = prim_zip
    unzip = prim_unzip
instance Sequence Array <= Functor Array, Foldable Array
    take = prim_take
    skip = prim_skip
    zip = prim_zip
    unzip = prim_unzip

instance Alternative List <= Applicative List
    or_else = prim_or_else
instance Alternative Option <= Applicative Option
    or_else = prim_or_else
instance Alternative Array <= Applicative Array
    or_else = prim_or_else
instance Alternative (Result e) <= Applicative (Result e)
    or_else = prim_or_else

{- Indexable instances -}
instance Indexable (List a, a)
    get = prim_get
instance Indexable (Array a, a)
    get = prim_get

0
