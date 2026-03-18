# Rex Documentation

Rex (short for *Rush Expressions*) is a strongly-typed, pure functional language built to be an excellent target for LLM-generated programs, with a focus on data processing. At a high level, you write transformations over lists, records, ADTs, and other values using familiar functional building blocks like `map`, `filter`, folds, pattern matching, and composition. The language is designed to make dataflow clear and predictable, with types and pure expressions doing most of the heavy lifting.

Rex is designed first and foremost to be embedded inside Rust applications. In that model, your Rust program acts as the host runtime and injects native functions into Rex so scripts can orchestrate real work while staying in a concise, declarative style. This makes Rex a practical scripting layer for workflow-style systems where you want strong typing and explicit control at the host boundary.

Because Rex programs are pure and free of side effects in the language itself, the runtime can safely execute host-provided async functions in parallel when it is valid to do so. In practice, that means users can write straightforward functional code and still benefit from concurrency without directly managing threads, locks, or low-level async orchestration.

For Rust embedders, prefer the `rexlang` crate as the stable public API surface. The
`rexlang-core` crate contains the underlying implementation and is re-exported by `rexlang`.

All Rex code samples in this documentation are interactive. Edit them, run them, and use the output to learn by experimentation. A good place to start is the sample below.

If you are using Rex as a code-generation target, read **[LLMs](LLMS.md)** early. It covers the
LLM-first semantic workflow, syntax pitfalls, and validation steps that reduce iteration time.

Try editing and running this intro data-processing demo:

```rex,interactive
let
  values = [3, 12, 7, 20, 15, 4],
  selected = filter (\n -> n >= 10) values,
  adjusted = map (\n -> n - 2) selected,
  total = foldl (\acc n -> acc + n) 0 adjusted
in
  (values, selected, adjusted, total)
```

This documentation is organized into several sections:

- **[Tutorial](tutorial/)** — A guided walk-through of writing Rex code
- **[Demos](demos/)** — Interactive algorithm demos
- **[Language Reference](LANGUAGE.md)** — Compact reference for Rex syntax and features
- **[Specification](SPEC.md)** — Locked semantics and edge cases
- **[Architecture](ARCHITECTURE.md)** — System architecture and design
- **[Memory Management](MEMORY_MANAGEMENT.md)** — Memory management implementation
- **[Embedding](EMBEDDING.md)** — Embedding Rex in other applications
- **[Contributing](CONTRIBUTING.md)** — How to contribute to Rex
- **[LLMs](LLMS.md)** — LLM-first semantic workflow and generation guidance

## Rex as a target for LLMs

Rex is the world’s first parallel functional language explicitly designed to be a useful target for LLMs. Its strong static type system gives rapid, high-signal feedback on generated programs, so both users and models can quickly identify mismatches and converge on correct code.

That typechecking loop works especially well with Rex’s functional, expression-oriented style. Because programs are written as pure data transformations, LLM-generated code tends to be easier to inspect, reason about, and refine than imperative scripts with hidden state or side effects.

Together, these properties make Rex a strong fit for LLM-generated data analysis pipelines and scientific workflows. Models can generate high-level orchestration in Rex, while host-provided Rust functions handle domain-specific execution, giving a clean split between deterministic workflow logic and host capabilities.
