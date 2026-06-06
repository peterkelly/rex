# modules_advanced

This example demonstrates Rex module imports with nested module dependencies under the CLI
filesystem importer:

- wildcard imports: `import foo.bar (*)`
- selective imports: `import foo.bar (x, y)`
- selective imports with rename: `import foo.bar (x, y as z)`
- module alias imports: `import foo.bar as Bar`
- modules importing other modules through the CLI import-path policy, including parent-style
  `super...` paths

Run it:

```sh
cargo run -p rex-cli --bin rex -- rex-cli/examples/modules_advanced/main.rex
```
