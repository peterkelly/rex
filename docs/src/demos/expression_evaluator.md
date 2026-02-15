# Demo: Expression Evaluator

This demo evaluates arithmetic expressions represented as an AST with constructors for literals, addition, multiplication, and negation. Alongside evaluation, it computes expression depth and performs a small simplification pass, showing how one tree structure can support multiple independent recursive analyses and transformations.

Related reading: [Abstract syntax tree](https://en.wikipedia.org/wiki/Abstract_syntax_tree).

The code defines one ADT (`Expr`) and then reuses it across three traversals: `eval` computes numeric meaning, `depth` computes structural height, and `simplify_once` applies a local rewrite rule for double negation. In the final expression block, two sample trees are built, one is simplified once, and the output tuple shows evaluation and depth results side-by-side.

```rex,interactive
type Expr = Lit i32 | Add Expr Expr | Mul Expr Expr | Neg Expr

fn eval : Expr -> i32 = \e ->
  match e
    when Lit n -> n
    when Add a b -> eval a + eval b
    when Mul a b -> eval a * eval b
    when Neg x -> 0 - eval x

fn depth : Expr -> i32 = \e ->
  match e
    when Lit _ -> 1
    when Add a b ->
      if depth a > depth b then 1 + depth a else 1 + depth b
    when Mul a b ->
      if depth a > depth b then 1 + depth a else 1 + depth b
    when Neg x -> 1 + depth x

fn simplify_once : Expr -> Expr = \e ->
  match e
    when Neg (Neg x) -> x
    when _ -> e

let
  expr1 = Add (Lit 2) (Mul (Lit 3) (Lit 4)),
  expr2 = Neg (Neg (Add (Lit 5) (Mul (Lit 1) (Lit 9))))
in
  let simplified = simplify_once expr2 in
  (eval expr1, depth expr1, eval simplified)
```
