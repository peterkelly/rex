{- prelude typeclasses and instances

   this file is parsed and injected by typesystem with_prelude

   design note:
   - class methods define the public surface (the names users call)
   - instance bodies in this file attach those names to rust-backed prim_ intrinsics
   - prim_ is the only magic: it is the equivalent of haskell / ghc primops
-}

{- numeric hierarchy -}
class AdditiveMonoid a where
    zero : a
    + : a -> a -> a

class MultiplicativeMonoid a where
    one : a
    * : a -> a -> a

class Semiring a <= AdditiveMonoid a, MultiplicativeMonoid a where

class AdditiveGroup a <= Semiring a where
    negate : a -> a
    - : a -> a -> a

class Ring a <= AdditiveGroup a, MultiplicativeMonoid a where

class Field a <= Ring a where
    / : a -> a -> a

class Integral a where
    % : a -> a -> a

{- equality and ordering -}
class Eq a where
    == : a -> a -> bool
    != : a -> a -> bool

class Ord a <= Eq a where
    cmp : a -> a -> i32
    < : a -> a -> bool
    <= : a -> a -> bool
    > : a -> a -> bool
    >= : a -> a -> bool

{- collection classes are still marker-only for now (their operations are native builtins) -}
class Functor f where
class Applicative f <= Functor f where
class Monad m <= Applicative m where
class Foldable t where
class Filterable f <= Functor f where
class Sequence f <= Functor f, Foldable f where
class Alternative f <= Applicative f where

class Indexable p where

{- AdditiveMonoid instances -}
instance AdditiveMonoid string where
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid u8 where
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid u16 where
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid u32 where
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid u64 where
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid i8 where
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid i16 where
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid i32 where
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid i64 where
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid f32 where
    zero = prim_zero
    + = prim_add
instance AdditiveMonoid f64 where
    zero = prim_zero
    + = prim_add

{- MultiplicativeMonoid instances -}
instance MultiplicativeMonoid u8 where
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid u16 where
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid u32 where
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid u64 where
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid i8 where
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid i16 where
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid i32 where
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid i64 where
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid f32 where
    one = prim_one
    * = prim_mul
instance MultiplicativeMonoid f64 where
    one = prim_one
    * = prim_mul

{- Semiring instances -}
instance Semiring u8 where
instance Semiring u16 where
instance Semiring u32 where
instance Semiring u64 where
instance Semiring i8 where
instance Semiring i16 where
instance Semiring i32 where
instance Semiring i64 where
instance Semiring f32 where
instance Semiring f64 where

{- AdditiveGroup and Ring instances -}
instance AdditiveGroup i8 <= Semiring i8 where
    negate = prim_negate
    - = prim_sub
instance AdditiveGroup i16 <= Semiring i16 where
    negate = prim_negate
    - = prim_sub
instance AdditiveGroup i32 <= Semiring i32 where
    negate = prim_negate
    - = prim_sub
instance AdditiveGroup i64 <= Semiring i64 where
    negate = prim_negate
    - = prim_sub
instance AdditiveGroup f32 <= Semiring f32 where
    negate = prim_negate
    - = prim_sub
instance AdditiveGroup f64 <= Semiring f64 where
    negate = prim_negate
    - = prim_sub

instance Ring i8 <= AdditiveGroup i8 where
instance Ring i16 <= AdditiveGroup i16 where
instance Ring i32 <= AdditiveGroup i32 where
instance Ring i64 <= AdditiveGroup i64 where
instance Ring f32 <= AdditiveGroup f32 where
instance Ring f64 <= AdditiveGroup f64 where

{- Field instances -}
instance Field f32 <= Ring f32 where
    / = prim_div
instance Field f64 <= Ring f64 where
    / = prim_div

{- Integral instances -}
instance Integral u8 where
    % = prim_mod
instance Integral u16 where
    % = prim_mod
instance Integral u32 where
    % = prim_mod
instance Integral u64 where
    % = prim_mod
instance Integral i8 where
    % = prim_mod
instance Integral i16 where
    % = prim_mod
instance Integral i32 where
    % = prim_mod
instance Integral i64 where
    % = prim_mod

{- Eq instances -}
instance Eq u8 where
    == = prim_eq
    != = prim_ne
instance Eq u16 where
    == = prim_eq
    != = prim_ne
instance Eq u32 where
    == = prim_eq
    != = prim_ne
instance Eq u64 where
    == = prim_eq
    != = prim_ne
instance Eq i8 where
    == = prim_eq
    != = prim_ne
instance Eq i16 where
    == = prim_eq
    != = prim_ne
instance Eq i32 where
    == = prim_eq
    != = prim_ne
instance Eq i64 where
    == = prim_eq
    != = prim_ne
instance Eq f32 where
    == = prim_eq
    != = prim_ne
instance Eq f64 where
    == = prim_eq
    != = prim_ne
instance Eq bool where
    == = prim_eq
    != = prim_ne
instance Eq string where
    == = prim_eq
    != = prim_ne
instance Eq uuid where
    == = prim_eq
    != = prim_ne
instance Eq datetime where
    == = prim_eq
    != = prim_ne

instance Eq (List a) <= Eq a where
    == = prim_eq
    != = prim_ne
instance Eq (Option a) <= Eq a where
    == = prim_eq
    != = prim_ne
instance Eq (Array a) <= Eq a where
    == = prim_eq
    != = prim_ne
instance Eq (Result e a) <= Eq e, Eq a where
    == = prim_eq
    != = prim_ne

{- Ord instances -}
instance Ord u8 <= Eq u8 where
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord u16 <= Eq u16 where
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord u32 <= Eq u32 where
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord u64 <= Eq u64 where
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord i8 <= Eq i8 where
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord i16 <= Eq i16 where
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord i32 <= Eq i32 where
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord i64 <= Eq i64 where
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord f32 <= Eq f32 where
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord f64 <= Eq f64 where
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge
instance Ord string <= Eq string where
    cmp = prim_cmp
    < = prim_lt
    <= = prim_le
    > = prim_gt
    >= = prim_ge

{- Functor / Applicative / Monad / Foldable / Filterable / Sequence / Alternative instances -}
instance Functor List where
instance Functor Option where
instance Functor Array where
instance Functor (Result e) where

instance Applicative List <= Functor List where
instance Applicative Option <= Functor Option where
instance Applicative Array <= Functor Array where
instance Applicative (Result e) <= Functor (Result e) where

instance Monad List <= Applicative List where
instance Monad Option <= Applicative Option where
instance Monad Array <= Applicative Array where
instance Monad (Result e) <= Applicative (Result e) where

instance Foldable List where
instance Foldable Option where
instance Foldable Array where

instance Filterable List <= Functor List where
instance Filterable Option <= Functor Option where
instance Filterable Array <= Functor Array where

instance Sequence List <= Functor List, Foldable List where
instance Sequence Array <= Functor Array, Foldable Array where

instance Alternative List <= Applicative List where
instance Alternative Option <= Applicative Option where
instance Alternative Array <= Applicative Array where
instance Alternative (Result e) <= Applicative (Result e) where

{- Indexable instances -}
instance Indexable (List a, a) where
instance Indexable (Array a, a) where

instance Indexable ((a, a), a) where
instance Indexable ((a, a, a), a) where
instance Indexable ((a, a, a, a), a) where
instance Indexable ((a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where
instance Indexable ((a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a, a), a) where

0
