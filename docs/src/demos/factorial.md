# Demo: Factorial

This demo computes `n!` using direct recursion: it multiplies `n` by the factorial of `n - 1` until it reaches the base case `0`, which returns `1`. It is a minimal example of structural recursion over integers and shows how a simple mathematical definition maps directly to Rex function syntax.

Related reading: [Factorial](https://en.wikipedia.org/wiki/Factorial).

The implementation is centered on `fact`, which uses an `if` expression to separate the base case from the recursive case. The final line evaluates `fact 6`, so the output is a single integer result; this keeps the demo focused on call structure and termination rather than data modeling.

```rex,interactive
fn fact : i32 -> i32 = \n ->
  if n == 0
  then 1
  else n * fact (n - 1)

fact 6
```
