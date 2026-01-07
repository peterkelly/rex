use std::fs;
use std::path::{Path, PathBuf};

use rex_engine::{Engine, Value};
use uuid::Uuid;

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("rex-engine-test-{name}-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[test]
fn module_import_local_pub() {
    let dir = temp_dir("module_import_local_pub");
    let main = dir.join("main.rex");
    let module = dir.join("foo").join("bar.rex");

    write_file(
        &module,
        r#"
pub fn add x: i32 -> y: i32 -> i32 = x + y
fn hidden x: i32 -> i32 = x + 1
()
"#,
    );
    write_file(
        &main,
        r#"
import foo.bar as Bar
Bar.add 1 2
"#,
    );

    let mut engine = Engine::with_prelude();
    engine.add_default_resolvers();
    let value = engine.eval_module_file(&main).unwrap();
    match value {
        Value::I32(v) => assert_eq!(v, 3),
        other => panic!("expected i32, got {other}"),
    }
}

#[test]
fn module_import_rejects_private_access() {
    let dir = temp_dir("module_import_rejects_private_access");
    let main = dir.join("main.rex");
    let module = dir.join("foo").join("bar.rex");

    write_file(
        &module,
        r#"
pub fn add x: i32 -> y: i32 -> i32 = x + y
fn hidden x: i32 -> i32 = x + 1
()
"#,
    );
    write_file(
        &main,
        r#"
import foo.bar as Bar
Bar.hidden 1
"#,
    );

    let mut engine = Engine::with_prelude();
    engine.add_default_resolvers();
    let err = match engine.eval_module_file(&main) {
        Ok(v) => panic!("expected error, got {v}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
}

#[test]
fn module_import_include_roots() {
    let dir = temp_dir("module_import_include_roots");
    let include_root = dir.join("includes");
    let main_root = dir.join("src");
    let main = main_root.join("main.rex");

    let module = include_root.join("lib").join("math.rex");
    write_file(
        &module,
        r#"
pub fn inc x: i32 -> i32 = x + 1
()
"#,
    );

    write_file(
        &main,
        r#"
import lib.math as Math
Math.inc 41
"#,
    );

    let mut engine = Engine::with_prelude();
    engine.add_default_resolvers();
    engine.add_include_resolver(&include_root).unwrap();
    let value = engine.eval_module_file(&main).unwrap();
    match value {
        Value::I32(v) => assert_eq!(v, 42),
        other => panic!("expected i32, got {other}"),
    }
}

#[test]
fn snippet_can_import_with_explicit_base() {
    let dir = temp_dir("snippet_can_import_with_explicit_base");
    let module = dir.join("foo").join("bar.rex");
    write_file(
        &module,
        r#"
pub fn add x: i32 -> y: i32 -> i32 = x + y
()
"#,
    );

    let mut engine = Engine::with_prelude();
    engine.add_default_resolvers();
    let value = engine
        .eval_snippet_at(
            r#"
import foo.bar as Bar
Bar.add 20 22
"#,
            dir.join("_snippet.rex"),
        )
        .unwrap();

    match value {
        Value::I32(v) => assert_eq!(v, 42),
        other => panic!("expected i32, got {other}"),
    }
}
