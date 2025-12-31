{- prelude typeclasses and instances

   this file is parsed and injected by typesystem with_prelude

   note these are marker classes with no methods
-}

class AdditiveMonoid a where
class MultiplicativeMonoid a where
class Semiring a <= AdditiveMonoid a, MultiplicativeMonoid a where
class AdditiveGroup a <= Semiring a where
class Ring a <= AdditiveGroup a, MultiplicativeMonoid a where
class Field a <= Ring a where
class Integral a where

class Eq a where
class Ord a <= Eq a where

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
instance AdditiveMonoid u8 where
instance AdditiveMonoid u16 where
instance AdditiveMonoid u32 where
instance AdditiveMonoid u64 where
instance AdditiveMonoid i8 where
instance AdditiveMonoid i16 where
instance AdditiveMonoid i32 where
instance AdditiveMonoid i64 where
instance AdditiveMonoid f32 where
instance AdditiveMonoid f64 where

{- MultiplicativeMonoid instances -}
instance MultiplicativeMonoid u8 where
instance MultiplicativeMonoid u16 where
instance MultiplicativeMonoid u32 where
instance MultiplicativeMonoid u64 where
instance MultiplicativeMonoid i8 where
instance MultiplicativeMonoid i16 where
instance MultiplicativeMonoid i32 where
instance MultiplicativeMonoid i64 where
instance MultiplicativeMonoid f32 where
instance MultiplicativeMonoid f64 where

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
instance AdditiveGroup i16 <= Semiring i16 where
instance AdditiveGroup i32 <= Semiring i32 where
instance AdditiveGroup i64 <= Semiring i64 where
instance AdditiveGroup f32 <= Semiring f32 where
instance AdditiveGroup f64 <= Semiring f64 where

instance Ring i8 <= AdditiveGroup i8 where
instance Ring i16 <= AdditiveGroup i16 where
instance Ring i32 <= AdditiveGroup i32 where
instance Ring i64 <= AdditiveGroup i64 where
instance Ring f32 <= AdditiveGroup f32 where
instance Ring f64 <= AdditiveGroup f64 where

{- Field instances -}
instance Field f32 <= Ring f32 where
instance Field f64 <= Ring f64 where

{- Integral instances -}
instance Integral u8 where
instance Integral u16 where
instance Integral u32 where
instance Integral u64 where
instance Integral i8 where
instance Integral i16 where
instance Integral i32 where
instance Integral i64 where

{- Eq instances -}
instance Eq u8 where
instance Eq u16 where
instance Eq u32 where
instance Eq u64 where
instance Eq i8 where
instance Eq i16 where
instance Eq i32 where
instance Eq i64 where
instance Eq f32 where
instance Eq f64 where
instance Eq bool where
instance Eq string where
instance Eq uuid where
instance Eq datetime where

instance Eq (List a) <= Eq a where
instance Eq (Option a) <= Eq a where
instance Eq (Array a) <= Eq a where
instance Eq (Result e a) <= Eq e, Eq a where

{- Ord instances -}
instance Ord u8 <= Eq u8 where
instance Ord u16 <= Eq u16 where
instance Ord u32 <= Eq u32 where
instance Ord u64 <= Eq u64 where
instance Ord i8 <= Eq i8 where
instance Ord i16 <= Eq i16 where
instance Ord i32 <= Eq i32 where
instance Ord i64 <= Eq i64 where
instance Ord f32 <= Eq f32 where
instance Ord f64 <= Eq f64 where
instance Ord string <= Eq string where

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
