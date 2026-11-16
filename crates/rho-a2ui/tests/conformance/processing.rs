//! The suites that run messages through a `MessageProcessor`: processing,
//! validation, surface state, catalog selection, functions on a surface and
//! actions.

use std::collections::{BTreeMap, VecDeque};
use std::time::SystemTime;

use rho_a2ui::{
    Action, Catalog, CatalogError, Derivation, Entry, ErrorCode, ErrorMessage, ErrorTarget, Expr,
    ExpressionError, Functions, InlineCatalogs, Interaction, MessageProcessor, Metadata,
    RendererMessage, Resolved, ResolvedComponent, Scope, UserActivation, Validation,
    ValidationConfig,
};
use serde_json::{json, Value};

use super::{
    assert_no_failures, case_name, check_function_result, check_keys, expected_error, is_skipped,
    load_suite, read_vendored, same,
};

/// Cases each suite leaves out because they target a protocol version before
/// 1.0, by [`case_version`].
const FILTERED_MESSAGE_PROCESSOR: usize = 1;
const FILTERED_ERROR_RESPONSES: usize = 1;
const FILTERED_ACTIONS: usize = 7;

/// The protocol version a case targets: its own `protocolVersion`, else its
/// catalogs', else that of the suite file it is in when the file's name
/// states one, else 0.9, the spec's default for a catalog that declares none.
pub(super) fn case_version(case: &Value, file: &str) -> String {
    let catalog = case.get("catalog");
    let declared = [
        case.get("protocolVersion"),
        catalog.and_then(|catalog| catalog.get("protocolVersion")),
        catalog.and_then(|catalog| catalog.pointer("/catalogSchema/protocolVersion")),
        case["catalogs"].as_array().and_then(|catalogs| {
            catalogs
                .iter()
                .find_map(|catalog| catalog.get("protocolVersion"))
        }),
    ]
    .into_iter()
    .flatten()
    .next()
    .cloned()
    .or_else(|| {
        let first = case["catalogPaths"].as_array()?.first()?.as_str()?;
        read_vendored(first).get("protocolVersion").cloned()
    });
    match declared {
        Some(Value::String(version)) => version,
        Some(other) => panic!(
            "{}: protocolVersion {other} is not a string",
            case_name(case)
        ),
        None if file.ends_with("_v1_0.yaml") => "1.0".to_owned(),
        None => "0.9".to_owned(),
    }
}

/// Runs every v1.0 case of `file` by its action, and asserts how many cases
/// it left out as pre-1.0.
fn run_suite(file: &str, filtered: usize) {
    let mut failures = Vec::new();
    let mut left_out = 0;
    for case in load_suite(file) {
        let name = case_name(&case);
        match case_version(&case, file).as_str() {
            "1.0" => {}
            "0.9" | "v0.8" | "v0.9" | "v0.9.1" => {
                left_out += 1;
                continue;
            }
            other => panic!("{name}: unknown protocol version {other:?}"),
        }
        if is_skipped(name) {
            continue;
        }
        if let Err(reason) = run_case(&case) {
            failures.push(format!("{name}: {reason}"));
        }
    }
    assert_no_failures(file, &failures);
    assert_eq!(left_out, filtered, "{file}: cases left out as pre-1.0");
}

/// Runs one v1.0 case by its action.
pub(super) fn run_case(case: &Value) -> Result<(), String> {
    match case["action"].as_str().ok_or("no action")? {
        "process_messages" => run_messages(case, Mode::Process),
        "validate" => run_messages(case, Mode::Validate),
        "get_renderer_capabilities" => run_capabilities(case),
        "get_renderer_data_model" => run_data_model(case),
        "evaluate_function" => run_function_on_surface(case),
        "dispatch_action" => run_dispatch(case),
        "select_catalog" => run_select_catalog(case),
        other => Err(format!("unknown action {other:?}")),
    }
}

#[test]
fn message_processor_suite() {
    run_suite("message_processor_v1_0.yaml", FILTERED_MESSAGE_PROCESSOR);
}

#[test]
fn validator_suite() {
    run_suite("validator_v1_0.yaml", 0);
}

#[test]
fn error_responses_suite() {
    run_suite("error_responses.yaml", FILTERED_ERROR_RESPONSES);
}

#[test]
fn data_deletion_suite() {
    run_suite("data_deletion.yaml", 0);
}

#[test]
fn data_model_pointers_suite() {
    run_suite("data_model_pointers.yaml", 0);
}

#[test]
fn reserved_keys_suite() {
    run_suite("reserved_keys.yaml", 0);
}

#[test]
fn composition_constraints_suite() {
    run_suite("composition_constraints.yaml", 0);
}

#[test]
fn index_function_suite() {
    run_suite("index_function.yaml", 0);
}

#[test]
fn validation_result_suite() {
    run_suite("validation_result.yaml", 0);
}

#[test]
fn actions_suite() {
    run_suite("actions.yaml", FILTERED_ACTIONS);
}

#[test]
fn multi_catalog_suite() {
    run_suite("multi_catalog.yaml", 0);
}

/// How a case runs its messages.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `process_messages`: no validation unless the case asks for it.
    Process,
    /// `validate`: strict unless the case says `strictMode: false`, and an
    /// error a surface reports while its components resolve fails the step,
    /// as the reference harnesses raise it.
    Validate,
}

fn run_messages(case: &Value, mode: Mode) -> Result<(), String> {
    check_keys(
        case,
        &[
            "name",
            "description",
            "features",
            "action",
            "protocolVersion",
            "catalog",
            "catalogPaths",
            "catalogs",
            "messages",
            "payload",
            "steps",
            "expect",
            "expectError",
            "expect_error",
            "strictMode",
            "validationConfig",
        ],
    )?;
    let validation = match (mode, case.get("validationConfig"), case.get("strictMode")) {
        (_, Some(flags), _) => Validation::Check(config_from(flags)?),
        (Mode::Validate, None, Some(Value::Bool(false))) => {
            Validation::Check(ValidationConfig::RELAXED)
        }
        (Mode::Validate, None, _) | (Mode::Process, None, Some(Value::Bool(true))) => {
            Validation::Check(ValidationConfig::STRICT)
        }
        (Mode::Process, None, _) => Validation::Skip,
    };
    let mut processor = case_processor(case, validation, Aliases::Add)?;
    let steps = match case["steps"].as_array() {
        Some(steps) => steps
            .iter()
            .enumerate()
            .map(|(n, step)| Step::of(step, (n + 1 == steps.len()).then_some(case)))
            .collect::<Result<Vec<_>, _>>()?,
        None => vec![Step {
            payload: messages(case)?,
            config: None,
            want_error: expected_error(case),
            expect: case.get("expect"),
        }],
    };
    for (n, step) in steps.iter().enumerate() {
        run_step(&mut processor, step, mode).map_err(|reason| format!("step {n}: {reason}"))?;
    }
    Ok(())
}

/// One call of a case: the messages it processes and what they must do.
struct Step<'a> {
    payload: &'a Value,
    /// `validationConfig` for this call only.
    config: Option<&'a Value>,
    want_error: Option<&'a Value>,
    expect: Option<&'a Value>,
}

impl<'a> Step<'a> {
    /// A step of `steps`. The last one, which `case` is given for, also
    /// takes the case's expectations.
    fn of(step: &'a Value, case: Option<&'a Value>) -> Result<Self, String> {
        check_keys(
            step,
            &[
                "messages",
                "payload",
                "expect",
                "expectError",
                "validationConfig",
            ],
        )?;
        Ok(Self {
            payload: messages(step)?,
            config: step.get("validationConfig"),
            want_error: expected_error(step).or_else(|| case.and_then(expected_error)),
            expect: step.get("expect").or_else(|| case?.get("expect")),
        })
    }
}

fn messages(step: &Value) -> Result<&Value, String> {
    step.get("messages")
        .or_else(|| step.get("payload"))
        .ok_or_else(|| "no messages".to_owned())
}

fn run_step(processor: &mut MessageProcessor, step: &Step<'_>, mode: Mode) -> Result<(), String> {
    let processed = match step.config {
        Some(flags) => processor.process_with(step.payload, Validation::Check(config_from(flags)?)),
        None => processor.process(step.payload),
    };
    let rejected = processed.errors().next().cloned();
    let Some(want) = step.want_error else {
        if let Some(error) = rejected {
            return Err(format!("rejected: {} at {:?}", error, error.path()));
        }
        return match step.expect {
            Some(expect) => check_surfaces(processor, expect),
            None => Ok(()),
        };
    };
    match (rejected, mode) {
        (Some(error), _) => check_error(want, &error, Source::Message),
        (None, Mode::Validate) => {
            let reported = processor
                .surfaces()
                .map(|surface| surface.id().to_owned())
                .collect::<Vec<_>>()
                .iter()
                .flat_map(|surface| resolve_all(processor, surface).into_values())
                .flat_map(|resolved| resolved.errors)
                .next();
            match reported {
                Some(error) => check_error(want, &error, Source::Resolution),
                None => Err(format!("accepted, want an error {want}")),
            }
        }
        (None, Mode::Process) => Err(format!("accepted, want an error {want}")),
    }
}

/// A case's `validationConfig`: the rules it names, every other rule strict.
fn config_from(flags: &Value) -> Result<ValidationConfig, String> {
    check_keys(
        flags,
        &[
            "allowOrphanComponents",
            "allowDanglingReferences",
            "allowMissingRoot",
            "allowUnknownElements",
        ],
    )?;
    let flag = |key: &str| flags[key] == true;
    Ok(ValidationConfig {
        allow_orphans: flag("allowOrphanComponents"),
        allow_dangling_references: flag("allowDanglingReferences"),
        allow_missing_root: flag("allowMissingRoot"),
        allow_unknown_elements: flag("allowUnknownElements"),
    })
}

/// A processor with the catalogs a case names.
///
/// `catalogPaths` name files; the published basic catalog is the crate's
/// own, with its functions. An inline `catalog` (or its `catalogSchema`) and
/// each `catalogs` entry is loaded, taking the case's version when it states
/// none; a stub with only an id is a catalog without entries. A catalog for a
/// version before 1.0 is left out, after checking that the crate refuses it:
/// a v1.0 processor cannot hold one.
///
/// With [`Aliases::Add`], as the reference harnesses do, `basic` and each
/// catalog id a `createSurface` names that the case defines nowhere are the
/// basic catalog under that id; the second only when the case expects no
/// `CatalogError`.
fn case_processor(
    case: &Value,
    validation: Validation,
    aliases: Aliases,
) -> Result<MessageProcessor, String> {
    let version = case_version(case, "");
    let mut documents: Vec<(Value, Functions)> = Vec::new();
    for path in case["catalogPaths"].as_array().into_iter().flatten() {
        let document = read_vendored(path.as_str().ok_or("catalogPaths holds a non-string")?);
        if document["catalogId"] == Catalog::basic().id() {
            documents.push((Catalog::basic().to_json(), Functions::basic()));
        } else {
            documents.push((document, Functions::new()));
        }
    }
    if let Some(catalog) = case.get("catalog") {
        let mut document = catalog.get("catalogSchema").unwrap_or(catalog).clone();
        if document.get("catalogId").is_some() {
            stamp_version(&mut document, &version);
            documents.push((document, Functions::new()));
        }
    }
    for entry in case["catalogs"].as_array().into_iter().flatten() {
        let mut document = entry.clone();
        stamp_version(&mut document, &version);
        documents.push((document, Functions::new()));
    }
    let mut processor = MessageProcessor::new(validation);
    let mut defined = Vec::new();
    for (document, functions) in documents {
        let id = document["catalogId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        defined.push(id.clone());
        match Catalog::from_json(document.clone()) {
            Ok(catalog) => processor
                .add_catalog(catalog, functions)
                .map_err(|e| format!("catalog {id}: {e}"))?,
            Err(CatalogError::UnsupportedVersion(_)) if document["protocolVersion"] != "1.0" => {}
            Err(e) => return Err(format!("catalog {id} does not load: {e}")),
        }
    }
    if aliases == Aliases::Omit {
        return Ok(processor);
    }
    let expects_catalog_error = std::iter::once(case)
        .chain(case["steps"].as_array().into_iter().flatten())
        .any(|step| expected_error(step).is_some_and(|want| want["category"] == "CatalogError"));
    let mut named = vec!["basic".to_owned()];
    if !expects_catalog_error {
        surface_catalog_ids(case, &mut named);
    }
    for id in named {
        if !defined.contains(&id) {
            defined.push(id.clone());
            processor
                .add_catalog(basic_as(&id), Functions::basic())
                .map_err(|e| format!("basic catalog as {id}: {e}"))?;
        }
    }
    Ok(processor)
}

/// Whether [`case_processor`] adds the catalogs the reference harnesses
/// supply beyond those a case defines.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Aliases {
    Add,
    Omit,
}

fn stamp_version(document: &mut Value, version: &str) {
    if let Value::Object(map) = document {
        map.entry("protocolVersion")
            .or_insert_with(|| Value::from(version));
    }
}

/// The basic catalog under another id.
fn basic_as(id: &str) -> Catalog {
    Catalog::basic()
        .derive(Derivation {
            catalog_id: id.to_owned(),
            components: None,
            functions: None,
            instructions: None,
        })
        .expect("the basic catalog derives under any id")
}

/// Every `catalogId` a `createSurface` below `node` names.
fn surface_catalog_ids(node: &Value, ids: &mut Vec<String>) {
    match node {
        Value::Object(map) => {
            if let Some(Value::String(id)) = map
                .get("createSurface")
                .and_then(|create| create.get("catalogId"))
            {
                ids.push(id.clone());
            }
            map.values()
                .for_each(|child| surface_catalog_ids(child, ids));
        }
        Value::Array(items) => items
            .iter()
            .for_each(|child| surface_catalog_ids(child, ids)),
        _ => {}
    }
}

/// Where an error came from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The processor rejected a message.
    Message,
    /// A surface reported it while its components resolved, in a `validate`
    /// step, where it counts as a validation failure.
    Resolution,
}

/// The error categories a code satisfies, following the blueprint's
/// exception hierarchy and the reference harnesses' table, in which an
/// `IntegrityError` expectation also accepts a recursion error.
fn categories(code: ErrorCode) -> &'static [&'static str] {
    match code {
        ErrorCode::ValidationFailed
        | ErrorCode::UnallowedParent
        | ErrorCode::UnallowedChild
        | ErrorCode::InvalidAction
        | ErrorCode::InvalidFunctionCall => &["ValidationError"],
        ErrorCode::IntegrityError => &["IntegrityError", "ValidationError"],
        ErrorCode::RecursionLimitExceeded => {
            &["RecursionError", "IntegrityError", "ValidationError"]
        }
        ErrorCode::CatalogNotFound | ErrorCode::CatalogError => &["CatalogError"],
        ErrorCode::DataError => &["DataError"],
        ErrorCode::ExpressionError | ErrorCode::ExecutionError => &["ExpressionError"],
    }
}

/// Checks an `error` message against a case's `expectError`: its category,
/// protocol codes, surface, function call and paths. Free-text messages are
/// not compared. A path matches when the error is at it or below it, as a
/// missing property is reported at the property.
fn check_error(want: &Value, got: &ErrorMessage, source: Source) -> Result<(), String> {
    let Value::Object(want) = want else {
        // A bare string names only the message.
        return Ok(());
    };
    check_keys(
        &Value::Object(want.clone()),
        &[
            "category",
            "code",
            "message",
            "path",
            "surfaceId",
            "functionCallId",
            "details",
            "rendererError",
        ],
    )?;
    let mismatch = |what: &str, wanted: &Value| {
        Err(format!(
            "{what} differs: got {got} at {:?}, want {wanted}",
            got.path()
        ))
    };
    if let Some(category) = want.get("category").and_then(Value::as_str) {
        let by_code = categories(got.code()).contains(&category);
        let in_validate = source == Source::Resolution && category == "ValidationError";
        if !(by_code || in_validate) {
            return mismatch("category", &want["category"]);
        }
    }
    if let Some(code) = want.get("code") {
        if code != got.code().as_str() {
            return mismatch("code", code);
        }
    }
    if let Some(surface) = want.get("surfaceId") {
        if got.surface_id() != surface.as_str() {
            return mismatch("surfaceId", surface);
        }
    }
    if let Some(call) = want.get("functionCallId") {
        if !matches!(got.target(), ErrorTarget::FunctionCall(id) if call == id.as_str()) {
            return mismatch("functionCallId", call);
        }
    }
    if let Some(path) = want.get("path") {
        if !path_matches(path, got.path()) {
            return mismatch("path", path);
        }
    }
    for detail in want
        .get("details")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        // Only details that name a protocol code describe the error message;
        // the others are one SDK's validation report.
        let Some(code) = detail["code"]
            .as_str()
            .filter(|code| code.chars().all(|c| c.is_ascii_uppercase() || c == '_'))
        else {
            continue;
        };
        if code != got.code().as_str() || !path_matches(&detail["path"], got.path()) {
            return mismatch("details", detail);
        }
    }
    if let Some(expected) = want.get("rendererError") {
        let wire = RendererMessage::Error(got.clone()).to_json();
        if let Some(version) = expected.get("version") {
            if wire["version"] != *version {
                return mismatch("rendererError.version", version);
            }
        }
        let body = expected.get("error").unwrap_or(expected);
        for key in ["code", "surfaceId", "functionCallId"] {
            if let Some(value) = body.get(key) {
                if wire["error"][key] != *value {
                    return mismatch(&format!("rendererError.{key}"), value);
                }
            }
        }
        if body.get("functionCallId").is_some()
            && body.get("surfaceId").is_none()
            && wire["error"].get("surfaceId").is_some()
        {
            return mismatch("rendererError.surfaceId", &Value::Null);
        }
        if let Some(path) = body.get("path") {
            if !path_matches(path, wire["error"]["path"].as_str()) {
                return mismatch("rendererError.path", path);
            }
        }
    }
    Ok(())
}

fn path_matches(want: &Value, got: Option<&str>) -> bool {
    match (want.as_str(), got) {
        (Some(want), Some(got)) => {
            got == want
                || got
                    .strip_prefix(want)
                    .is_some_and(|rest| rest.starts_with('/'))
        }
        (None, _) => want.is_null(),
        (Some(_), None) => false,
    }
}

/// Every component of `surface` resolved, keyed as the suites name them: by
/// id at the root, and `<id>_<index>` for an instance in a template item.
fn resolve_all(processor: &MessageProcessor, surface: &str) -> BTreeMap<String, ResolvedComponent> {
    let mut resolved = BTreeMap::new();
    let mut queue: VecDeque<(String, Scope)> = processor
        .surface(surface)
        .into_iter()
        .flat_map(rho_a2ui::Surface::components)
        .map(|component| (component.id().to_owned(), Scope::root()))
        .collect();
    while let Some((id, scope)) = queue.pop_front() {
        let key = match scope.index() {
            Some(index) => format!("{id}_{index}"),
            None => id.clone(),
        };
        if resolved.contains_key(&key) {
            continue;
        }
        let Some(component) = processor.resolve(surface, &id, &scope) else {
            continue;
        };
        for property in component.properties.values() {
            queue_items(property, &mut queue);
        }
        resolved.insert(key, component);
    }
    resolved
}

/// Queues the children of `property` that are template items or inside one.
fn queue_items(property: &Resolved, queue: &mut VecDeque<(String, Scope)>) {
    match property {
        Resolved::Child(child) => {
            if child.scope.index().is_some() {
                queue.push_back((child.id.clone(), child.scope.clone()));
            }
        }
        Resolved::Children(children) => {
            for child in children
                .iter()
                .filter(|child| child.scope.index().is_some())
            {
                queue.push_back((child.id.clone(), child.scope.clone()));
            }
        }
        Resolved::List(items) => items.iter().for_each(|item| queue_items(item, queue)),
        Resolved::Object(members) => members
            .values()
            .for_each(|member| queue_items(member, queue)),
        Resolved::Value(_) | Resolved::Action(_) => {}
    }
}

/// A resolved property as the suites write it: a value, or ids for children.
fn property_json(property: &Resolved) -> Value {
    match property {
        Resolved::Value(value) => value.clone(),
        Resolved::Child(child) => Value::from(child.id.as_str()),
        Resolved::Children(children) => children
            .iter()
            .map(|child| Value::from(child.id.as_str()))
            .collect(),
        Resolved::Action(_) => json!({ "action": "unevaluated" }),
        Resolved::List(items) => items.iter().map(property_json).collect(),
        Resolved::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, member)| (key.clone(), property_json(member)))
                .collect(),
        ),
    }
}

/// Checks `expect.surfaces`.
fn check_surfaces(processor: &MessageProcessor, expect: &Value) -> Result<(), String> {
    check_keys(expect, &["surfaces"])?;
    for (id, want) in expect["surfaces"]
        .as_object()
        .ok_or("surfaces is not a map")?
    {
        check_keys(
            want,
            &[
                "exists",
                "sendDataModel",
                "metadata",
                "dataModel",
                "components",
                "isValid",
                "validationErrors",
                "validationResults",
                "validationResult",
            ],
        )?;
        let surface = processor.surface(id);
        if want["exists"] == false {
            return match surface {
                None => Ok(()),
                Some(_) => Err(format!("surface {id} exists, want none")),
            };
        }
        let surface = surface.ok_or(format!("no surface {id}"))?;
        if let Some(flag) = want.get("sendDataModel") {
            same(
                &Value::Bool(surface.sends_data_model()),
                flag,
                "sendDataModel",
            )?;
        }
        if let Some(metadata) = want.get("metadata") {
            same(
                &surface.metadata().map_or(Value::Null, Metadata::to_json),
                metadata,
                "metadata",
            )?;
        }
        if let Some(model) = want.get("dataModel") {
            let root = surface
                .data()
                .get(&rho_a2ui::Pointer::root())
                .cloned()
                .unwrap_or(Value::Null);
            same(&root, model, "dataModel")?;
        }
        let resolved = resolve_all(processor, id);
        if let Some(components) = want.get("components") {
            check_components(processor, id, &resolved, components)?;
        }
        check_validation(&resolved, want)?;
    }
    Ok(())
}

/// Checks `components`: a list names every component, a map some of them,
/// by id or instance key.
fn check_components(
    processor: &MessageProcessor,
    surface: &str,
    resolved: &BTreeMap<String, ResolvedComponent>,
    want: &Value,
) -> Result<(), String> {
    let entries: Vec<(String, &Value)> = match want {
        Value::Array(list) => {
            let count = processor
                .surface(surface)
                .map_or(0, |surface| surface.components().len());
            if count != list.len() {
                return Err(format!(
                    "{surface} has {count} components, want {}",
                    list.len()
                ));
            }
            list.iter()
                .map(|entry| {
                    Ok((
                        entry["id"]
                            .as_str()
                            .ok_or("component without id")?
                            .to_owned(),
                        entry,
                    ))
                })
                .collect::<Result<_, String>>()?
        }
        Value::Object(map) => map.iter().map(|(id, entry)| (id.clone(), entry)).collect(),
        other => return Err(format!("components {other} is neither a list nor a map")),
    };
    for (key, entry) in entries {
        let component = resolved.get(&key).ok_or(format!("no component {key}"))?;
        for (field, wanted) in entry
            .as_object()
            .ok_or("component expectation is not an object")?
        {
            let got = match field.as_str() {
                "id" => continue,
                "component" => Value::from(component.kind.as_str()),
                "@metadata" => component
                    .metadata
                    .as_ref()
                    .map_or(Value::Null, Metadata::to_json),
                property => component
                    .properties
                    .get(property)
                    .map_or(Value::Null, property_json),
            };
            if !value_matches(&got, wanted) {
                return Err(format!("{key}.{field} is {got}, want {wanted}"));
            }
        }
    }
    Ok(())
}

/// Whether a resolved value is the expected one. As in the reference
/// harnesses, an integer also matches the string that writes it, since the
/// suites write `@index` results bound to a string property as strings.
fn value_matches(got: &Value, want: &Value) -> bool {
    got == want || (got.is_i64() && want.as_str() == Some(got.to_string().as_str()))
}

/// Checks `isValid`, `validationErrors`, `validationResults` and
/// `validationResult`, each a map from component to what its checks gave.
fn check_validation(
    resolved: &BTreeMap<String, ResolvedComponent>,
    want: &Value,
) -> Result<(), String> {
    for field in [
        "isValid",
        "validationErrors",
        "validationResults",
        "validationResult",
    ] {
        let Some(expected) = want.get(field).and_then(Value::as_object) else {
            continue;
        };
        for (id, wanted) in expected {
            let checks = resolved
                .get(id)
                .ok_or(format!("no component {id}"))?
                .checks
                .clone()
                .unwrap_or_default();
            let failures: Vec<Value> = checks
                .failures()
                .iter()
                .map(rho_a2ui::CheckFailure::to_json)
                .collect();
            let got = match field {
                "isValid" => Value::Bool(checks.is_valid()),
                "validationErrors" => json!(checks.errors()),
                "validationResults" => Value::Array(failures),
                _ => {
                    // The first failure, compared by the members the case
                    // states, as `ValidationResult` comparisons are.
                    let first = failures
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| json!({ "valid": true }));
                    let members = wanted
                        .as_object()
                        .ok_or("validationResult is not an object")?;
                    if members
                        .iter()
                        .all(|(key, value)| first.get(key) == Some(value))
                    {
                        continue;
                    }
                    first
                }
            };
            if got != *wanted {
                return Err(format!("{id}.{field} is {got}, want {wanted}"));
            }
        }
    }
    Ok(())
}

fn run_capabilities(case: &Value) -> Result<(), String> {
    check_keys(
        case,
        &[
            "name",
            "description",
            "features",
            "action",
            "catalogs",
            "args",
            "expect",
        ],
    )?;
    let processor = case_processor(case, Validation::Skip, Aliases::Omit)?;
    let args = &case["args"];
    check_keys(args, &["includeInlineCatalogs", "version"])?;
    if args["version"] != "1.0" {
        return Err(format!("unsupported version {}", args["version"]));
    }
    let inline = if args["includeInlineCatalogs"] == true {
        InlineCatalogs::Include
    } else {
        InlineCatalogs::Omit
    };
    same(
        &processor.renderer_capabilities(inline),
        &case["expect"],
        "capabilities",
    )
}

fn run_data_model(case: &Value) -> Result<(), String> {
    check_keys(
        case,
        &[
            "name",
            "description",
            "features",
            "action",
            "protocolVersion",
            "catalogPaths",
            "messages",
            "steps",
            "args",
            "expect",
        ],
    )?;
    if case["args"]["version"] != "1.0" {
        return Err(format!("unsupported version {}", case["args"]["version"]));
    }
    let mut processor = case_processor(case, Validation::Skip, Aliases::Add)?;
    let processed = processor.process(case.get("messages").ok_or("no messages")?);
    if let Some(error) = processed.errors().next() {
        return Err(format!("rejected: {error}"));
    }
    same(
        &processor.renderer_data_model().unwrap_or(Value::Null),
        &case["expect"],
        "renderer data model",
    )
}

/// Runs an `evaluate_function` case that names a surface or a catalog: the
/// call is evaluated on surface `main`, whose default catalog the case's
/// `surface` block names.
pub(super) fn run_function_on_surface(case: &Value) -> Result<(), String> {
    check_keys(
        case,
        &[
            "name",
            "description",
            "features",
            "action",
            "protocolVersion",
            "catalogPaths",
            "surface",
            "function",
            "catalogId",
            "args",
            "dataModel",
            "expect",
            "expectError",
        ],
    )?;
    let surface = case.get("surface").ok_or("no surface")?;
    check_keys(surface, &["catalogId"])?;
    let mut processor = case_processor(case, Validation::Skip, Aliases::Omit)?;
    let mut create = json!({ "surfaceId": "main" });
    if let Some(id) = surface.get("catalogId") {
        create["catalogId"] = id.clone();
    }
    if let Some(model) = case.get("dataModel") {
        create["dataModel"] = model.clone();
    }
    let processed = processor.process(&json!({ "version": "1.0", "createSurface": create }));
    if let Some(error) = processed.errors().next() {
        return Err(format!("createSurface rejected: {error}"));
    }
    let mut call = json!({ "@call": case["function"], "args": case.get("args").cloned().unwrap_or_else(|| json!({})) });
    if let Some(id) = case.get("catalogId") {
        call["catalogId"] = id.clone();
    }
    let expr = Expr::from_json(&call).map_err(|e| format!("call does not parse: {e}"))?;
    let outcome = processor
        .evaluate("main", &expr, &Scope::root())
        .ok_or("no surface main")?;
    match (outcome, expected_error(case)) {
        (Ok(got), None) => check_function_result(case, &got),
        (Ok(got), Some(_)) => Err(format!("returned {got}, want an error")),
        (Err(e), None) => Err(format!("failed: {e}")),
        (Err(e), Some(want)) => {
            let category = match e {
                ExpressionError::UnresolvedCatalog(_) => "CatalogError",
                _ => "ExpressionError",
            };
            if want["category"] == category {
                Ok(())
            } else {
                Err(format!(
                    "failed with {category} ({e}), want {}",
                    want["category"]
                ))
            }
        }
    }
}

/// The catalog `dispatch_action` cases run on: the basic catalog with the
/// two local functions the cases call, as the reference harnesses add them.
fn dispatch_catalog() -> (Catalog, Functions) {
    let define = |name: &str, args: Value| Entry::Define {
        name: name.to_owned(),
        schema: json!({ "returnType": "void", "properties": { "@call": { "const": name }, "args": args } }),
    };
    let mut functions: Vec<Entry> = Catalog::basic()
        .functions()
        .keys()
        .cloned()
        .map(Entry::Keep)
        .collect();
    functions.push(define(
        "incrementCounter",
        json!({ "type": "object", "properties": { "path": { "type": "string" }, "step": { "type": "number" } } }),
    ));
    functions.push(define("failingAction", json!({ "type": "object" })));
    let catalog = Catalog::basic()
        .derive(Derivation {
            catalog_id: "conformance:dispatch".to_owned(),
            components: None,
            functions: Some(functions),
            instructions: None,
        })
        .expect("the dispatch catalog derives");
    let mut implementations = Functions::basic();
    implementations.insert("incrementCounter", |args, context| {
        let path = context
            .pointer(&args.string("path"))
            .map_err(|e| rho_a2ui::FunctionError::Failed(e.to_string()))?;
        let current = context
            .read(&path.to_string())
            .map_err(|e| rho_a2ui::FunctionError::Failed(e.to_string()))?;
        let next =
            current.as_i64().unwrap_or(0) + args.get("step").and_then(Value::as_i64).unwrap_or(1);
        context.push_effect(rho_a2ui::Effect::SetData {
            path,
            value: json!(next),
        });
        Ok(Value::Null)
    });
    implementations.insert("failingAction", |_, _| {
        Err(rho_a2ui::FunctionError::Failed(
            "Action execution failed".to_owned(),
        ))
    });
    (catalog, implementations)
}

fn run_dispatch(case: &Value) -> Result<(), String> {
    check_keys(
        case,
        &[
            "name",
            "description",
            "features",
            "action",
            "protocolVersion",
            "surfaceId",
            "sourceComponentId",
            "scope",
            "dataModel",
            "actionPayload",
            "metadata",
            "userActivation",
            "expectDispatched",
            "expectDataModel",
            "expectError",
        ],
    )?;
    let surface = case["surfaceId"].as_str().unwrap_or("main");
    let mut processor = dispatch_processor(case, surface)?;
    let want_error = expected_error(case);
    // An action or metadata the crate refuses to read never reaches a
    // surface; the case's error is that refusal.
    let action = match Action::from_json(&case["actionPayload"]) {
        Ok(action) => action,
        Err(e) => {
            return match want_error {
                Some(want) if want.get("code").is_none_or(|code| code == "INVALID_ACTION") => {
                    Ok(())
                }
                _ => Err(format!("action refused: {e}")),
            };
        }
    };
    let metadata = match case.get("metadata").map(Metadata::from_json).transpose() {
        Ok(metadata) => metadata,
        Err(e) => {
            return match want_error {
                Some(want) if want["category"] == "ValidationError" => Ok(()),
                _ => Err(format!("metadata refused: {e}")),
            };
        }
    };
    let interaction = interaction(case, surface, metadata)?;
    let dispatched = processor
        .dispatch_action(&interaction, &action)
        .ok_or("no surface")?;
    let error = dispatched
        .outbound
        .iter()
        .find_map(|message| match message {
            RendererMessage::Error(error) => Some(error),
            _ => None,
        });
    match (error, want_error) {
        (Some(error), Some(want)) => check_error(want, error, Source::Message)?,
        (None, Some(want)) => return Err(format!("dispatched, want an error {want}")),
        (Some(error), None) => return Err(format!("failed: {error}")),
        (None, None) => {}
    }
    let action = dispatched
        .outbound
        .iter()
        .find_map(|message| match message {
            RendererMessage::Action(_) => Some(message.to_json()["action"].clone()),
            _ => None,
        });
    match (case.get("expectDispatched"), action) {
        (None | Some(Value::Null), None) | (None, Some(_)) => {}
        (Some(Value::Null), Some(got)) => return Err(format!("dispatched {got}, want nothing")),
        (Some(want), Some(got)) => check_dispatched(want, &got)?,
        (Some(want), None) => return Err(format!("dispatched nothing, want {want}")),
    }
    if let Some(model) = case.get("expectDataModel") {
        let root = processor
            .surface(surface)
            .and_then(|surface| surface.data().get(&rho_a2ui::Pointer::root()).cloned())
            .unwrap_or(Value::Null);
        same(&root, model, "data model")?;
    }
    Ok(())
}

/// The interaction a `dispatch_action` case describes.
fn interaction<'a>(
    case: &'a Value,
    surface: &'a str,
    metadata: Option<Metadata>,
) -> Result<Interaction<'a>, String> {
    let scope = match case["scope"].as_str() {
        None => Scope::root(),
        Some(path) => {
            let index = path
                .rsplit('/')
                .next()
                .and_then(|last| last.parse().ok())
                .ok_or("scope is not a list item")?;
            Scope::item(path.to_owned(), index)
        }
    };
    let activation = if case["userActivation"] == false {
        UserActivation::Absent
    } else {
        UserActivation::Present
    };
    Ok(Interaction {
        surface_id: surface,
        source_component_id: case["sourceComponentId"].as_str().unwrap_or("test_comp"),
        scope,
        activation,
        metadata,
        at: SystemTime::now(),
    })
}

/// A processor with surface `surface` on the dispatch catalog, holding the
/// case's data model.
fn dispatch_processor(case: &Value, surface: &str) -> Result<MessageProcessor, String> {
    let mut processor = MessageProcessor::new(Validation::Skip);
    let (catalog, functions) = dispatch_catalog();
    let catalog_id = catalog.id().to_owned();
    processor
        .add_catalog(catalog, functions)
        .map_err(|e| e.to_string())?;
    let mut create = json!({ "surfaceId": surface, "catalogId": catalog_id });
    if let Some(model) = case.get("dataModel") {
        create["dataModel"] = model.clone();
    }
    match processor
        .process(&json!({ "version": "1.0", "createSurface": create }))
        .errors()
        .next()
    {
        Some(error) => Err(format!("createSurface rejected: {error}")),
        None => Ok(processor),
    }
}

fn check_dispatched(want: &Value, got: &Value) -> Result<(), String> {
    check_keys(
        want,
        &[
            "name",
            "context",
            "userMessage",
            "surfaceId",
            "sourceComponentId",
            "metadata",
            "timestampIso8601",
        ],
    )?;
    for key in [
        "name",
        "context",
        "userMessage",
        "surfaceId",
        "sourceComponentId",
        "metadata",
    ] {
        if let Some(wanted) = want.get(key) {
            same(got.get(key).unwrap_or(&Value::Null), wanted, key)?;
        }
    }
    if want["timestampIso8601"] == true && got["timestamp"].as_str().is_none_or(str::is_empty) {
        return Err(format!(
            "timestamp is {}, want an ISO 8601 string",
            got["timestamp"]
        ));
    }
    Ok(())
}

fn run_select_catalog(case: &Value) -> Result<(), String> {
    check_keys(
        case,
        &[
            "name",
            "description",
            "features",
            "action",
            "catalog",
            "args",
            "expectSelected",
            "expectError",
        ],
    )?;
    let args = &case["args"];
    check_keys(args, &["surface", "components", "functionCall", "catalogs"])?;
    let surface = &args["surface"];
    let id = surface["id"].as_str().ok_or("no surface id")?;
    let mut processor = MessageProcessor::new(Validation::Skip);
    let mut outcome = Ok(String::new());
    for catalog in surface["supportedCatalogIds"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let catalog = catalog.as_str().ok_or("catalog id is not a string")?;
        let version = args["catalogs"][catalog]["protocolVersion"]
            .as_str()
            .unwrap_or("1.0");
        match Catalog::from_json(json!({ "catalogId": catalog, "protocolVersion": version })) {
            Ok(loaded) => processor
                .add_catalog(loaded, Functions::new())
                .map_err(|e| e.to_string())?,
            Err(e) => outcome = Err(("CatalogError", e.to_string())),
        }
    }
    if outcome.is_ok() {
        let components: Vec<Value> = args["components"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(id, component)| {
                let mut component = component.clone();
                component["id"] = Value::from(id.as_str());
                component
            })
            .collect();
        let mut create = json!({ "surfaceId": id, "catalogId": surface["defaultCatalogId"] });
        if !components.is_empty() {
            create["components"] = Value::Array(components.clone());
        }
        let processed = processor.process(&json!({ "version": "1.0", "createSurface": create }));
        outcome = match processed.errors().next() {
            Some(error) => Err((categories(error.code())[0], error.to_string())),
            None => match (&args["functionCall"], components.last()) {
                (Value::Object(call), _) => processor
                    .resolve_catalog(id, call.get("catalogId").and_then(Value::as_str))
                    .map(|catalog| catalog.id().to_owned())
                    .map_err(|e| ("CatalogError", e.to_string())),
                (_, Some(last)) => processor
                    .surface(id)
                    .and_then(|surface| surface.component(last["id"].as_str().unwrap_or_default()))
                    .map(|component| component.catalog_id().to_owned())
                    .ok_or(("ValidationError", "no component".to_owned())),
                (_, None) => Err((
                    "ValidationError",
                    "nothing to select a catalog for".to_owned(),
                )),
            },
        };
    }
    match (outcome, case.get("expectSelected"), expected_error(case)) {
        (Ok(got), Some(want), None) if want == got.as_str() => Ok(()),
        (Ok(got), _, _) => Err(format!("selected {got}, want {}", case["expectSelected"])),
        (Err((category, _)), None, Some(want)) if want["category"] == category => Ok(()),
        (Err((category, e)), _, _) => Err(format!("failed with {category} ({e})")),
    }
}
