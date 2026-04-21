# libraries_advanced

This example demonstrates Rex library imports with nested library dependencies:

- wildcard imports: `import foo.bar (*)`
- selective imports: `import foo.bar (x, y)`
- selective imports with rename: `import foo.bar (x, y as z)`
- library alias imports: `import foo.bar as Bar`
- libraries importing other libraries (including `super...` paths)

Run it:

```sh
cargo run -p rex -- run rex/examples/libraries_advanced/main.rex
```
