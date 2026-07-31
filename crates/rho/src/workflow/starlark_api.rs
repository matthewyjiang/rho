use starlark::{
    environment::{Globals, GlobalsBuilder},
    eval::Evaluator,
    starlark_module,
    values::{dict::AllocDict, none::NoneType, Value},
};

pub(crate) fn globals() -> Globals {
    let mut builder = GlobalsBuilder::standard();
    workflow_api(&mut builder);
    builder.namespace("input", input_api);
    builder.build()
}

fn object<'v>(eval: &Evaluator<'v, '_, '_>, fields: Vec<(&str, Value<'v>)>) -> Value<'v> {
    let heap = eval.heap();
    heap.alloc(AllocDict(
        fields
            .into_iter()
            .map(|(key, value)| (heap.alloc(key), value)),
    ))
}

fn tagged<'v>(eval: &Evaluator<'v, '_, '_>, kind: &str) -> Value<'v> {
    object(eval, vec![("__rho_type", eval.heap().alloc(kind))])
}

#[starlark_module]
fn input_api(builder: &mut GlobalsBuilder) {
    fn string<'v>(
        #[starlark(default = NoneType)] default: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("input_string")),
                ("default", default),
            ],
        ))
    }

    fn integer<'v>(
        #[starlark(default = NoneType)] default: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("input_integer")),
                ("default", default),
            ],
        ))
    }

    fn bool<'v>(
        #[starlark(default = NoneType)] default: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("input_bool")),
                ("default", default),
            ],
        ))
    }

    fn enum_<'v>(
        members: Value<'v>,
        #[starlark(default = NoneType)] default: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("input_enum")),
                ("members", members),
                ("default", default),
            ],
        ))
    }
}

#[starlark_module]
fn workflow_api(builder: &mut GlobalsBuilder) {
    fn define<'v>(
        inputs: Value<'v>,
        build: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("definition")),
                ("inputs", inputs),
                ("build", build),
            ],
        ))
    }

    fn workflow<'v>(
        name: &str,
        nodes: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("workflow")),
                ("name", eval.heap().alloc(name)),
                ("nodes", nodes),
            ],
        ))
    }

    fn template<'v>(
        parts: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("template")),
                ("parts", parts),
            ],
        ))
    }

    fn output<'v>(
        node: &str,
        path: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("output_ref")),
                ("node", eval.heap().alloc(node)),
                ("path", path),
            ],
        ))
    }

    fn status<'v>(node: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("status_ref")),
                ("node", eval.heap().alloc(node)),
            ],
        ))
    }

    fn exit_code<'v>(node: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("exit_code_ref")),
                ("node", eval.heap().alloc(node)),
            ],
        ))
    }

    fn equals<'v>(
        reference: Value<'v>,
        value: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("equals")),
                ("reference", reference),
                ("value", value),
            ],
        ))
    }

    fn is_one_of<'v>(
        reference: Value<'v>,
        values: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("is_one_of")),
                ("reference", reference),
                ("values", values),
            ],
        ))
    }

    fn all<'v>(
        conditions: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("all")),
                ("conditions", conditions),
            ],
        ))
    }

    fn any<'v>(
        conditions: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("any")),
                ("conditions", conditions),
            ],
        ))
    }

    fn not<'v>(
        condition: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("not")),
                ("condition", condition),
            ],
        ))
    }

    fn null<'v>(eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        Ok(tagged(eval, "schema_null"))
    }
    fn bool<'v>(eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        Ok(tagged(eval, "schema_bool"))
    }
    fn integer<'v>(eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        Ok(tagged(eval, "schema_integer"))
    }
    fn string<'v>(eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        Ok(tagged(eval, "schema_string"))
    }

    fn enum_<'v>(
        members: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("schema_enum")),
                ("members", members),
            ],
        ))
    }

    fn list<'v>(item: Value<'v>, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("schema_list")),
                ("item", item),
            ],
        ))
    }

    fn optional<'v>(
        schema: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("schema_optional")),
                ("schema", schema),
            ],
        ))
    }

    fn record<'v>(
        fields: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("schema_record")),
                ("fields", fields),
            ],
        ))
    }

    fn stdout_json<'v>(
        schema: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(object(
            eval,
            vec![
                ("__rho_type", eval.heap().alloc("stdout_json")),
                ("schema", schema),
            ],
        ))
    }

    // Keep named workflow fields explicit for readable source.
    #[allow(clippy::too_many_arguments)]
    fn agent<'v>(
        name: &str,
        agent: &str,
        prompt: Value<'v>,
        #[starlark(default = "mutating")] access: &str,
        #[starlark(default = NoneType)] needs: Value<'v>,
        #[starlark(default = NoneType)] when: Value<'v>,
        #[starlark(default = false)] allow_failure: bool,
        #[starlark(default = NoneType)] output: Value<'v>,
        #[starlark(default = NoneType)] timeout_seconds: Value<'v>,
        #[starlark(default = NoneType)] max_output_bytes: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(node(
            eval,
            "agent",
            name,
            needs,
            when,
            allow_failure,
            timeout_seconds,
            max_output_bytes,
            vec![
                ("agent", eval.heap().alloc(agent)),
                ("prompt", prompt),
                ("access", eval.heap().alloc(access)),
                ("output", output),
            ],
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn command<'v>(
        name: &str,
        argv: Value<'v>,
        #[starlark(default = ".")] cwd: &str,
        #[starlark(default = NoneType)] needs: Value<'v>,
        #[starlark(default = NoneType)] when: Value<'v>,
        #[starlark(default = false)] allow_failure: bool,
        #[starlark(default = NoneType)] output: Value<'v>,
        #[starlark(default = NoneType)] timeout_seconds: Value<'v>,
        #[starlark(default = NoneType)] max_output_bytes: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(node(
            eval,
            "command",
            name,
            needs,
            when,
            allow_failure,
            timeout_seconds,
            max_output_bytes,
            vec![
                ("argv", argv),
                ("cwd", eval.heap().alloc(cwd)),
                ("output", output),
            ],
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn shell<'v>(
        name: &str,
        executable: &str,
        arguments: Value<'v>,
        command: &str,
        #[starlark(default = ".")] cwd: &str,
        #[starlark(default = NoneType)] needs: Value<'v>,
        #[starlark(default = NoneType)] when: Value<'v>,
        #[starlark(default = false)] allow_failure: bool,
        #[starlark(default = NoneType)] output: Value<'v>,
        #[starlark(default = NoneType)] timeout_seconds: Value<'v>,
        #[starlark(default = NoneType)] max_output_bytes: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(node(
            eval,
            "shell",
            name,
            needs,
            when,
            allow_failure,
            timeout_seconds,
            max_output_bytes,
            vec![
                ("executable", eval.heap().alloc(executable)),
                ("arguments", arguments),
                ("command", eval.heap().alloc(command)),
                ("cwd", eval.heap().alloc(cwd)),
                ("output", output),
            ],
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn node<'v>(
    eval: &Evaluator<'v, '_, '_>,
    kind: &str,
    name: &str,
    needs: Value<'v>,
    when: Value<'v>,
    allow_failure: bool,
    timeout_seconds: Value<'v>,
    max_output_bytes: Value<'v>,
    extra: Vec<(&str, Value<'v>)>,
) -> Value<'v> {
    let mut fields = vec![
        ("__rho_type", eval.heap().alloc(kind)),
        ("name", eval.heap().alloc(name)),
        ("needs", needs),
        ("when", when),
        ("allow_failure", eval.heap().alloc(allow_failure)),
        ("timeout_seconds", timeout_seconds),
        ("max_output_bytes", max_output_bytes),
    ];
    fields.extend(extra);
    object(eval, fields)
}
