# Algebraic Data Types (ADTs)

ADTs let you define your own sum types.

You’ll use ADTs to model “this or that” choices: optional values, tagged unions, trees, results,
etc.

## Simple ADT

```rex
type Maybe a = Just a | Nothing
```

Constructors are values:

```rex
(Just 1, Nothing)
```

### Using ADTs is all about `match`

Defining an ADT is only half the story; consuming it is done with pattern matching:

```rex
let
  fromMaybe = \d m ->
    match m
      when Just x -> x
      when Nothing -> d
in
  fromMaybe 0 (Just 5)
```

## Constructors with multiple fields

```rex
type Pair a b = Pair a b

Pair 1 "hi"
```

This is a single-constructor ADT (a “product type”). In many programs you’ll use record-carrying
constructors instead because they self-document field names.

## Record-carrying constructors

Variants can carry a record payload:

```rex
type User = User { name: string, age: i32 }

User { name = "Ada", age = 36 }
```

This style works well with field projection and update (covered later).

## Multi-variant and recursive ADTs

You can define sum types with multiple constructors, including recursive ones:

```rex
type Tree
  = Leaf { value: i32 }
  | Node { left: Tree, right: Tree }
```

Recursive ADTs are the foundation for ASTs, expression trees, and many structured data problems.
