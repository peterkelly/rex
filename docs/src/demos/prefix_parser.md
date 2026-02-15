# Demo: Prefix Parser + Evaluator

This demo performs recursive-descent parsing over a prefix token stream to build an expression tree, then evaluates that tree. It separates syntax (`Tok`) from semantics (`Expr`) and returns both the parsed expression and unconsumed tokens, which is a common parser design for composing larger grammars.

Related reading: [Polish notation](https://en.wikipedia.org/wiki/Polish_notation) and [Recursive descent parser](https://en.wikipedia.org/wiki/Recursive_descent_parser).

`parse_expr` is the parser entrypoint and consumes tokens according to constructor shape: numbers produce leaf nodes, while operators recursively parse the required subexpressions. `eval` then interprets the produced AST, and `is_empty` checks whether parsing consumed all tokens; the two sample token streams demonstrate both parsing and evaluation in one result tuple.

```rex,interactive
type Tok = TNum i32 | TPlus | TMul | TNeg
type Expr = Num i32 | Add Expr Expr | Mul Expr Expr | Neg Expr

fn parse_expr : List Tok -> (Expr, List Tok) = \toks ->
  match toks
    when [] -> (Num 0, [])
    when TNum n:rest -> (Num n, rest)
    when TPlus:rest ->
      let
        (lhs, rest1) = parse_expr rest,
        (rhs, rest2) = parse_expr rest1
      in
        (Add lhs rhs, rest2)
    when TMul:rest ->
      let
        (lhs, rest1) = parse_expr rest,
        (rhs, rest2) = parse_expr rest1
      in
        (Mul lhs rhs, rest2)
    when TNeg:rest ->
      let (inner, rest1) = parse_expr rest in
      (Neg inner, rest1)

fn eval : Expr -> i32 = \expr ->
  match expr
    when Num n -> n
    when Add a b -> eval a + eval b
    when Mul a b -> eval a * eval b
    when Neg x -> 0 - eval x

fn is_empty : List a -> bool = \xs ->
  match xs
    when [] -> true
    when _:_ -> false

let
  toks1 = [TPlus, TNum 2, TMul, TNum 3, TNum 4],
  toks2 = [TPlus, TNeg, TNum 3, TNum 10],
  (ast1, rest1) = parse_expr toks1,
  (ast2, rest2) = parse_expr toks2
in
  (eval ast1, is_empty rest1, eval ast2, is_empty rest2)
```
