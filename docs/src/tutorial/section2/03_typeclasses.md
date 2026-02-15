# Type Classes: Defining Overloads

Type classes define a set of operations that can be implemented for many types.

Rex type classes are similar to Haskell’s: they are *compile-time* constraints with *runtime*
dictionary resolution.

## Defining a class

```rex,interactive
class Size a
  size : a -> i32
```

Method signatures can mention the class parameter `a` and any other types in scope.

### Optional `where`

You may also see:

```rex,interactive
class Size a where
  size : a -> i32
```

Both forms are accepted.

## Operators as methods

```rex
class Eq a
  == : a -> a -> bool
  != : a -> a -> bool
```

## Superclasses

Superclasses use `<=` (read “requires”):

```rex
class Ord a <= Eq a
  < : a -> a -> bool
```

If you have an `Ord a`, you also must have an `Eq a` instance.

## Multi-parameter classes (tupled)

Some prelude classes logically take multiple type parameters, such as `Indexable t a`.

In Rex source you write:

```rex
class Indexable t a
  get : i32 -> t -> a
```

In `where` constraints, multi-parameter classes are written using a tuple:

```rex
where Indexable (t, a) -> ...
```

This matches the implementation model described in [Specification](../../SPEC.md).
