# Built-in types & functions

> This page is auto-generated from the prelude source. Run `cargo run -p rex-cli --bin gen_prelude_docs` to refresh it.

## Built-in Types

| Type | Description |
|---|---|
| `Bool` | Boolean truth value. |
| `Char` | One Unicode scalar value, matching Rust's `char`. |
| `DateTime` | UTC timestamp value. |
| `Dict a` | Immutable mapping from string keys to values of one type. |
| `Hash` | BLAKE3 hash value. |
| `List a` | Immutable ordered sequence. Constructors: `Empty`, `Cons`. |
| `Option a` | Optional value (`Some` or `None`). Constructors: `Some`, `None`. |
| `Ordering` | Three-way comparison result (`Less`, `Equal`, or `Greater`). Constructors: `Less`, `Equal`, `Greater`. |
| `Result a b` | Result value (`Ok` or `Err`) for success/failure flows. Constructors: `Err`, `Ok`. |
| `String` | UTF-8 string value. |
| `UUID` | UUID value. |
| `f32` | 32-bit floating-point number. |
| `f64` | 64-bit floating-point number. |
| `i16` | 16-bit signed integer. |
| `i32` | 32-bit signed integer. |
| `i64` | 64-bit signed integer. |
| `i8` | 8-bit signed integer. |
| `u16` | 16-bit unsigned integer. |
| `u32` | 32-bit unsigned integer. |
| `u64` | 64-bit unsigned integer. |
| `u8` | 8-bit unsigned integer. |

## Built-in Type Classes

### `AdditiveGroup`
Types supporting additive inverse.

Superclasses: `Subtractive`

Methods:
- `negate`: `AdditiveGroup 'a => ('a -> 'a)`. Additive inverse.

### `AdditiveMonoid`
Types with additive identity and associative addition.

Superclasses: _none_

Methods:
- `zero`: `AdditiveMonoid 'a => 'a`. Additive identity.
- `+`: `AdditiveMonoid 'a => ('a -> ('a -> 'a))`. Addition (or concatenation for strings).

### `Alternative`
Applicative types with a fallback choice operation.

Superclasses: `Applicative`

Methods:
- `or_else`: `Alternative 'f => ((('f 'a) -> ('f 'a)) -> (('f 'a) -> ('f 'a)))`. Provide an alternative container value.

### `Applicative`
Functors that can lift values and apply wrapped functions.

Superclasses: `Functor`

Methods:
- `pure`: `Applicative 'f => ('a -> ('f 'a))`. Lift a plain value into an applicative context.
- `ap`: `Applicative 'f => (('f ('a -> 'b)) -> (('f 'a) -> ('f 'b)))`. Apply wrapped functions to wrapped values.

### `Default`
Types with a canonical default value.

Superclasses: _none_

Methods:
- `default`: `Default 'a => 'a`. Canonical default value for a type. For `Result a e`, this requires `Default a`.

### `Divisive`
Types supporting division.

Superclasses: `MultiplicativeMonoid`

Methods:
- `/`: `Divisive 'a => ('a -> ('a -> 'a))`. Division.

### `Eq`
Types supporting equality/inequality comparison.

Superclasses: _none_

Methods:
- `==`: `Eq 'a => ('a -> ('a -> Bool))`. Equality comparison.
- `!=`: `Eq 'a => ('a -> ('a -> Bool))`. Inequality comparison.

### `Field`
Types supporting division in addition to ring operations.

Superclasses: `Ring`, `Divisive`

Methods:

### `Filterable`
Functors supporting filtering and partial mapping.

Superclasses: `Functor`

Methods:
- `filter`: `Filterable 'f => (('a -> Bool) -> (('f 'a) -> ('f 'a)))`. Keep elements that satisfy a predicate.
- `filter_map`: `Filterable 'f => (('a -> (Option 'b)) -> (('f 'a) -> ('f 'b)))`. Map and drop missing results in one pass.

### `Foldable`
Containers that can be reduced with folds.

Superclasses: _none_

Methods:
- `foldl`: `Foldable 't => (('b -> ('a -> 'b)) -> ('b -> (('t 'a) -> 'b)))`. Strict left fold.
- `foldr`: `Foldable 't => (('a -> ('b -> 'b)) -> ('b -> (('t 'a) -> 'b)))`. Right fold.
- `fold`: `Foldable 't => (('b -> ('a -> 'b)) -> ('b -> (('t 'a) -> 'b)))`. Left-style fold over a container.

### `Functor`
Type constructors that support structure-preserving mapping.

Superclasses: _none_

Methods:
- `map`: `Functor 'f => (('a -> 'b) -> (('f 'a) -> ('f 'b)))`. Apply a function to each value inside a functor.

### `Indexable`
Containers that support indexed element access.

Superclasses: _none_

Methods:
- `get`: `Indexable ('t, 'a) => (u64 -> ('t -> 'a))`. Get an element by index.

### `Integral`
Integral numeric types supporting modulo.

Superclasses: _none_

Methods:
- `%`: `Integral 'a => ('a -> ('a -> 'a))`. Remainder/modulo operation.

### `Length`
Collections and strings whose length can be measured.

Superclasses: _none_

Methods:
- `length`: `Length 'a => ('a -> u64)`. Return the number of list elements, dictionary entries, or string Unicode scalar values.

### `Monad`
Applicatives supporting dependent sequencing (`bind`).

Superclasses: `Applicative`

Methods:
- `bind`: `Monad 'm => (('a -> ('m 'b)) -> (('m 'a) -> ('m 'b)))`. Monadic flat-map/sequencing operation.

### `MultiplicativeMonoid`
Types with multiplicative identity and associative multiplication.

Superclasses: _none_

Methods:
- `one`: `MultiplicativeMonoid 'a => 'a`. Multiplicative identity.
- `*`: `MultiplicativeMonoid 'a => ('a -> ('a -> 'a))`. Multiplication.

### `Ord`
Types with total ordering comparisons.

Superclasses: `Eq`

Methods:
- `cmp`: `Ord 'a => ('a -> ('a -> Ordering))`. Three-way comparison returning `Less`, `Equal`, or `Greater`.
- `<`: `Ord 'a => ('a -> ('a -> Bool))`. Less-than comparison.
- `<=`: `Ord 'a => ('a -> ('a -> Bool))`. Less-than-or-equal comparison.
- `>`: `Ord 'a => ('a -> ('a -> Bool))`. Greater-than comparison.
- `>=`: `Ord 'a => ('a -> ('a -> Bool))`. Greater-than-or-equal comparison.

### `Ring`
Types supporting additive group plus multiplication.

Superclasses: `AdditiveGroup`, `MultiplicativeMonoid`

Methods:

### `Semiring`
Types supporting additive and multiplicative monoid operations.

Superclasses: `AdditiveMonoid`, `MultiplicativeMonoid`

Methods:

### `Sequence`
Ordered containers with slicing/zipping operations.

Superclasses: `Functor`, `Foldable`

Methods:
- `take`: `Sequence 'f => (u64 -> (('f 'a) -> ('f 'a)))`. Keep only the first `n` elements.
- `skip`: `Sequence 'f => (u64 -> (('f 'a) -> ('f 'a)))`. Drop the first `n` elements.
- `zip`: `Sequence 'f => (('f 'a) -> (('f 'b) -> ('f ('a, 'b))))`. Pair elements from two containers by position.
- `unzip`: `Sequence 'f => (('f ('a, 'b)) -> (('f 'a), ('f 'b)))`. Split a container of pairs into a pair of containers.

### `Show`
Types that can be converted to user-facing strings (Haskell-style naming).

Superclasses: _none_

Methods:
- `show`: `Show 'a => ('a -> String)`. Render a value as a human-readable string.

### `Subtractive`
Types supporting binary subtraction.

Superclasses: `Semiring`

Methods:
- `-`: `Subtractive 'a => ('a -> ('a -> 'a))`. Subtraction.

## Built-in Functions

### Overloaded (Type Class Methods)

| Function | Signature | Implemented On | Description |
|---|---|---|---|
| `negate` | `('a -> 'a)` | `i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Additive inverse. |
| `zero` | `'a` | `(List 'a)`<br>`String`<br>`u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Additive identity. |
| `+` | `('a -> ('a -> 'a))` | `(List 'a)`<br>`String`<br>`u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Addition (or concatenation for strings). |
| `or_else` | `((('f 'a) -> ('f 'a)) -> (('f`<br>`'a) -> ('f 'a)))` | `List`<br>`Option`<br>`(Result 'e)` | Provide an alternative container value. |
| `pure` | `('a -> ('f 'a))` | `List`<br>`Option`<br>`(Result 'e)` | Lift a plain value into an applicative context. |
| `ap` | `(('f ('a -> 'b)) -> (('f 'a)`<br>`-> ('f 'b)))` | `List`<br>`Option`<br>`(Result 'e)` | Apply wrapped functions to wrapped values. |
| `default` | `'a` | `Bool`<br>`u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`Char`<br>`String`<br>`(List 'a)`<br>`(Option 'a)`<br>`(Result 'a 'e)` | Canonical default value for a type. For `Result a e`, this requires `Default a`. |
| `/` | `('a -> ('a -> 'a))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Division. |
| `==` | `('a -> ('a -> Bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`Bool`<br>`Char`<br>`String`<br>`UUID`<br>`Hash`<br>`DateTime`<br>`(List 'a)`<br>`(Option 'a)`<br>`(Result 'a 'e)` | Equality comparison. |
| `!=` | `('a -> ('a -> Bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`Bool`<br>`Char`<br>`String`<br>`UUID`<br>`Hash`<br>`DateTime`<br>`(List 'a)`<br>`(Option 'a)`<br>`(Result 'a 'e)` | Inequality comparison. |
| `filter` | `(('a -> Bool) -> (('f 'a) ->`<br>`('f 'a)))` | `List`<br>`Option`<br>`Dict` | Keep elements that satisfy a predicate. |
| `filter_map` | `(('a -> (Option 'b)) -> (('f`<br>`'a) -> ('f 'b)))` | `List`<br>`Option`<br>`Dict` | Map and drop missing results in one pass. |
| `foldl` | `(('b -> ('a -> 'b)) -> ('b ->`<br>`(('t 'a) -> 'b)))` | `List`<br>`Option` | Strict left fold. |
| `foldr` | `(('a -> ('b -> 'b)) -> ('b ->`<br>`(('t 'a) -> 'b)))` | `List`<br>`Option` | Right fold. |
| `fold` | `(('b -> ('a -> 'b)) -> ('b ->`<br>`(('t 'a) -> 'b)))` | `List`<br>`Option` | Left-style fold over a container. |
| `map` | `(('a -> 'b) -> (('f 'a) -> ('f`<br>`'b)))` | `List`<br>`Option`<br>`(Result 'e)`<br>`Dict` | Apply a function to each value inside a functor. |
| `get` | `(u64 -> ('t -> 'a))` | `((List 'a), 'a)` | Get an element by index. |
| `%` | `('a -> ('a -> 'a))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64` | Remainder/modulo operation. |
| `length` | `('a -> u64)` | `(List 'a)`<br>`(Dict 'a)`<br>`String` | Return the number of list elements, dictionary entries, or string Unicode scalar values. |
| `bind` | `(('a -> ('m 'b)) -> (('m 'a)`<br>`-> ('m 'b)))` | `List`<br>`Option`<br>`(Result 'e)` | Monadic flat-map/sequencing operation. |
| `one` | `'a` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Multiplicative identity. |
| `*` | `('a -> ('a -> 'a))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Multiplication. |
| `cmp` | `('a -> ('a -> Ordering))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`Char`<br>`String` | Three-way comparison returning `Less`, `Equal`, or `Greater`. |
| `<` | `('a -> ('a -> Bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`Char`<br>`String` | Less-than comparison. |
| `<=` | `('a -> ('a -> Bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`Char`<br>`String` | Less-than-or-equal comparison. |
| `>` | `('a -> ('a -> Bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`Char`<br>`String` | Greater-than comparison. |
| `>=` | `('a -> ('a -> Bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`Char`<br>`String` | Greater-than-or-equal comparison. |
| `take` | `(u64 -> (('f 'a) -> ('f 'a)))` | `List` | Keep only the first `n` elements. |
| `skip` | `(u64 -> (('f 'a) -> ('f 'a)))` | `List` | Drop the first `n` elements. |
| `zip` | `(('f 'a) -> (('f 'b) -> ('f`<br>`('a, 'b))))` | `List` | Pair elements from two containers by position. |
| `unzip` | `(('f ('a, 'b)) -> (('f 'a),`<br>`('f 'b)))` | `List` | Split a container of pairs into a pair of containers. |
| `show` | `('a -> String)` | `Bool`<br>`u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`Char`<br>`String`<br>`UUID`<br>`Hash`<br>`DateTime`<br>`(List 'a)`<br>`(Option 'a)`<br>`(Result 'a 'e)` | Render a value as a human-readable string. |
| `-` | `('a -> ('a -> 'a))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Subtraction. |

### Other Built-ins

| Function | Signature | Description |
|---|---|---|
| `&&` | `(Bool -> (Bool -> Bool))` | Boolean conjunction. |
| `Cons` | `('a -> ((List 'a) -> (List`<br>`'a)))` | Construct a non-empty list from head and tail. |
| `Empty` | `(List 'a)` | The empty list constructor. |
| `Equal` | `Ordering` | Construct an `Ordering` result for equal values. |
| `Err` | `('e -> (Result 't 'e))` | Construct a failed `Result`. |
| `Greater` | `Ordering` | Construct an `Ordering` result when the left value is greater. |
| `Less` | `Ordering` | Construct an `Ordering` result when the left value is less. |
| `None` | `(Option 't)` | The empty `Option` constructor. |
| `Ok` | `('t -> (Result 't 'e))` | Construct a successful `Result`. |
| `Some` | `('t -> (Option 't))` | Construct a present `Option` value. |
| `dict_empty` | `(Dict 'a)` | Construct an empty dictionary. |
| `dict_entries` | `((Dict 'a) -> (List (String,`<br>`'a)))` | Return key/value tuples in lexicographic key order. |
| `dict_filter` | `(((String, 'a) -> Bool) ->`<br>`((Dict 'a) -> (Dict 'a)))` | Keep dictionary entries whose key/value tuple satisfies a predicate. |
| `dict_from_entries` | `((List (String, 'a)) -> (Dict`<br>`'a))` | Construct a dictionary from key/value tuples; later duplicate keys win. |
| `dict_get` | `(String -> ((Dict 'a) ->`<br>`(Option 'a)))` | Look up a string key, returning `Some` for a present value or `None`. |
| `dict_has` | `(String -> ((Dict 'a) ->`<br>`Bool))` | Test whether a string key is present. |
| `dict_insert` | `(String -> ('a -> ((Dict 'a)`<br>`-> (Dict 'a))))` | Return a dictionary with a string key inserted or replaced. |
| `dict_is_empty` | `((Dict 'a) -> Bool)` | Test whether a dictionary has no entries. |
| `dict_keys` | `((Dict 'a) -> (List String))` | Return keys in lexicographic order. |
| `dict_map` | `(((String, 'a) -> (String,`<br>`'b)) -> ((Dict 'a) -> (Dict`<br>`'b)))` | Transform key/value tuples into a dictionary; later collisions in input-key order win. |
| `dict_remove` | `(String -> ((Dict 'a) -> (Dict`<br>`'a)))` | Return a dictionary without a string key. |
| `dict_singleton` | `(String -> ('a -> (Dict 'a)))` | Construct a dictionary containing one key/value entry. |
| `dict_update` | `(String -> (((Option 'a) ->`<br>`(Option 'a)) -> ((Dict 'a) ->`<br>`(Dict 'a))))` | Insert, replace, or remove a key by transforming its optional value. |
| `dict_values` | `((Dict 'a) -> (List 'a))` | Return values in lexicographic key order. |
| `first` | `(i32 -> ((List 'a) -> (List`<br>`'a)))` | Return the first `n` list elements; errors if `n` is out of range. |
| `is_err` | `((Result 't 'e) -> Bool)` | Check whether a `Result` is `Err`. |
| `is_none` | `((Option 'a) -> Bool)` | Check whether an `Option` is `None`. |
| `is_ok` | `((Result 't 'e) -> Bool)` | Check whether a `Result` is `Ok`. |
| `is_some` | `((Option 'a) -> Bool)` | Check whether an `Option` is `Some`. |
| `last` | `(i32 -> ((List 'a) -> (List`<br>`'a)))` | Return the last `n` list elements; errors if `n` is out of range. |
| `max` | `Foldable 'f, Ord 'a => (('f`<br>`'a) -> 'a)` | Maximum element by ordering. |
| `mean` | `Foldable 'f, Field 'a => (('f`<br>`'a) -> 'a)` | Arithmetic mean over numeric foldables. |
| `min` | `Foldable 'f, Ord 'a => (('f`<br>`'a) -> 'a)` | Minimum element by ordering. |
| `slice` | `(i32 -> (i32 -> ((List 'a) ->`<br>`(List 'a))))` | Return elements in the half-open range `n..m`; errors if either bound is out of range or `m < n`. |
| `string_to_hash` | `(String -> Hash)` | Parse a hexadecimal BLAKE3 hash string; raises an error if the string is invalid. |
| `sum` | `Foldable 'f, AdditiveMonoid 'a`<br>`=> (('f 'a) -> 'a)` | Sum all elements in a foldable container. |
| `unwrap` | `((Option 'a) -> 'a)`<br><br>`((Result 't 'e) -> 't)` | Extract the inner value from `Some`/`Ok`, or raise an error for `None`/`Err`. |
| `||` | `(Bool -> (Bool -> Bool))` | Boolean disjunction. |
