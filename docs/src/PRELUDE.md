# Built-in types & functions

> This page is auto-generated from the prelude source. Run `cargo run -p rexlang-core --bin gen_prelude_docs` to refresh it.

## Built-in Types

| Type | Description |
|---|---|
| `Array a` | Fixed-size indexed sequence. |
| `Dict a` | Dictionary/record-like mapping from field labels to values. |
| `List a` | Immutable singly linked list. Constructors: `Empty`, `Cons`. |
| `Option a` | Optional value (`Some` or `None`). Constructors: `Some`, `None`. |
| `Result a b` | Result value (`Ok` or `Err`) for success/failure flows. Constructors: `Err`, `Ok`. |
| `bool` | Boolean truth value. |
| `datetime` | UTC timestamp value. |
| `f32` | 32-bit floating-point number. |
| `f64` | 64-bit floating-point number. |
| `i16` | 16-bit signed integer. |
| `i32` | 32-bit signed integer. |
| `i64` | 64-bit signed integer. |
| `i8` | 8-bit signed integer. |
| `string` | UTF-8 string value. |
| `u16` | 16-bit unsigned integer. |
| `u32` | 32-bit unsigned integer. |
| `u64` | 64-bit unsigned integer. |
| `u8` | 8-bit unsigned integer. |
| `uuid` | UUID value. |

## Built-in Type Classes

### `AdditiveGroup`
Types supporting additive inverse and subtraction.

Superclasses: `Semiring`

Methods:
- `negate`: `AdditiveGroup 'a => ('a -> 'a)`. Additive inverse.
- `-`: `AdditiveGroup 'a => ('a -> ('a -> 'a))`. Subtraction.

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

### `Eq`
Types supporting equality/inequality comparison.

Superclasses: _none_

Methods:
- `==`: `Eq 'a => ('a -> ('a -> bool))`. Equality comparison.
- `!=`: `Eq 'a => ('a -> ('a -> bool))`. Inequality comparison.

### `Field`
Types supporting division in addition to ring operations.

Superclasses: `Ring`

Methods:
- `/`: `Field 'a => ('a -> ('a -> 'a))`. Division.

### `Filterable`
Functors supporting filtering and partial mapping.

Superclasses: `Functor`

Methods:
- `filter`: `Filterable 'f => (('a -> bool) -> (('f 'a) -> ('f 'a)))`. Keep elements that satisfy a predicate.
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
- `get`: `Indexable ('t, 'a) => (i32 -> ('t -> 'a))`. Get an element by index.

### `Integral`
Integral numeric types supporting modulo.

Superclasses: _none_

Methods:
- `%`: `Integral 'a => ('a -> ('a -> 'a))`. Remainder/modulo operation.

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
- `cmp`: `Ord 'a => ('a -> ('a -> i32))`. Three-way comparison returning negative/zero/positive `i32`.
- `<`: `Ord 'a => ('a -> ('a -> bool))`. Less-than comparison.
- `<=`: `Ord 'a => ('a -> ('a -> bool))`. Less-than-or-equal comparison.
- `>`: `Ord 'a => ('a -> ('a -> bool))`. Greater-than comparison.
- `>=`: `Ord 'a => ('a -> ('a -> bool))`. Greater-than-or-equal comparison.

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
- `take`: `Sequence 'f => (i32 -> (('f 'a) -> ('f 'a)))`. Keep only the first `n` elements.
- `skip`: `Sequence 'f => (i32 -> (('f 'a) -> ('f 'a)))`. Drop the first `n` elements.
- `zip`: `Sequence 'f => (('f 'a) -> (('f 'b) -> ('f ('a, 'b))))`. Pair elements from two containers by position.
- `unzip`: `Sequence 'f => (('f ('a, 'b)) -> (('f 'a), ('f 'b)))`. Split a container of pairs into a pair of containers.

### `Show`
Types that can be converted to user-facing strings (Haskell-style naming).

Superclasses: _none_

Methods:
- `show`: `Show 'a => ('a -> string)`. Render a value as a human-readable string.

## Built-in Functions

### Overloaded (Type Class Methods)

| Function | Signature | Implemented On | Description |
|---|---|---|---|
| `negate` | `('a -> 'a)` | `i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Additive inverse. |
| `-` | `('a -> ('a -> 'a))` | `i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Subtraction. |
| `zero` | `'a` | `string`<br>`u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Additive identity. |
| `+` | `('a -> ('a -> 'a))` | `string`<br>`u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Addition (or concatenation for strings). |
| `or_else` | `((('f 'a) -> ('f 'a)) -> (('f`<br>`'a) -> ('f 'a)))` | `List`<br>`Option`<br>`Array`<br>`(Result 'e)` | Provide an alternative container value. |
| `pure` | `('a -> ('f 'a))` | `List`<br>`Option`<br>`Array`<br>`(Result 'e)` | Lift a plain value into an applicative context. |
| `ap` | `(('f ('a -> 'b)) -> (('f 'a)`<br>`-> ('f 'b)))` | `List`<br>`Option`<br>`Array`<br>`(Result 'e)` | Apply wrapped functions to wrapped values. |
| `default` | `'a` | `bool`<br>`u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`string`<br>`(List 'a)`<br>`(Array 'a)`<br>`(Option 'a)`<br>`(Result 'a 'e)` | Canonical default value for a type. For `Result a e`, this requires `Default a`. |
| `==` | `('a -> ('a -> bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`bool`<br>`string`<br>`uuid`<br>`datetime`<br>`(List 'a)`<br>`(Option 'a)`<br>`(Array 'a)`<br>`(Result 'a 'e)` | Equality comparison. |
| `!=` | `('a -> ('a -> bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`bool`<br>`string`<br>`uuid`<br>`datetime`<br>`(List 'a)`<br>`(Option 'a)`<br>`(Array 'a)`<br>`(Result 'a 'e)` | Inequality comparison. |
| `/` | `('a -> ('a -> 'a))` | `f32`<br>`f64` | Division. |
| `filter` | `(('a -> bool) -> (('f 'a) ->`<br>`('f 'a)))` | `List`<br>`Option`<br>`Array` | Keep elements that satisfy a predicate. |
| `filter_map` | `(('a -> (Option 'b)) -> (('f`<br>`'a) -> ('f 'b)))` | `List`<br>`Option`<br>`Array` | Map and drop missing results in one pass. |
| `foldl` | `(('b -> ('a -> 'b)) -> ('b ->`<br>`(('t 'a) -> 'b)))` | `List`<br>`Option`<br>`Array` | Strict left fold. |
| `foldr` | `(('a -> ('b -> 'b)) -> ('b ->`<br>`(('t 'a) -> 'b)))` | `List`<br>`Option`<br>`Array` | Right fold. |
| `fold` | `(('b -> ('a -> 'b)) -> ('b ->`<br>`(('t 'a) -> 'b)))` | `List`<br>`Option`<br>`Array` | Left-style fold over a container. |
| `map` | `(('a -> 'b) -> (('f 'a) -> ('f`<br>`'b)))` | `List`<br>`Option`<br>`Array`<br>`(Result 'e)` | Apply a function to each value inside a functor. |
| `get` | `(i32 -> ('t -> 'a))` | `((List 'a), 'a)`<br>`((Array 'a), 'a)` | Get an element by index. |
| `%` | `('a -> ('a -> 'a))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64` | Remainder/modulo operation. |
| `bind` | `(('a -> ('m 'b)) -> (('m 'a)`<br>`-> ('m 'b)))` | `List`<br>`Option`<br>`Array`<br>`(Result 'e)` | Monadic flat-map/sequencing operation. |
| `one` | `'a` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Multiplicative identity. |
| `*` | `('a -> ('a -> 'a))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64` | Multiplication. |
| `cmp` | `('a -> ('a -> i32))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`string` | Three-way comparison returning negative/zero/positive `i32`. |
| `<` | `('a -> ('a -> bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`string` | Less-than comparison. |
| `<=` | `('a -> ('a -> bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`string` | Less-than-or-equal comparison. |
| `>` | `('a -> ('a -> bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`string` | Greater-than comparison. |
| `>=` | `('a -> ('a -> bool))` | `u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`string` | Greater-than-or-equal comparison. |
| `take` | `(i32 -> (('f 'a) -> ('f 'a)))` | `List`<br>`Array` | Keep only the first `n` elements. |
| `skip` | `(i32 -> (('f 'a) -> ('f 'a)))` | `List`<br>`Array` | Drop the first `n` elements. |
| `zip` | `(('f 'a) -> (('f 'b) -> ('f`<br>`('a, 'b))))` | `List`<br>`Array` | Pair elements from two containers by position. |
| `unzip` | `(('f ('a, 'b)) -> (('f 'a),`<br>`('f 'b)))` | `List`<br>`Array` | Split a container of pairs into a pair of containers. |
| `show` | `('a -> string)` | `bool`<br>`u8`<br>`u16`<br>`u32`<br>`u64`<br>`i8`<br>`i16`<br>`i32`<br>`i64`<br>`f32`<br>`f64`<br>`string`<br>`uuid`<br>`datetime`<br>`(List 'a)`<br>`(Array 'a)`<br>`(Option 'a)`<br>`(Result 'a 'e)` | Render a value as a human-readable string. |

### Other Built-ins

| Function | Signature | Description |
|---|---|---|
| `&&` | `(bool -> (bool -> bool))` | Boolean conjunction. |
| `Cons` | `('a -> ((List 'a) -> (List`<br>`'a)))` | Construct a non-empty list from head and tail. |
| `Empty` | `(List 'a)` | The empty list constructor. |
| `Err` | `('e -> (Result 't 'e))` | Construct a failed `Result`. |
| `None` | `(Option 't)` | The empty `Option` constructor. |
| `Ok` | `('t -> (Result 't 'e))` | Construct a successful `Result`. |
| `Some` | `('t -> (Option 't))` | Construct a present `Option` value. |
| `count` | `Foldable 'f => (('f 'a) ->`<br>`i32)` | Count elements in a foldable container. |
| `is_err` | `((Result 't 'e) -> bool)` | Check whether a `Result` is `Err`. |
| `is_none` | `((Option 'a) -> bool)` | Check whether an `Option` is `None`. |
| `is_ok` | `((Result 't 'e) -> bool)` | Check whether a `Result` is `Ok`. |
| `is_some` | `((Option 'a) -> bool)` | Check whether an `Option` is `Some`. |
| `max` | `Foldable 'f, Ord 'a => (('f`<br>`'a) -> 'a)` | Maximum element by ordering. |
| `mean` | `Foldable 'f, Field 'a => (('f`<br>`'a) -> 'a)` | Arithmetic mean over numeric foldables. |
| `min` | `Foldable 'f, Ord 'a => (('f`<br>`'a) -> 'a)` | Minimum element by ordering. |
| `sum` | `Foldable 'f, AdditiveMonoid 'a`<br>`=> (('f 'a) -> 'a)` | Sum all elements in a foldable container. |
| `to_array` | `((List 'a) -> (Array 'a))` | Convert a list to an array. |
| `to_list` | `((Array 'a) -> (List 'a))` | Convert an array to a list. |
| `||` | `(bool -> (bool -> bool))` | Boolean disjunction. |
