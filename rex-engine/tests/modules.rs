use std::fs;
use std::path::{Path, PathBuf};

use rex_engine::{Engine, Pointer, Value};
use rex_util::{GasCosts, GasMeter};
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

fn engine_with_prelude() -> Engine {
    Engine::with_prelude().unwrap()
}

fn unlimited_gas() -> GasMeter {
    GasMeter::unlimited(GasCosts::sensible_defaults())
}

async fn eval_module_file(
    engine: &mut Engine,
    path: &Path,
) -> Result<Pointer, rex_engine::EngineError> {
    let mut gas = unlimited_gas();
    engine.eval_module_file_with_gas(path, &mut gas).await
}

async fn eval_snippet(
    engine: &mut Engine,
    source: &str,
) -> Result<Pointer, rex_engine::EngineError> {
    let mut gas = unlimited_gas();
    engine.eval_snippet_with_gas(source, &mut gas).await
}

async fn eval_snippet_at(
    engine: &mut Engine,
    source: &str,
    importer_path: impl AsRef<Path>,
) -> Result<Pointer, rex_engine::EngineError> {
    let mut gas = unlimited_gas();
    engine
        .eval_snippet_at_with_gas(source, importer_path, &mut gas)
        .await
}

macro_rules! pvals {
    ($engine:expr, $vals:expr) => {
        $vals
            .iter()
            .map(|pointer| {
                (
                    pointer.clone(),
                    $engine
                        .heap()
                        .get(pointer)
                        .map(|value| value.as_ref().clone())
                        .unwrap(),
                )
            })
            .collect::<Vec<(Pointer, Value)>>()
    };
}

#[tokio::test]
async fn module_import_local_pub() {
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

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let value_ptr = eval_module_file(&mut engine, &main).await.unwrap();
    let value = engine
        .heap()
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();
    match value {
        Value::I32(v) => assert_eq!(v, 3),
        _ => panic!(
            "expected i32, got {}",
            engine.heap().type_name(&value_ptr).unwrap()
        ),
    }
}

#[tokio::test]
async fn module_import_rejects_private_access() {
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

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_module_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
}

#[tokio::test]
async fn module_import_include_roots() {
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

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    engine.add_include_resolver(&include_root).unwrap();
    let value_ptr = eval_module_file(&mut engine, &main).await.unwrap();
    let value = engine
        .heap()
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();
    match value {
        Value::I32(v) => assert_eq!(v, 42),
        _ => panic!(
            "expected i32, got {}",
            engine.heap().type_name(&value_ptr).unwrap()
        ),
    }
}

#[tokio::test]
async fn snippet_can_import_with_explicit_base() {
    let dir = temp_dir("snippet_can_import_with_explicit_base");
    let module = dir.join("foo").join("bar.rex");
    write_file(
        &module,
        r#"
pub fn add x: i32 -> y: i32 -> i32 = x + y
()
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let value_ptr = eval_snippet_at(
        &mut engine,
        r#"
import foo.bar as Bar
Bar.add 20 22
"#,
        dir.join("_snippet.rex"),
    )
    .await
    .unwrap();
    let value = engine
        .heap()
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    match value {
        Value::I32(v) => assert_eq!(v, 42),
        _ => panic!(
            "expected i32, got {}",
            engine.heap().type_name(&value_ptr).unwrap()
        ),
    }
}

#[tokio::test]
async fn std_json_encode_decode_smoke() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let value_ptr = eval_snippet(
        &mut engine,
        r#"
import std.json as Json

let
  b_ok =
    match (Json.from_json (Json.to_json true))
      when Ok b -> if b then 1 else 0
      when Err _ -> -1,

  n_ok =
    match (Json.from_json (Json.to_json 123))
      when Ok n -> if n == 123 then 1 else 0
      when Err _ -> -1,

  opt_val =
    match (Json.from_json (Json.to_json (Some 7)))
      when Ok opt ->
        (
          match opt
            when Some x -> x
            when None -> -1
        )
      when Err _ -> -2,

  list_head =
    match (Json.from_json (Json.to_json [1, 2, 3]))
      when Ok xs ->
        (
          match xs
            when [] -> -1
            when x:rest -> x
        )
      when Err _ -> -2,

  arr0 =
    let a = prim_array_from_list [4, 5] in
    match (Json.from_json (Json.to_json a))
      when Ok xs ->
        let ys: Array i32 = xs in get 0 ys
      when Err _ -> -1,

  dict_sum =
    let d = ({a = 1, b = 2}) is Dict i32 in
    match (Json.from_json (Json.to_json d))
      when Ok d2 ->
        (
          match d2
            when {a, b} -> a + b
            when {} -> -1
        )
      when Err _ -> -2,

  res_ok =
    match (Json.from_json (Json.to_json (Ok 3)))
      when Ok r ->
        (
          match r
            when Ok x -> x
            when Err _ -> -1
        )
      when Err _ -> -2,

  res_err_ok =
    match (Json.from_json (Json.to_json (Err "no")))
      when Ok r ->
        (
          match r
            when Err s -> if s == "no" then 1 else 0
            when Ok _ -> 0
        )
      when Err _ -> -1
in
  (b_ok, n_ok, opt_val, list_head, arr0, dict_sum, res_ok, res_err_ok)
"#,
    )
    .await
    .unwrap();
    let value = engine
        .heap()
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(xs) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap().type_name(&value_ptr).unwrap()
        );
    };
    let xs = pvals!(engine, xs);
    let got: Vec<i32> = xs
        .into_iter()
        .map(|(pointer, v)| match v {
            Value::I32(n) => n,
            _ => panic!(
                "expected i32, got {}",
                engine.heap().type_name(&pointer).unwrap()
            ),
        })
        .collect();
    assert_eq!(got, vec![1, 1, 7, 1, 4, 3, 3, 1]);
}

#[tokio::test]
async fn std_json_roundtrip_nested() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let value_ptr = eval_snippet(
        &mut engine,
        r#"
import std.json as Json

let
  xs =
    [ Some (Ok 1)
    , None
    , Some (Err "no")
    , Some (Ok 42)
    ],

  xs_ok =
    match (Json.from_json (Json.to_json xs))
      when Ok ys -> if ys == xs then 1 else 0
      when Err _ -> -1,

  arr =
    prim_array_from_list [Ok 1, Err "bad", Ok 3],

  arr_ok =
    match (Json.from_json (Json.to_json arr))
      when Ok ys -> if ys == arr then 1 else 0
      when Err _ -> -1
in
  (xs_ok, arr_ok)
"#,
    )
    .await
    .unwrap();
    let value = engine
        .heap()
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(xs) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap().type_name(&value_ptr).unwrap()
        );
    };
    let xs = pvals!(engine, xs);
    let got: Vec<i32> = xs
        .into_iter()
        .map(|(pointer, v)| match v {
            Value::I32(n) => n,
            _ => panic!(
                "expected i32, got {}",
                engine.heap().type_name(&pointer).unwrap()
            ),
        })
        .collect();
    assert_eq!(got, vec![1, 1]);
}

#[tokio::test]
async fn std_json_decode_errors_have_useful_messages() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let value_ptr = eval_snippet(
        &mut engine,
        r#"
import std.json as Json

let
  both =
    let v = Json.Object { ok = Json.Number (prim_to_f64 1), err = Json.String "bad" } in
    match (Json.from_json v)
      when Ok r -> let _r: Result i32 string = r in "unexpected ok"
      when Err e -> e.message,

  neither =
    let v = Json.Object {} in
    match (Json.from_json v)
      when Ok r -> let _r: Result i32 string = r in "unexpected ok"
      when Err e -> e.message,

  wrong_kind =
    let v = Json.Bool true in
    match (Json.from_json v)
      when Ok xs -> let _xs: List i32 = xs in "unexpected ok"
      when Err e -> e.message,

  bad_list_elem =
    let v =
      Json.Array (prim_array_from_list [Json.Number (prim_to_f64 1), Json.String "oops"])
    in
    match (Json.from_json v)
      when Ok xs -> let _xs: List i32 = xs in "unexpected ok"
      when Err e -> e.message
in
  (both, neither, wrong_kind, bad_list_elem)
"#,
    )
    .await
    .unwrap();
    let value = engine
        .heap()
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(parts) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap().type_name(&value_ptr).unwrap()
        );
    };
    let parts = pvals!(engine, parts);
    let got: Vec<String> = parts
        .into_iter()
        .map(|(pointer, v)| match v {
            Value::String(s) => s,
            _ => panic!(
                "expected string, got {}",
                engine.heap().type_name(&pointer).unwrap()
            ),
        })
        .collect();

    assert!(got[0].contains("exactly one"), "{}", got[0]);
    assert!(got[1].contains("{ok} or {err}"), "{}", got[1]);
    assert!(got[2].contains("expected array, got bool"), "{}", got[2]);
    assert!(got[3].contains("expected number, got string"), "{}", got[3]);
}

#[tokio::test]
async fn std_json_numeric_decode_errors() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let value_ptr = eval_snippet(
        &mut engine,
        r#"
import std.json as Json

let
  u8_overflow =
    match (Json.from_json (Json.Number (prim_to_f64 256)))
      when Ok n -> let _n: u8 = n in "unexpected ok"
      when Err e -> e.message,

  i32_fractional =
    match (Json.from_json (Json.Number (prim_to_f64 1.5)))
      when Ok n -> let _n: i32 = n in "unexpected ok"
      when Err e -> e.message
in
  (u8_overflow, i32_fractional)
"#,
    )
    .await
    .unwrap();
    let value = engine
        .heap()
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(parts) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap().type_name(&value_ptr).unwrap()
        );
    };
    let parts = pvals!(engine, parts);
    let got: Vec<String> = parts
        .into_iter()
        .map(|(pointer, v)| match v {
            Value::String(s) => s,
            _ => panic!(
                "expected string, got {}",
                engine.heap().type_name(&pointer).unwrap()
            ),
        })
        .collect();

    assert!(got[0].contains("representable as u8"), "{}", got[0]);
    assert!(got[1].contains("representable as i32"), "{}", got[1]);
}

#[tokio::test]
async fn std_json_pretty_renders_valid_json() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let value_ptr = eval_snippet(
        &mut engine,
        r#"
import std.json as Json

let
  v =
    Json.Object {
      a = Json.Number (prim_to_f64 1),
      b = Json.String "a\"b\\c\n",
      c =
        Json.Array (prim_array_from_list [
          Json.Null,
          Json.Bool true,
          Json.Number (prim_to_f64 (0.0 / 0.0))
        ])
    }
in
  pretty v
"#,
    )
    .await
    .unwrap();
    let value = engine
        .heap()
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::String(rendered) = value else {
        panic!(
            "expected string, got {}",
            engine.heap().type_name(&value_ptr).unwrap()
        );
    };

    let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    let obj = parsed.as_object().expect("expected object");

    assert_eq!(obj.get("a").and_then(|v| v.as_f64()), Some(1.0));
    assert_eq!(
        obj.get("b").and_then(|v| v.as_str()),
        Some("a\\\"b\\\\c\\n")
    );
    let arr = obj
        .get("c")
        .and_then(|v| v.as_array())
        .expect("expected array");
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0], serde_json::Value::Null);
    assert_eq!(arr[1], serde_json::Value::Bool(true));
    assert_eq!(arr[2], serde_json::Value::Null);
}

#[tokio::test]
async fn std_json_parse_and_from_string_roundtrip() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let value_ptr = eval_snippet(
        &mut engine,
        r#"
import std.json as Json

let
  v =
    Json.Object {
      a = Json.Number (prim_to_f64 1),
      b = Json.String "a\"b\\c\n",
      c = Json.Array (prim_array_from_list [Json.Null, Json.Bool true])
    },

  parsed_ok =
    match (Json.parse (pretty v))
      when Ok v2 -> if pretty v2 == pretty v then 1 else 0
      when Err _ -> -1,

  xs = [1, 2, 3],
  s = Json.stringify (Json.to_json xs),
  decoded_ok =
    match (Json.parse s)
      when Err _ -> -1
      when Ok v0 ->
        (
          match (Json.from_json v0)
            when Ok ys -> if ys == xs then 1 else 0
            when Err _ -> -2
        ),

  bad_s = "{",
  parse_err =
    match (Json.parse bad_s)
      when Ok _ -> 0
      when Err e -> if e.message != "" then 1 else 0
in
  (parsed_ok, decoded_ok, parse_err)
"#,
    )
    .await
    .unwrap();
    let value = engine
        .heap()
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(xs) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap().type_name(&value_ptr).unwrap()
        );
    };
    let xs = pvals!(engine, xs);
    let got: Vec<i32> = xs
        .into_iter()
        .map(|(pointer, v)| match v {
            Value::I32(n) => n,
            _ => panic!(
                "expected i32, got {}",
                engine.heap().type_name(&pointer).unwrap()
            ),
        })
        .collect();
    assert_eq!(got, vec![1, 1, 1]);
}
