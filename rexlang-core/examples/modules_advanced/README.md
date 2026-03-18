# modules_advanced

This example demonstrates Rex module imports with nested module dependencies:

- wildcard imports: `import foo.bar (*)`
- selective imports: `import foo.bar (x, y)`
- selective imports with rename: `import foo.bar (x, y as z)`
- module alias imports: `import foo.bar as Bar`
- modules importing other modules (including `super...` paths)

Run it:

```sh
cargo run -p rexlang-cli -- run rexlang-core/examples/modules_advanced/main.rex
```
