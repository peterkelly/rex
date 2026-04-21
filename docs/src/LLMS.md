# LLMs

## Introduction and Rationale

Rex includes a semantic assistance layer designed first for machine clients that generate code,
especially LLM agents, and second for humans using an editor. This ordering is deliberate. LLMs are
fast at proposing code but weak at maintaining a precise internal model of a language's static
semantics over many edits. A practical system therefore externalizes semantic reasoning into stable,
tool-facing interfaces that can be queried repeatedly. Human users still benefit from the same
machinery, but the core design target is iterative machine control: propose code, observe structured
feedback, apply a constrained repair, and repeat.

A key design decision is to prioritize structured outputs over prose. Natural-language diagnostics
are useful for people, but brittle for agents. Rex exposes semantic information and quick-fix data
through explicit command contracts so that an LLM can operate as a controller over the typechecker
and editor transformations rather than as a parser of unstructured text.

## Typed Holes as a Control Primitive

The center of the workflow is the typed hole, written as `?`. A hole allows partial programs to be
represented directly in source code. Instead of treating incompleteness as a syntax error, Rex keeps
the program parseable and infers constraints around the missing expression.

This shifts generation from "write final code in one pass" to "write a scaffold, then solve local
obligations." For LLMs, this is a better fit: the model can produce a coarse structure, ask for the
expected type at the hole, retrieve candidate repairs, and select one.

```rex,interactive
fn parse_ph : string -> Result f32 string = \raw ->
  if raw == "7.3" then Ok 7.3 else Err "bad reading"

fn classify_ph : f32 -> string = \ph ->
  if ph < 6.8 then "acidic"
  else if ph > 7.8 then "alkaline"
  else "stable"

fn qc_label_from_sensor : string -> Result string string = \raw ->
  match (parse_ph raw)
    when Ok ph -> Ok (classify_ph ph)
    when Err e -> Err e

let sensor_reading = "7.3" in
let qc_label : Result string string = ? in
qc_label
```

In an LSP-enabled editor (including the browser playground), placing the cursor on `?` exposes
hole-filling actions and semantic candidates such as `qc_label_from_sensor sensor_reading`. The
expected type at the hole is `Result string string`, so the model can fill a semantically meaningful
real-world step without guessing. The same machinery is consumed by VS Code and by external LLM
tooling.

## Semantic Loop Endpoints

Rex provides semantic commands that return JSON-shaped data for program state at a position. The
most important operation is a single semantic loop step, which reports expected and inferred types,
in-scope values, candidate functions and adapters, local diagnostics, quick-fixes, and hole metadata.

From a control-systems viewpoint, this is an observation function over the current text. Separate
commands apply a selected quick-fix by identifier, or repeatedly apply best-ranked quick-fixes in
bulk mode. Bulk mode also supports a dry-run option so agents can preview predicted text without
committing edits.

The intended loop is simple: observe, choose, apply, re-observe. This structure is robust because it
avoids fragile prompt-only planning and continuously re-anchors decisions in the compiler's current
state.

## Candidate Narrowing and Adapter-Aware Repair

Candidate generation is hole-targeted and type-directed. Rex prefers functions whose result type can
satisfy the local expected type and attempts to satisfy function arguments from in-scope values. When
no direct value exists for an argument, Rex can propose single-step adapter expressions derived from
in-scope functions.

This does not prove semantic correctness. It proves local type plausibility and improves search
efficiency. The mechanism narrows the action space; it does not replace domain reasoning.

```rex,interactive
fn mk : i32 -> string = \n -> "value"
let x = 1 in
let y : string = ? in
y
```

In the editor, the hole can be filled with a candidate such as `mk x`, generated from local type
compatibility and in-scope bindings.

## Bulk Repair, Dry Runs, and Contracts

Rex supports multi-step quick-fix application around a cursor location. Bulk repair is useful for
agents because it can reduce several local errors in one command while returning telemetry about what
changed at each step. Dry-run mode computes the same sequence but reports predicted output without
mutating source text.

The semantic endpoints use a stable JSON contract with regression tests. This matters operationally:
agents are software clients, and software clients break when response schemas drift. Contract tests
convert "it usually works" into "it remains parseable across refactors."

## Resource Bounds and Adversarial Inputs

Semantic assistance can become expensive when scope size is large. To keep the system usable under
load and safer for embedded deployments, Rex enforces explicit limits in semantic candidate
pipelines, including caps on scanned environment schemes, in-scope values, candidate list sizes, and
hole-report counts. This is a pragmatic defense against unbounded CPU and output growth in LSP-side
analysis.

These bounds are not a complete security model. They should be combined with host-level gas budgets,
timeouts, concurrency limits, and request-rate controls in production embeddings.

## Trying the Workflow in the Browser Playground

The interactive playground has full LSP support, so this chapter can be exercised directly in the
browser. Paste a snippet with a hole, place the cursor on the hole, and inspect available quick-fixes
and semantic suggestions.

```rex,interactive
fn parse_i32 : string -> Result string i32 = \s ->
  if s == "42" then Ok 42 else Err "bad-int"

fn plus1 : i32 -> i32 = \n -> n + 1

let input = "42" in
let out : Result string i32 = ? in
out
```

A useful exercise is to fill `out` in multiple ways, observe type errors, then invoke semantic
quick-fixes and compare outcomes.

## Related Work and Positioning

The ideas used here are mostly established. Typed holes and goal-directed development are prominent
in systems such as GHC (Haskell) and dependently typed environments like Agda and Idris. Live,
structure-aware editor semantics have been explored in research systems such as Hazel. Type-directed
code search and synthesis has a long line of work, including tools like InSynth and later synthesis
frameworks.

Rex does not claim conceptual novelty in these foundations. Its contribution is engineering
integration: one semantics pipeline serving both human editor workflows and LLM control loops, with
contract-stable machine interfaces, regression coverage, and bounded candidate generation.

## Reference: Semantic Assists

Rex exposes the following assists through LSP execute commands. Each assist is intended to be used
in a short observe-then-act loop rather than as a one-shot oracle.

The argument forms below use JSON types and a 0-based `Position`.

Common types:

```ts
type UriArg =
  | { uri: string }
  | [uri: string];

type PosArg =
  | { uri: string; line: u32; character: u32 }
  | [uri: string, line: u32, character: u32];

type DiagnosticLite = {
  message: string;
  line: u32;
  character: u32;
};

type QuickFix = {
  id: string;
  title: string;
  kind: string | null;
  edit: WorkspaceEdit | null;
};

type HoleInfo = {
  name: string;
  line: u32;
  character: u32;
  expectedType: string;
};
```

`rex.expectedTypeAt`

```ts
args: PosArg
returns: null | { expectedType: string }
```

`rex.functionsProducingExpectedTypeAt`

```ts
args: PosArg
returns: { items: string[] } // each item rendered as "name : type"
```

`rex.functionsAcceptingInferredTypeAt`

```ts
args: PosArg
returns: {
  inferredType: string | null;
  items: string[];
}
```

`rex.adaptersFromInferredToExpectedAt`

```ts
args: PosArg
returns: {
  inferredType: string | null;
  expectedType: string | null;
  items: string[];
}
```

`rex.functionsCompatibleWithInScopeValuesAt`

```ts
args: PosArg
returns: { items: string[] } // concrete call-style suggestions
```

`rex.holesExpectedTypes`

```ts
args: UriArg
returns: { holes: HoleInfo[] }
```

`rex.semanticLoopStep`

```ts
args: PosArg
returns: {
  expectedType: string | null;
  inferredType: string | null;
  inScopeValues: string[];
  functionCandidates: string[];
  holeFillCandidates: Array<{ name: string; replacement: string }>;
  functionsAcceptingInferredType: string[];
  adaptersFromInferredToExpectedType: string[];
  functionsCompatibleWithInScopeValues: string[];
  localDiagnostics: DiagnosticLite[];
  quickFixes: QuickFix[];
  quickFixTitles: string[];
  holes: HoleInfo[];
}
```

`rex.semanticLoopApplyQuickFixAt`

```ts
args:
  | { uri: string; line: u32; character: u32; id: string }
  | [uri: string, line: u32, character: u32, id: string]
returns: null | { quickFix: QuickFix }
```

`rex.semanticLoopApplyBestQuickFixesAt`

```ts
args:
  | {
      uri: string;
      line: u32;
      character: u32;
      maxSteps?: u64;
      strategy?: "conservative" | "aggressive";
      dryRun?: bool;
    }
  | [
      uri: string,
      line: u32,
      character: u32,
      maxSteps?: u64,
      strategy?: string,
      dryRun?: bool
    ]
// maxSteps is clamped to [1, 20]

returns: {
  strategy: "conservative" | "aggressive";
  dryRun: bool;
  appliedQuickFixes: QuickFix[];
  appliedCount: u64;
  steps: Array<{
    index: u64;
    quickFix: QuickFix;
    diagnosticsBefore: DiagnosticLite[];
    diagnosticsAfter: DiagnosticLite[];
    diagnosticsBeforeCount: u64;
    diagnosticsAfterCount: u64;
    diagnosticsDelta: i64;
    noImprovementStreak: u64;
  }>;
  updatedText: string;
  localDiagnosticsAfter: DiagnosticLite[];
  stoppedReason: string;
  stoppedReasonDetail: string;
  lastDiagnosticsDelta: i64;
  noImprovementStreak: u64;
  seenStatesCount: u64;
}
```

## Practical Generation Guidance (Legacy Checklist)

The remainder of this chapter preserves the practical generation checklist that was previously a
standalone LLM guidance page. It remains useful when an LLM is emitting Rex directly rather than
running a semantic loop command at each step.

### Recommended Context Order

When building or revising Rex code, read docs in this order:

1. This chapter (`LLMS.md`) for semantic-loop workflow and generation pitfalls.
2. `LANGUAGE.md` for syntax and everyday feature usage.
3. `SPEC.md` for locked behavior when edge cases matter.

### High-Value Rules

1. Use `fn` for top-level reusable functions; use `let` and `let rec` for local helpers.
2. For local mutual recursion, use comma-separated `let rec` bindings.
3. Use `x::xs` for list cons in both patterns and expressions (`x::xs` is equivalent to `Cons x xs`).
4. Validate snippets with the Rex CLI before shipping docs.

### Quick Generation Checklist

Before returning generated Rex code:

1. Put top-level reusable functions in `fn` declarations (they are mutually recursive).
2. Use `let rec` only for local recursive helpers inside expressions.
3. Add annotations where constructor or numeric ambiguity is likely (`Empty`, `zero`, overloaded methods).
4. Ensure the final expression returns a visible result (often a tuple for demos).
5. Run `cargo run -p rex-cli -- run /tmp/snippet.rex` and fix all parse and type errors.

### Syntax Pitfalls

#### 1) Recursion model

- Top-level `fn` declarations are mutually recursive.
- Single recursive local helper: `let rec`
- Mutually recursive local helpers: `let rec` with commas between bindings.

Top-level mutual recursion:

```rex,interactive
fn even : i32 -> bool = \n ->
  if n == 0 then true else odd (n - 1)

fn odd : i32 -> bool = \n ->
  if n == 0 then false else even (n - 1)

even 10
```

```rex,interactive
let rec
  even = \n -> if n == 0 then true else odd (n - 1),
  odd = \n -> if n == 0 then false else even (n - 1)
in
  even 10
```

If you define local helpers in plain `let` and reference each other, you will get unbound-variable
errors. Use `let rec` for local recursion.

#### 2) List construction and list patterns

- Pattern matching: `x::xs` is valid in `when` patterns.
- Expression construction: `x::xs` and `Cons x xs` are equivalent (list literals are also valid).
  `Cons` uses normal constructor and function call style (`Cons head tail`).

Equivalent:

```rex
x::xs
Cons x xs
```

#### 3) ADT equality is not implicit

Do not assume custom ADTs automatically implement `Eq`. For example, comparing `Node` values with
`==` can fail with a missing-instance type error.

For small enums and ADTs, write an explicit equality helper:

```rex
node_eq = \a b ->
  match (a, b)
    when (A, A) -> true
    when (B, B) -> true
    when _ -> false
```

Related: avoid checking list emptiness with direct equality like `xs == []` in generic code. Prefer
an explicit matcher helper.

#### 4) Ambiguous constructors (for example `Empty`)

Constructors like `Empty` can be ambiguous when multiple ADTs define the same constructor name (for
example `List.Empty` and `Tree.Empty`).

Disambiguate with an annotation at the binding site:

```rex,interactive
type Tree = Empty | Node { key: i32, left: Tree, right: Tree }

let
  t0: Tree = Empty
in
  match t0
    when Empty -> 0
    when Node {key, left, right} -> key
```

#### 5) Reserved identifiers

Avoid bindings that collide with keywords (for example `as`). Use alternatives like `xs1`, `lefts`,
`rest1`, and similar.

#### 6) Constructor patterns with literals

Some forms like `Lit 1` inside nested patterns can fail to parse. Prefer simpler constructor
patterns and do literal checks in expression logic if needed.

Also avoid relying on tuple and list patterns that include numeric literals in one branch (for
example `(x::_, 0)`); match structurally first, then use an `if` guard in expression code.

### Validation Workflow

Before emitting generated Rex snippets in docs:

1. Save the snippet to a temporary `.rex` file.
2. Run `cargo run -p rex-cli -- run /tmp/snippet.rex`.
3. If parse or type errors appear, fix and re-run until clean.

For mdBook interactive demos, also run:

```sh
cd docs
mdbook build
```
