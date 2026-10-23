//! Slice-2 conformance: the resolved WIT surface must match `wit/conformance.json`.
//!
//! The fixture is the single source of truth for the expected surface; this file
//! contains no second copy of the contract, only the rules that compare it with
//! what `wit-parser` actually resolves.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use wit_parser::{Handle, PackageId, Resolve, Type, TypeDefKind};

fn wit_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit")
}

fn fixture() -> Value {
    let path = wit_dir().join("conformance.json");
    let raw =
        fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|err| panic!("parse {}: {err}", path.display()))
}

struct ExpectedPackage {
    id: String,
    interfaces: Vec<String>,
    functions: BTreeMap<String, ExpectedFunction>,
    required_records: Vec<String>,
    forbidden_symbols: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct ExpectedFunction {
    params: BTreeMap<String, String>,
    ok: Option<String>,
    err: Option<String>,
}

fn expected_function(id: &str, name: &str, value: &Value) -> ExpectedFunction {
    let params = value
        .get("params")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("{id}: function {name:?} field \"params\" must be an object"))
        .iter()
        .map(|(param, ty)| {
            let ty = ty.as_str().unwrap_or_else(|| {
                panic!("{id}: function {name:?} parameter {param:?} type must be a string")
            });
            assert!(
                !param.is_empty() && !ty.is_empty(),
                "{id}: function {name:?} parameter names/types must be non-empty"
            );
            (param.clone(), ty.to_owned())
        })
        .collect();
    let optional_type = |key: &str| match value.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::String(ty)) => {
            assert!(
                !ty.is_empty(),
                "{id}: function {name:?} {key:?} type must be non-empty"
            );
            Some(ty.clone())
        }
        Some(_) => panic!("{id}: function {name:?} {key:?} must be a string or null"),
    };
    ExpectedFunction {
        params,
        ok: optional_type("ok"),
        err: optional_type("err"),
    }
}

fn string_array(value: &Value, key: &str, package: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{package}: fixture field {key:?} must be an array"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("{package}: {key:?} entries must be strings"))
                .to_owned()
        })
        .collect()
}

fn expected_packages() -> Vec<ExpectedPackage> {
    let fixture = fixture();
    let packages = fixture
        .get("packages")
        .and_then(Value::as_array)
        .expect("conformance.json: packages must be an array");
    packages
        .iter()
        .map(|package| {
            let id = package
                .get("id")
                .and_then(Value::as_str)
                .expect("conformance.json: package id must be a string")
                .to_owned();
            let functions = package
                .get("functions")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("{id}: fixture field \"functions\" must be an object"));
            let functions = functions
                .iter()
                .map(|(name, expected)| (name.clone(), expected_function(&id, name, expected)))
                .collect();
            ExpectedPackage {
                interfaces: string_array(package, "interfaces", &id),
                functions,
                required_records: string_array(package, "required-records", &id),
                forbidden_symbols: string_array(package, "forbidden-symbols", &id),
                id,
            }
        })
        .collect()
}

/// Resolve every `.wit` file in `wit/` and map its package id to the resolved id.
fn resolved_surface() -> (Resolve, BTreeMap<String, PackageId>) {
    let dir = wit_dir();
    let mut sources: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("read dir {}: {err}", dir.display()))
        .map(|entry| entry.expect("read wit dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wit"))
        .collect();
    sources.sort();
    assert!(
        !sources.is_empty(),
        "{} contained no .wit files",
        dir.display()
    );

    let mut resolve = Resolve::default();
    let mut by_id = BTreeMap::new();
    for source in &sources {
        let package = resolve
            .push_file(source)
            .unwrap_or_else(|err| panic!("parse {}: {err}", source.display()));
        let id = resolve.packages[package].name.to_string();
        assert!(
            by_id.insert(id.clone(), package).is_none(),
            "duplicate resolved package id {id}"
        );
    }
    (resolve, by_id)
}

/// A syntax regression must fail here, not at slice-2 implementation time.
#[test]
fn wit_files_parse() {
    let (resolve, by_id) = resolved_surface();
    let expected: BTreeSet<String> = expected_packages().into_iter().map(|p| p.id).collect();
    assert_eq!(
        by_id.keys().cloned().collect::<BTreeSet<_>>(),
        expected,
        "resolved WIT packages (got {:?}) must be exactly the fixture packages",
        by_id.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        by_id.len(),
        2,
        "fixture must not name packages outside the two"
    );
    for package in by_id.values() {
        assert!(
            !resolve.packages[*package].interfaces.is_empty(),
            "package {} resolved with no interfaces",
            resolve.packages[*package].name
        );
    }
}

#[test]
fn fixture_matches_resolved_surface() {
    let (resolve, by_id) = resolved_surface();
    let expected = expected_packages();

    let expected_ids: BTreeSet<&str> = expected.iter().map(|p| p.id.as_str()).collect();
    let resolved_ids: BTreeSet<String> = by_id.keys().cloned().collect();
    assert_eq!(
        resolved_ids,
        expected_ids.iter().map(|id| (*id).to_owned()).collect(),
        "fixture packages must equal resolved packages"
    );

    for package in &expected {
        let package_id = *by_id
            .get(&package.id)
            .unwrap_or_else(|| panic!("{}: fixture package did not resolve", package.id));
        let resolved = &resolve.packages[package_id];
        assert_eq!(
            resolved.name.to_string(),
            package.id,
            "resolved package id drifted from fixture"
        );

        let resolved_interfaces: BTreeSet<String> = resolved.interfaces.keys().cloned().collect();
        let fixture_interfaces: BTreeSet<String> = package.interfaces.iter().cloned().collect();
        assert_eq!(
            resolved_interfaces, fixture_interfaces,
            "{}: interfaces must match the fixture exactly",
            package.id
        );

        for interface_name in &package.interfaces {
            let interface_id = *resolved.interfaces.get(interface_name).unwrap_or_else(|| {
                panic!(
                    "{}: fixture interface {interface_name:?} is missing from WIT",
                    package.id
                )
            });
            let interface = &resolve.interfaces[interface_id];

            for (name, expected_function) in &package.functions {
                let function = interface.functions.get(name).unwrap_or_else(|| {
                    panic!(
                        "{}::{interface_name}: fixture function {name:?} is missing from WIT",
                        package.id
                    )
                });

                let actual_params: BTreeMap<String, String> = function
                    .params
                    .iter()
                    .map(|(param, ty)| (param.clone(), type_display(&resolve, ty)))
                    .collect();
                assert_eq!(
                    actual_params, expected_function.params,
                    "{}::{interface_name}::{name}: parameter names/types must match the fixture exactly",
                    package.id
                );

                let result_types: Vec<&Type> = function.results.iter_types().collect();
                assert_eq!(
                    result_types.len(),
                    1,
                    "{}::{interface_name}::{name}: expected exactly one result type",
                    package.id
                );
                let (ok, err) = match result_types[0] {
                    Type::Id(id) => match &resolve.types[*id].kind {
                        TypeDefKind::Result(result) => (result.ok.as_ref(), result.err.as_ref()),
                        _ => (Some(result_types[0]), None),
                    },
                    ty => (Some(ty), None),
                };
                assert_eq!(
                    ok.map(|ty| type_display(&resolve, ty)),
                    expected_function.ok,
                    "{}::{interface_name}::{name}: result ok type must match the fixture exactly",
                    package.id
                );
                assert_eq!(
                    err.map(|ty| type_display(&resolve, ty)),
                    expected_function.err,
                    "{}::{interface_name}::{name}: result error type must match the fixture exactly",
                    package.id
                );
            }

            let resolved_functions: BTreeSet<String> =
                interface.functions.keys().cloned().collect();
            let fixture_functions: BTreeSet<String> = package.functions.keys().cloned().collect();
            assert_eq!(
                resolved_functions, fixture_functions,
                "{}::{interface_name}: exported functions must match the fixture exactly",
                package.id
            );

            for record in &package.required_records {
                let type_id = *interface.types.get(record).unwrap_or_else(|| {
                    panic!(
                        "{}::{interface_name}: required record {record:?} is missing",
                        package.id
                    )
                });
                assert!(
                    matches!(resolve.types[type_id].kind, TypeDefKind::Record(_)),
                    "{}::{interface_name}: {record:?} must be a record, found {}",
                    package.id,
                    resolve.types[type_id].kind.as_str()
                );
            }

            for (name, function) in &interface.functions {
                let mut type_names = Vec::new();
                for (_, ty) in &function.params {
                    collect_type_names(&resolve, ty, &mut type_names);
                }
                for ty in function.results.iter_types() {
                    collect_type_names(&resolve, ty, &mut type_names);
                }
                for type_name in &type_names {
                    let lower = type_name.to_lowercase();
                    for forbidden in &package.forbidden_symbols {
                        assert!(
                            !lower.contains(&forbidden.to_lowercase()),
                            "{}::{interface_name}::{name}: type {type_name:?} contains forbidden symbol {forbidden:?}",
                            package.id
                        );
                    }
                }
            }
        }
    }
}

/// Prints the surface the test actually saw; run with `--nocapture`.
#[test]
fn resolved_surface_summary() {
    let (resolve, by_id) = resolved_surface();
    for (id, package_id) in &by_id {
        let package = &resolve.packages[*package_id];
        println!("package {id}");
        for (interface_name, interface_id) in &package.interfaces {
            let interface = &resolve.interfaces[*interface_id];
            println!("  interface {interface_name}");
            let functions: Vec<&str> = interface.functions.keys().map(String::as_str).collect();
            println!("    functions: {}", functions.join(", "));
            let records: Vec<&str> = interface
                .types
                .iter()
                .filter(|(_, id)| matches!(resolve.types[**id].kind, TypeDefKind::Record(_)))
                .map(|(name, _)| name.as_str())
                .collect();
            println!("    records: {}", records.join(", "));
        }
    }
}

fn type_display(resolve: &Resolve, ty: &Type) -> String {
    match ty {
        Type::Bool => "bool",
        Type::U8 => "u8",
        Type::U16 => "u16",
        Type::U32 => "u32",
        Type::U64 => "u64",
        Type::S8 => "s8",
        Type::S16 => "s16",
        Type::S32 => "s32",
        Type::S64 => "s64",
        Type::F32 => "f32",
        Type::F64 => "f64",
        Type::Char => "char",
        Type::String => "string",
        Type::Id(id) => {
            let def = &resolve.types[*id];
            match &def.name {
                Some(name) => return name.clone(),
                None => panic!(
                    "cannot display unnamed {} type; name it in the WIT or decompose it in the fixture",
                    def.kind.as_str()
                ),
            }
        }
    }
    .to_owned()
}

fn collect_type_names(resolve: &Resolve, ty: &Type, out: &mut Vec<String>) {
    if let Type::Id(id) = ty {
        collect_type_id_names(resolve, *id, out);
    }
}

fn collect_type_id_names(resolve: &Resolve, id: wit_parser::TypeId, out: &mut Vec<String>) {
    let def = &resolve.types[id];
    if let Some(name) = &def.name {
        out.push(name.clone());
    }
    match &def.kind {
        TypeDefKind::Record(record) => {
            for field in &record.fields {
                collect_type_names(resolve, &field.ty, out);
            }
        }
        TypeDefKind::Tuple(tuple) => {
            for ty in &tuple.types {
                collect_type_names(resolve, ty, out);
            }
        }
        TypeDefKind::Variant(variant) => {
            for case in &variant.cases {
                if let Some(ty) = &case.ty {
                    collect_type_names(resolve, ty, out);
                }
            }
        }
        TypeDefKind::Option(ty) | TypeDefKind::List(ty) | TypeDefKind::Type(ty) => {
            collect_type_names(resolve, ty, out);
        }
        TypeDefKind::Result(result) => {
            for ty in [result.ok.as_ref(), result.err.as_ref()]
                .into_iter()
                .flatten()
            {
                collect_type_names(resolve, ty, out);
            }
        }
        TypeDefKind::Handle(Handle::Own(id) | Handle::Borrow(id)) => {
            collect_type_id_names(resolve, *id, out);
        }
        TypeDefKind::Future(ty) => {
            if let Some(ty) = ty {
                collect_type_names(resolve, ty, out);
            }
        }
        TypeDefKind::Stream(stream) => {
            for ty in [stream.element.as_ref(), stream.end.as_ref()]
                .into_iter()
                .flatten()
            {
                collect_type_names(resolve, ty, out);
            }
        }
        TypeDefKind::Resource | TypeDefKind::Flags(_) | TypeDefKind::Enum(_) => {}
        TypeDefKind::Unknown => unreachable!("resolved types cannot be unknown"),
    }
}
