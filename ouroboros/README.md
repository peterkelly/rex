## Compatibility

Types are able to check for compatibility with a value. Right now, values are assumed to be JSON values, but this is subject to change in the future to allow for more complex values (and streaming).

Value compatiblity is fairly self-explanatory:

```rs
assert!(i32::t().is_compat(&json!(69i32)));
assert!(Vec::<i32>::t().is_compat(&json!([69i32, 420i32])));
assert!(<(i32, bool, String)>::t().is_compat(&json!([69i32, true, "hello, world!"])));
```

There are some special cases. Functions are not truly able to be passed by value, and so we identify them purely by the presence of the `"λ"` key (this strange UTF8 character was chosen because it is unlikely to be present in most normal JSON objects).

```rs
assert!(Func::new(i32::t(), i32::t()).is_compat(json!({ "λ": "anything can go here", "other": "keys", "are": "allowed too" })));
```

You'll notice that — other than the `"λ"` key — other keys are also allowed. This is done to support arbitrary ways on implementing function values. Ouroboros ships with a default function value — known as a lambda — that maps `"λ"` to a function name. It also contains `"extras"` and `"captured_args"` allowing for arbitrary application-specific data and currying respectively.

## Lambdas

Passing lambdas around as first-class citizens can get pretty hairy. In Ouroboros, the function type can only have one input type (`A`) and one output type (`B`). If you want a function that takes multiple input arguments then you need to express that `A` is a tuple. For example the function type for multiplication nmight look like `Func::new(<(i32, i32)>::t(), i32::t())`.

Lambda values that have tuples (with more than one element) as an input type can be curried by passing in only a subset of their required values. For example:

```rs
let foo = Lambda::<(i32, i32), i32>::new("mul_i32");
assert_eq!(&foo.n, "mul_i32");
assert_eq!(&bar.captured_args, vec![]);
assert_eq!(foo.type_info(), Func::new(<(i32, i32)>::t(), i32::t()));

let bar = foo.curry(69i32);
assert_eq!(&bar.n, "mul_i32");
assert_eq!(&bar.captured_args, vec![json!(69i32)]);
assert_eq!(bar.type_info(), Func::new(i32::t(), i32::t()));
```

Notice that the name of the lambda value has not changed, although it has now captured the argument that was passed to it.

## Lambdas vs Functions

Lambdas are the value, functions are the type. It is a little bit gross, but it is less gross than constantly being confused about what to do when importing them.

## Building WASM

To build WASM for the Ouroboros VM, you can run (from the root of the repository)
`cargo build --package ouroboros-vm-prelude --target wasm32-unknown-unknown --no-default-features`
