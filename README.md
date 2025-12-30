# 🦖

Rex (short for Rush Expressions) is a strongly-typed domain-specific functional
programming language for defining complex workflows in the Rush platform.

It comes with a set of built-in functions (like `map`, `fold`, `zip` etc) but
all other functions are defined and implemented by the host VM. These host
functions are typically high-performance computing modules that are intended to
be dispatched to supercomputers.

Right now, all of that is managed by a proprietary VM implementation (known as
`tengu`), but we will slowly be moving more and more of that logic into Rex
itself (mostly so that local dev is easier).

## Example

```rex
-- Double everything in a list
map (λx → 2 * x) [1, 2, 3, 4]

-- Or, if you prefer currying
map ((*) 2) [1, 2, 3, 4]
```

## Syntax Reference

- Programs are a single expression; whitespace (including newlines) is ignored outside of strings and comments. Block comments use `{- ... -}` and are stripped before parsing.
- Identifiers start with `_` or a letter and can contain numbers and underscores. Operators such as `+`, `-`, `*`, `/`, `==`, `<`, `>`, `++`, `&&`, `||`, and `.` are parsed as infix functions, so `(+) 2` and `(-) 4` work for partial application.
- Function application is left-associative: `f x y` is parsed as `(f x) y`. Parentheses control grouping when mixing applications and infix operators.
- Operator precedence (high to low): `.` > `* / %` > `+ - ++` > `== != < <= > >=` > `&&` > `||`.
- Core expression forms:
  - Literals: booleans, integers, floats, strings, UUIDs, datetimes.
  - Collections: tuples `(e1, e2)`, lists `[e1, e2]`, dictionaries `{ key = value }`.
  - Functions and control flow: lambdas `\x y -> body` (also accepts `λ` and `→`), let-in bindings `let x = e1, y = e2 in body`, and conditionals `if cond then a else b`.

## Let-in

We can assign variable names to expressions, allowing them to be re-used multiple times. This can help to simplify our Rex code. To create a variable, we need to use a let-in expression.

```rex
let 
    x = 1 + 2,
    y = 3
in
    x * y
```

It is important to note that variables are only accessible inside the let-in expression in which they are created. For example, the following code is not valid, and attempting to execute it will result in an error:

```rex
(let x = 1 + 2 in x * 3) * x
```

## Tuples, Lists, and Dictionaries

Rex supports the following collection types: tuples, lists, and dictionaries. Tuple are collections where each element can be a different type. Lists are collections where every element must be the same type. Dictionaries are collections that map an explicit name to an element, where each element can be a different type (you can think of them like "named tuples").

### Tuples

We create tuples using parentheses:

```rex
("this is a tuple", 420, true)
```

We can also use the `get` function to get specific elements from the tuple. For example:

```rex
let
    tuple = ("this is a ", 420, true)
in
    (get 0 tuple) ++ "tuple"
```

will result in the value `"this is a tuple"` (we are using the `++` concatentation operator, which works on strings and lists).

### Lists

We create lists using brackets:

```rex
["this", "is", "a", "list", "of", "strings" ]
```

Lists can also be constructed explicitly using the prelude constructors:

```rex
Cons "this" (Cons "is" (Cons "a" Empty))
```

Similar to tuples, we can use the `get` function to get specific elements from the list:

```rex
let
    list = ["this", "is", "a", "list", "of", "strings"]
in
    (get 0 list) ++ " " ++ (get 1 list) ++ "a string"
```

We can also use the `take` function to take a sub-list from the front of the list. For example:

```rex
let
    list = ["this", "is", "a", "list", "of", "strings"]
in
    take 3 list
```

will return `["this", "is", "a"]`. We can combine this with `skip` to take sub-lists from deeper in the list. For example:

```rex
let
    list = ["this", "is", "a", "list", "of", "strings"]
in
    take 2 (skip 2 list)
```

will return `["a", "list"]`.

### Dictionaries

We create dictionaries using braces:

```rex
{ key1: "value1", key2: 420, key3: true }
```

## Lambda Functions

Rex allows you to define your own functions (also known as lambdas). These lambdas can accept any number of variables, and define an expression applied to those variables. You define a lambda by writing the `\` or `λ` characters, naming your variables, writing the `->` or `→` characters, and then writing the body of the lambda. Let's see an example:

```rex
(λ x y → x + y) 2 3
```

This defines a lambda that accepts 2 variables, `x` and `y`, that, when called, will add them together. We then immediately call this lambda using the values `2` and `3`. We can mix lambdas with let-in expressions to name our lambdas:

```rex
let
    quad_eq_pos = λ a b c →   (sqrt (b * b + 4 * a * c) - b) / (2 * a),
    quad_eq_neg = λ a b c → - (sqrt (b * b + 4 * a * c) + b) / (2 * a),
    a = 1,
    b = 0,
    c = -1
in
    (quad_eq_pos a b c, quad_eq_neg a b c)
```

This expression produces the solutions for the quadratic equation `x^2 - 1`.

## If-then-else

Sometimes you want to execute different code depending on a condition. This is done using the if-then-else construct. Consider the following expression:

```rex
λ x → if x >= 0 then "positive" else "negative"
```

This expression defines a lambda function that takes a number x as input and returns "positive" if x is greater than or equal to 0, and "negative" otherwise.

## Mapping

In most purely functional programming languages, mapping is a important technique for applying a function to every element in a list. Rex includes a built-in `map` function for doing this. Let's see it in action:

```rex
map (λ x → 2 * x) [0.5, 1.0, 1.5]
```

Running this expressions should return the list `[1.0, 2.0, 3.0]`. What's going on? Well, the first argument expected by `map` is a lambda function that accepts one argument and defines the transformation of that argument. In our example, the lambda `(λ x → 2 * x)` defines a multiplication by 2. The second argument expected by `map` is the list of values that we will apply this transformation to. In this case, we pass the list `[0.5, 1.0, 1.5]`. So the result is doubling every element in the list, resulting in `[1.0, 2.0, 3.0]`.

## Currying

Curring is a special technique for defining a function without explicitly creating a lambda. It is done by _partially_ calling a function. The result is yet another function that expects the remainder of the arguments. It is easiest to understand with an example:

```rex
let 
    triple = (*) 3
in
    (triple 3, triple 5, triple 11)
```

First, we define a new function called `triple` which is the result of _partially_ calling the multiplication `(*)` operator. Multiplication usually expects 2 arguments. So when we call it with only 1 argument, we get back a _new_ function that stores the first argument, and expects one more argument. Whatever argument it receives, it will multiple it with the first argument that was received. So in our example above, we would get the result `(9, 15, 33)`.

Another way to think about currying is that it's a short-hand for explicitly defining a lambda function:

```rex
let 
    triple = (λ x → 3 * x)
in
    (triple 3, triple 5, triple 11)
```

There is no difference between these two expressions. Both will result in `(9, 15, 33)`. Some people prefer `(*) 3` and some people prefer `λ x → 3 * x`. It is mostly a matter of taste. If we think that to our `map` example, we should simplify it:

```rex
map ((*) 2) [0.5, 1.0, 1.5]
```

This is shorter and -- for many people -- easier to read.

## Composition

Function composition is a more advanced technique that also allows us to simplify code and make it more readable. Put simply, you can think of function composition as creating a "pipeline" of function calls. Let's say we have 3 functions -- `f`, `g`, and `h` -- that we need to call one after the other: `f (g (h x))`. This works, but it is a little messy. Function composition allows us to re-write this as `(f . g . h) x`. This has far fewer parentheses and many people find it easier to read (especially in the functional programming community).

While this seems like a small optimization, it can be very helpful in siutations where `f`, `g`, and `h` have multiple arguments and we combined composition with currying.

```rex
(foo x y . bar a . baz t u v) my_value
```

More clearly says "apply foo and then bar and then baz" than:

```rex
foo x y (bar a (baz t u v my_value))
```

## Pattern Matching

`match` performs structural pattern matching without braces. It takes a scrutinee expression followed by one or more `when` arms:

```rex
match named
  when Ok x -> handle_ok x
  when Err e -> handle_err e
  when _ -> default
```

Supported patterns today:

- Wildcards: `_`
- Variables: `x`
- Named constructors with one or more subpatterns: `Ok x`, `Pair (Just a) (Just b)`, `Node left right`
- Lists with pattern elements: `[]`, `[x]`, `[x, y, z]`, `[head, _]` (any arity is allowed)
- Cons with pattern parts: `x:xs`, `_ : rest`, `(Cons h t) : xs`
- Dict key presence: `{foo, bar}` (keys are identifiers only)

Another example on lists:

```rex
match list
  when [] -> "empty"
  when [x] -> x
  when [x, y, z] -> z
  when x:xs -> xs
  when _ -> "catch-all"
```

## Record Field Projection

ADT variants can declare record fields with `{ field: Type }`. When a value is *definitely* that record variant, you can project fields with `x:field`.

Single-variant ADT:

```rex
type Boxed = Boxed { value: i32 }

let
  x = Boxed { value = 1 }
in
  x:value
```

Multi-variant ADT (projection is only valid in a branch that proves the constructor):

```rex
type MyADT = MyVariant1 { field1: i32 } | MyVariant2 i32

let
  x = MyVariant1 { field1 = 1 }
in
  match x
    when MyVariant1 { field1 } -> x:field1
    when MyVariant2 _ -> 0
```

## Standard Library (Prelude)

Rex ships with a small prelude of common helpers. The type system constrains these using type classes where appropriate.

- Arithmetic: `+`, `-`, `*`, `/`, `negate`, `zero`, `one`
- Equality: `==`, `!=`
- Ordering: `<`, `<=`, `>`, `>=`
- Booleans: `&&`, `||`
- Collections (List/Array/Option/Result): `map`, `fold`, `foldl`, `foldr`, `filter`, `filter_map`, `flat_map`, `sum`, `mean`, `count`, `take`, `skip`, `zip`, `unzip`, `min`, `max`, `and_then`, `or_else`
- Option/Result helpers: `is_some`, `is_none`, `is_ok`, `is_err`

## Contribute

Made with ♡ by QDX
