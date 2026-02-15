# Operators and Precedence

Operators like `+` and `==` are just functions with infix syntax.

## Using an operator as a value

Parentheses turn an operator into a function value:

```rex,interactive
(+) 1 2
```

This enables partial application:

```rex,interactive
map ((*) 2) [1, 2, 3]
```

## Operators come from type classes

Many operators are methods on prelude classes:

- `+` / `zero` from `AdditiveMonoid`
- `*` / `one` from `MultiplicativeMonoid`
- `==` / `!=` from `Eq`
- ordering from `Ord`

This is why you can write `+` for both numbers *and* strings.

## Precedence

Rex has a fixed precedence table (see [Language Reference](../../LANGUAGE.md)). A good habit is to use parentheses
whenever you mix application with multiple infix operators.

```rex,interactive
(1 + 2) * 3
```

### Record projection is not an operator

`x.field` is *field projection syntax*, not an operator you can partially apply:

```rex,interactive
let u = { name = "Ada" } in u.name
```
