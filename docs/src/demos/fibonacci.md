# Demo: Fibonacci

This demo generates Fibonacci numbers with the classic recursive recurrence: each value is the sum of the previous two, with base cases at `0` and `1`. It illustrates branching recursion and builds a small prefix of the sequence so you can see the growth pattern directly in the output.

Related reading: [Fibonacci number](https://en.wikipedia.org/wiki/Fibonacci_number).

The `fib` function mirrors the mathematical recurrence directly: when `n` is `0` or `1` it returns immediately, otherwise it performs two recursive calls and adds their results. Instead of evaluating just one input, the demo computes a list from `fib 0` through `fib 10`, which makes it easy to confirm correctness across multiple cases in one run.

```rex,interactive
fn fib : i32 -> i32 = \n ->
  if n <= 1
  then n
  else fib (n - 1) + fib (n - 2)

[fib 0, fib 1, fib 2, fib 3, fib 4, fib 5, fib 6, fib 7, fib 8, fib 9, fib 10]
```
