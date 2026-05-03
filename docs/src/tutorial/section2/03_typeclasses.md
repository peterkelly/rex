# Type Classes: Defining Overloads

Type classes define a set of operations that can be implemented for many types.

Rex type classes are similar to Haskell’s: they are *compile-time* constraints with *runtime*
dictionary resolution.

## Defining a class

```rex,interactive
class Size a where {
  size : a -> i32;
}
```

Method signatures can mention the class parameter `a` and any other types in scope.

### Empty Classes

Classes with methods use `where { ... }`. Marker classes with no methods use a semicolon:

```rex,interactive
class Marker a;
```

## Operators as methods

```rex
class Eq a where {
  == : a -> a -> bool;
  != : a -> a -> bool;
}
```

## Superclasses

Superclasses use `<=` (read “requires”):

```rex
class Ord a <= Eq a where {
  < : a -> a -> bool;
}
```

If you have an `Ord a`, you also must have an `Eq a` instance.

## Multi-parameter classes (tupled)

Some prelude classes logically take multiple type parameters, such as `Indexable t a`.

In Rex source you write:

```rex
class Indexable t a where {
  get : i32 -> t -> a;
}
```

In `where` constraints, multi-parameter classes are written using a tuple:

```rex
where Indexable (t, a) -> ...
```

This matches the implementation model described in [Specification](../../SPEC.md).
