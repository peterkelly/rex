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
| `List a` | Immutable ordered sequence. Constructors: `List.Empty`, `List.Cons`. |
| `Option a` | Optional value (`Some` or `None`). Constructors: `Option.Some`, `Option.None`. |
| `Ordering` | Three-way comparison result (`Less`, `Equal`, or `Greater`). Constructors: `Ordering.Less`, `Ordering.Equal`, `Ordering.Greater`. |
| `Result a b` | Result value (`Ok` or `Err`) for success/failure flows. Constructors: `Result.Err`, `Result.Ok`. |
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

## Reading Function Entries

Rex functions are curried, so `f first second` can be partially applied as `f first`. The **Call** paragraph gives every parameter a stable, descriptive name. The **Type** paragraph gives the inferred Rex type; type variables begin with an apostrophe. For overloaded functions, **Available for** lists the built-in types and type constructors that provide the operation.

## Boolean Operations

Boolean operators combine two `Bool` values.

### `&&`

**Call:** `left && right`

**Type:** `Bool -> Bool -> Bool`

Returns `true` only when both `left` and `right` are `true`.

### `||`

**Call:** `left || right`

**Type:** `Bool -> Bool -> Bool`

Returns `true` when either `left` or `right` is `true`.

## Comparison Operations

Equality is available for all listed equality-comparable types. Ordering operations are available for numbers, characters, and strings.

### `==`

**Call:** `left == right`

**Type:** `'a -> 'a -> Bool`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `Bool`, `Char`, `String`, `UUID`, `Hash`, `DateTime`, `List 'a`, `Option 'a`, `Result 'a 'e`

Returns `true` when `left` and `right` are equal. Lists, options, and results require their contained types to support equality.

### `!=`

**Call:** `left != right`

**Type:** `'a -> 'a -> Bool`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `Bool`, `Char`, `String`, `UUID`, `Hash`, `DateTime`, `List 'a`, `Option 'a`, `Result 'a 'e`

Returns `true` when `left` and `right` are not equal. Lists, options, and results require their contained types to support equality.

### `<`

**Call:** `left < right`

**Type:** `'a -> 'a -> Bool`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `Char`, `String`

Returns `true` when `left` is less than `right`.

### `<=`

**Call:** `left <= right`

**Type:** `'a -> 'a -> Bool`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `Char`, `String`

Returns `true` when `left` is less than or equal to `right`.

### `>`

**Call:** `left > right`

**Type:** `'a -> 'a -> Bool`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `Char`, `String`

Returns `true` when `left` is greater than `right`.

### `>=`

**Call:** `left >= right`

**Type:** `'a -> 'a -> Bool`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `Char`, `String`

Returns `true` when `left` is greater than or equal to `right`.

### `cmp`

**Call:** `cmp left right`

**Type:** `'a -> 'a -> Ordering`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `Char`, `String`

Returns `Less`, `Equal`, or `Greater` according to the ordering of `left` relative to `right`.

## Ordering Values

`cmp` returns one of these `Ordering` values. The bare names are convenient aliases for the type-qualified constructors.

### `Ordering.Less`

**Call:** `Ordering.Less`

**Type:** `Ordering`

The type-qualified `Ordering` value returned when the left value is less than the right value.

### `Ordering.Equal`

**Call:** `Ordering.Equal`

**Type:** `Ordering`

The type-qualified `Ordering` value returned when two values are equal.

### `Ordering.Greater`

**Call:** `Ordering.Greater`

**Type:** `Ordering`

The type-qualified `Ordering` value returned when the left value is greater than the right value.

### `Less`

**Call:** `Less`

**Type:** `Ordering`

The `Ordering` value returned when the left value is less than the right value.

### `Equal`

**Call:** `Equal`

**Type:** `Ordering`

The `Ordering` value returned when two values are equal.

### `Greater`

**Call:** `Greater`

**Type:** `Ordering`

The `Ordering` value returned when the left value is greater than the right value.

## Arithmetic and Aggregation

Arithmetic operators work on the numeric types listed for each operation. Some operations are also overloaded for lists or strings where noted.

### `zero`

**Call:** `zero`

**Type:** `'a`

**Available for:** `List 'a`, `String`, `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`

Returns the additive identity for the inferred result type.

### `one`

**Call:** `one`

**Type:** `'a`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`

Returns the multiplicative identity for the inferred result type.

### `+`

**Call:** `left + right`

**Type:** `'a -> 'a -> 'a`

**Available for:** `List 'a`, `String`, `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`

Adds `left` and `right`; for lists and strings, concatenates `left` with `right`.

### `-`

**Call:** `left - right`

**Type:** `'a -> 'a -> 'a`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`

Subtracts `right` from `left`.

### `*`

**Call:** `left * right`

**Type:** `'a -> 'a -> 'a`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`

Multiplies `left` by `right`.

### `/`

**Call:** `left / right`

**Type:** `'a -> 'a -> 'a`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`

Divides `left` by `right`.

### `%`

**Call:** `left % right`

**Type:** `'a -> 'a -> 'a`

**Available for:** `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`

Returns the remainder after dividing `left` by `right`.

### `negate`

**Call:** `negate value`

**Type:** `'a -> 'a`

**Available for:** `i8`, `i16`, `i32`, `i64`, `f32`, `f64`

Returns the additive inverse of `value`.

### `sum`

**Call:** `sum values`

**Type:** `'f 'a -> 'a`

**Available for:** `List 'a` and `Option 'a`, where `'a` is a numeric type, `String`, or another `List` type

Combines all elements in `values` using addition, beginning with that element type's additive identity.

### `mean`

**Call:** `mean values`

**Type:** `'f 'a -> 'a`

**Available for:** `List f32`, `List f64`, `Option f32`, and `Option f64`

Returns the arithmetic mean of `values`; raises an error when `values` is empty.

### `min`

**Call:** `min values`

**Type:** `'f 'a -> 'a`

**Available for:** `List 'a` and `Option 'a`, where `'a` is a numeric type, `Char`, or `String`

Returns the least element in `values` according to its ordering; raises an error when `values` is empty.

### `max`

**Call:** `max values`

**Type:** `'f 'a -> 'a`

**Available for:** `List 'a` and `Option 'a`, where `'a` is a numeric type, `Char`, or `String`

Returns the greatest element in `values` according to its ordering; raises an error when `values` is empty.

## General Value Functions

Functions for constructing defaults, parsing strings, and rendering values.

### `default`

**Call:** `default`

**Type:** `'a`

**Available for:** `Bool`, `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `Char`, `String`, `List 'a`, `Option 'a`, `Result 'a 'e`

Returns the canonical default value for the inferred result type. For `Result a e`, the success type `a` must also have a default.

### `parse`

**Call:** `parse input`

**Type:** `String -> Option 'a`

**Available for:** `Bool`, `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `Char`, `UUID`, `Hash`, `DateTime`

Attempts to convert `input` to the result type selected by context, returning `Some value` on success or `None` for malformed or out-of-range input.

### `show`

**Call:** `show value`

**Type:** `'a -> String`

**Available for:** `Bool`, `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `Char`, `String`, `UUID`, `Hash`, `DateTime`, `List 'a`, `Option 'a`, `Result 'a 'e`

Renders `value` as a human-readable string. Lists, options, and results require their contained types to be renderable.

## Collection and Container Functions

Generic operations shared by lists, options, results, dictionaries, or strings. Check the availability list on each function.

### `length`

**Call:** `length value`

**Type:** `'a -> u64`

**Available for:** `List 'a`, `Dict 'a`, `String`

Returns the number of list elements, dictionary entries, or string Unicode scalar values in `value`.

### `map`

**Call:** `map transform container`

**Type:** `('a -> 'b) -> 'f 'a -> 'f 'b`

**Available for:** `List`, `Option`, `Result 'e`, `Dict`

Applies `transform` to every value in `container` while preserving its structure.

### `filter`

**Call:** `filter predicate container`

**Type:** `('a -> Bool) -> 'f 'a -> 'f 'a`

**Available for:** `List`, `Option`, `Dict`

Keeps each value in `container` for which `predicate` returns `true`.

### `filter_map`

**Call:** `filter_map transform container`

**Type:** `('a -> Option 'b) -> 'f 'a -> 'f 'b`

**Available for:** `List`, `Option`, `Dict`

Applies `transform` to each value in `container` and drops every result that is `None`.

### `foldl`

**Call:** `foldl step initial container`

**Type:** `('b -> 'a -> 'b) -> 'b -> 't 'a -> 'b`

**Available for:** `List`, `Option`

Strictly reduces `container` from left to right by applying `step` to the accumulator and each value, beginning with `initial`.

### `foldr`

**Call:** `foldr step initial container`

**Type:** `('a -> 'b -> 'b) -> 'b -> 't 'a -> 'b`

**Available for:** `List`, `Option`

Reduces `container` from right to left by applying `step` to each value and the accumulator, beginning with `initial`.

### `fold`

**Call:** `fold step initial container`

**Type:** `('b -> 'a -> 'b) -> 'b -> 't 'a -> 'b`

**Available for:** `List`, `Option`

Reduces `container` from left to right by applying `step` to the accumulator and each value, beginning with `initial`.

### `pure`

**Call:** `pure value`

**Type:** `'a -> 'f 'a`

**Available for:** `List`, `Option`, `Result 'e`

Wraps `value` in the inferred container type.

### `ap`

**Call:** `ap functions values`

**Type:** `'f ('a -> 'b) -> 'f 'a -> 'f 'b`

**Available for:** `List`, `Option`, `Result 'e`

Applies the wrapped functions in `functions` to the wrapped values in `values`.

### `bind`

**Call:** `bind transform container`

**Type:** `('a -> 'm 'b) -> 'm 'a -> 'm 'b`

**Available for:** `List`, `Option`, `Result 'e`

Applies `transform` to each successful value in `container` and flattens the resulting container layer.

### `or_else`

**Call:** `or_else fallback value`

**Type:** `('f 'a -> 'f 'a) -> 'f 'a -> 'f 'a`

**Available for:** `List`, `Option`, `Result 'e`

Returns `value` when it is non-empty, present, or successful; otherwise applies `fallback` to `value`.

## List Functions

Construct, index, slice, and combine `List` values.

### `List.Empty`

**Call:** `List.Empty`

**Type:** `List 'a`

The type-qualified empty list constructor.

### `List.Cons`

**Call:** `List.Cons head tail`

**Type:** `'a -> List 'a -> List 'a`

Constructs a non-empty list whose first element is `head` and whose remaining elements are `tail`.

### `Empty`

**Call:** `Empty`

**Type:** `List 'a`

The empty list constructor.

### `Cons`

**Call:** `Cons head tail`

**Type:** `'a -> List 'a -> List 'a`

Constructs a non-empty list whose first element is `head` and whose remaining elements are `tail`.

### `list_get`

**Call:** `list_get index list`

**Type:** `u64 -> List 'a -> Option 'a`

Returns the element at zero-based `index`, or `None` when `index` is out of bounds.

### `list_slice`

**Call:** `list_slice start end list`

**Type:** `u64 -> u64 -> List 'a -> Option (List 'a)`

Returns the half-open range `start..end`, or `None` for invalid bounds.

### `list_reverse`

**Call:** `list_reverse list`

**Type:** `List 'a -> List 'a`

Returns the elements of `list` in reverse order.

### `list_concat`

**Call:** `list_concat lists`

**Type:** `List (List 'a) -> List 'a`

Concatenates the nested `lists` into one list while preserving their order.

### `list_repeat`

**Call:** `list_repeat count value`

**Type:** `u64 -> 'a -> List 'a`

Returns a list containing `count` copies of `value`.

### `list_any`

**Call:** `list_any predicate list`

**Type:** `('a -> Bool) -> List 'a -> Bool`

Returns whether `predicate` is `true` for any element, stopping at the first `true` result.

### `list_all`

**Call:** `list_all predicate list`

**Type:** `('a -> Bool) -> List 'a -> Bool`

Returns whether `predicate` is `true` for every element, stopping at the first `false` result.

### `list_find`

**Call:** `list_find predicate list`

**Type:** `('a -> Bool) -> List 'a -> Option 'a`

Returns the first element for which `predicate` returns `true`, or `None` when no element matches.

### `list_find_index`

**Call:** `list_find_index predicate list`

**Type:** `('a -> Bool) -> List 'a -> Option u64`

Returns the zero-based index of the first matching element, or `None` when no element matches.

### `list_count`

**Call:** `list_count predicate list`

**Type:** `('a -> Bool) -> List 'a -> u64`

Returns the number of elements for which `predicate` returns `true`.

### `list_partition`

**Call:** `list_partition predicate list`

**Type:** `('a -> Bool) -> List 'a -> (List 'a, List 'a)`

Returns matching and non-matching elements as a pair of lists, preserving their relative order.

### `take`

**Call:** `take count list`

**Type:** `u64 -> 'f 'a -> 'f 'a`

**Available for:** `List`

Returns at most the first `count` elements from `list`.

### `skip`

**Call:** `skip count list`

**Type:** `u64 -> 'f 'a -> 'f 'a`

**Available for:** `List`

Drops the first `count` elements from `list`; when `count` exceeds its length, returns an empty list.

### `first`

**Call:** `first count list`

**Type:** `i32 -> List 'a -> List 'a`

Returns the first `count` elements of `list`; raises an error when `count` is out of range.

### `last`

**Call:** `last count list`

**Type:** `i32 -> List 'a -> List 'a`

Returns the last `count` elements of `list`; raises an error when `count` is out of range.

### `slice`

**Call:** `slice start end list`

**Type:** `i32 -> i32 -> List 'a -> List 'a`

Returns elements in the half-open range `start..end` from `list`; raises an error for out-of-range bounds or when `end < start`.

### `zip`

**Call:** `zip left right`

**Type:** `'f 'a -> 'f 'b -> 'f ('a, 'b)`

**Available for:** `List`

Pairs elements from `left` and `right` by position, stopping when either list ends.

### `unzip`

**Call:** `unzip pairs`

**Type:** `'f ('a, 'b) -> ('f 'a, 'f 'b)`

**Available for:** `List`

Splits the list of `pairs` into a pair of lists containing their first and second components.

## Dict Functions

Dictionary-specific operations. Keys are strings, dictionaries are immutable, and operations that modify a dictionary return a new value.

### `dict_empty`

**Call:** `dict_empty`

**Type:** `Dict 'a`

Constructs an empty dictionary.

### `dict_singleton`

**Call:** `dict_singleton key value`

**Type:** `String -> 'a -> Dict 'a`

Constructs a dictionary containing the single association from `key` to `value`.

### `dict_get`

**Call:** `dict_get key dictionary`

**Type:** `String -> Dict 'a -> Option 'a`

Looks up `key` in `dictionary`, returning `Some value` when present or `None` when absent.

### `dict_has`

**Call:** `dict_has key dictionary`

**Type:** `String -> Dict 'a -> Bool`

Returns whether `dictionary` contains `key`.

### `dict_insert`

**Call:** `dict_insert key value dictionary`

**Type:** `String -> 'a -> Dict 'a -> Dict 'a`

Returns `dictionary` with `key` associated with `value`, replacing the previous value when the key exists.

### `dict_remove`

**Call:** `dict_remove key dictionary`

**Type:** `String -> Dict 'a -> Dict 'a`

Returns `dictionary` without `key`.

### `dict_update`

**Call:** `dict_update key update dictionary`

**Type:** `String -> (Option 'a -> Option 'a) -> Dict 'a -> Dict 'a`

Calls `update` with the optional current value for `key`; returning `Some value` inserts or replaces the entry, while returning `None` removes it.

### `dict_is_empty`

**Call:** `dict_is_empty dictionary`

**Type:** `Dict 'a -> Bool`

Returns whether `dictionary` has no entries.

### `dict_keys`

**Call:** `dict_keys dictionary`

**Type:** `Dict 'a -> List String`

Returns the keys from `dictionary` in lexicographic order.

### `dict_values`

**Call:** `dict_values dictionary`

**Type:** `Dict 'a -> List 'a`

Returns the values from `dictionary` in lexicographic key order.

### `dict_entries`

**Call:** `dict_entries dictionary`

**Type:** `Dict 'a -> List (String, 'a)`

Returns the key/value tuples from `dictionary` in lexicographic key order.

### `dict_from_entries`

**Call:** `dict_from_entries entries`

**Type:** `List (String, 'a) -> Dict 'a`

Constructs a dictionary from `entries`; when a key occurs more than once, its later value wins.

### `dict_map`

**Call:** `dict_map transform dictionary`

**Type:** `((String, 'a) -> (String, 'b)) -> Dict 'a -> Dict 'b`

Applies `transform` to each `(key, value)` tuple in `dictionary`; later collisions in input-key order win.

### `dict_filter`

**Call:** `dict_filter predicate dictionary`

**Type:** `((String, 'a) -> Bool) -> Dict 'a -> Dict 'a`

Keeps the entries from `dictionary` for which `predicate` returns `true`; `predicate` receives each `(key, value)` tuple.

## String Functions

String indexing and positions count Unicode scalar values, not UTF-8 bytes. Functions with selector or modifier arguments place those arguments before the input string.

### `string_get`

**Call:** `string_get index input`

**Type:** `u64 -> String -> Option Char`

Returns the character at zero-based Unicode scalar `index` in `input`, or `None` when `index` is out of bounds.

### `string_slice`

**Call:** `string_slice start end input`

**Type:** `u64 -> u64 -> String -> Option String`

Returns the half-open Unicode scalar range `start..end` from `input`, or `None` for invalid bounds.

### `string_contains`

**Call:** `string_contains needle haystack`

**Type:** `String -> String -> Bool`

Returns whether `haystack` contains `needle` as a substring.

### `string_starts_with`

**Call:** `string_starts_with prefix input`

**Type:** `String -> String -> Bool`

Returns whether `input` starts with `prefix`.

### `string_ends_with`

**Call:** `string_ends_with suffix input`

**Type:** `String -> String -> Bool`

Returns whether `input` ends with `suffix`.

### `string_find`

**Call:** `string_find needle haystack`

**Type:** `String -> String -> Option u64`

Finds `needle` in `haystack`, returning its first Unicode scalar index or `None`.

### `string_split`

**Call:** `string_split separator input`

**Type:** `String -> String -> List String`

Splits `input` at each non-overlapping occurrence of `separator`.

### `string_join`

**Call:** `string_join separator parts`

**Type:** `String -> List String -> String`

Joins `parts`, inserting `separator` between adjacent strings.

### `string_replace`

**Call:** `string_replace needle replacement input`

**Type:** `String -> String -> String -> String`

Returns `input` with every non-overlapping occurrence of `needle` replaced by `replacement`.

### `string_trim`

**Call:** `string_trim input`

**Type:** `String -> String`

Removes Unicode whitespace from both ends of `input`.

### `string_trim_start`

**Call:** `string_trim_start input`

**Type:** `String -> String`

Removes Unicode whitespace from the start of `input`.

### `string_trim_end`

**Call:** `string_trim_end input`

**Type:** `String -> String`

Removes Unicode whitespace from the end of `input`.

### `string_to_lower`

**Call:** `string_to_lower input`

**Type:** `String -> String`

Converts `input` using the Unicode lowercase mapping.

### `string_to_upper`

**Call:** `string_to_upper input`

**Type:** `String -> String`

Converts `input` using the Unicode uppercase mapping.

### `string_to_chars`

**Call:** `string_to_chars input`

**Type:** `String -> List Char`

Returns the Unicode scalar values in `input` as a character list.

### `chars_to_string`

**Call:** `chars_to_string chars`

**Type:** `List Char -> String`

Concatenates the Unicode scalar values in `chars` into a string.

### `string_to_utf8`

**Call:** `string_to_utf8 input`

**Type:** `String -> List u8`

Encodes `input` as its UTF-8 byte sequence.

### `utf8_to_string`

**Call:** `utf8_to_string bytes`

**Type:** `List u8 -> Option String`

Decodes `bytes` as UTF-8, returning `None` when the byte sequence is invalid.

## Option and Result Functions

Construct, inspect, and extract optional values and success-or-error results. The bare constructor names are convenient aliases for the type-qualified forms.

### `Option.None`

**Call:** `Option.None`

**Type:** `Option 't`

The type-qualified empty `Option` constructor.

### `Option.Some`

**Call:** `Option.Some value`

**Type:** `'t -> Option 't`

Constructs a present `Option` containing `value` using its type-qualified name.

### `None`

**Call:** `None`

**Type:** `Option 't`

The empty `Option` constructor.

### `Some`

**Call:** `Some value`

**Type:** `'t -> Option 't`

Constructs a present `Option` containing `value`.

### `is_none`

**Call:** `is_none option`

**Type:** `Option 'a -> Bool`

Returns whether `option` is `None`.

### `is_some`

**Call:** `is_some option`

**Type:** `Option 'a -> Bool`

Returns whether `option` is `Some`.

### `Result.Err`

**Call:** `Result.Err error`

**Type:** `'e -> Result 't 'e`

Constructs a failed `Result` containing `error` using its type-qualified name.

### `Result.Ok`

**Call:** `Result.Ok value`

**Type:** `'t -> Result 't 'e`

Constructs a successful `Result` containing `value` using its type-qualified name.

### `Err`

**Call:** `Err error`

**Type:** `'e -> Result 't 'e`

Constructs a failed `Result` containing `error`.

### `Ok`

**Call:** `Ok value`

**Type:** `'t -> Result 't 'e`

Constructs a successful `Result` containing `value`.

### `is_err`

**Call:** `is_err result`

**Type:** `Result 't 'e -> Bool`

Returns whether `result` is `Err`.

### `is_ok`

**Call:** `is_ok result`

**Type:** `Result 't 'e -> Bool`

Returns whether `result` is `Ok`.

### `unwrap`

**Call:** `unwrap value`

**Types:** `Option 'a -> 'a`; `Result 't 'e -> 't`

Extracts the value from `Some` or `Ok`; raises an error when `value` is `None` or `Err`.
