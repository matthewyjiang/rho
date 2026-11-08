from __future__ import annotations

import copy
import json
import unittest
from collections.abc import Callable

from scripts import check_protocol_compat

RELEASED = json.loads((check_protocol_compat.ROOT / check_protocol_compat.SCHEMA).read_text())
MAJOR, MINOR = check_protocol_compat.version(RELEASED)


def changed(change: Callable[[dict], None], *, major: int = 0, minor: int = 1) -> dict:
    """The committed schema with `change` applied, its version raised by
    `major` and `minor`; a new major starts its minors at 0."""
    document = copy.deepcopy(RELEASED)
    change(document)
    document["protocol"] = (
        {"major": MAJOR + major, "minor": 0} if major else {"major": MAJOR, "minor": MINOR + minor}
    )
    return document


def defs(document: dict) -> dict:
    return document["$defs"]


def add_optional_field(document: dict) -> None:
    defs(document)["SessionAttachParams"]["properties"]["follow"] = {"type": ["boolean", "null"]}


def add_required_field(document: dict) -> None:
    add_optional_field(document)
    defs(document)["SessionAttachParams"]["required"].append("follow")


def require_optional_field(document: dict) -> None:
    defs(document)["SessionAttachParams"]["required"].append("after")


def retype_field(document: dict) -> None:
    defs(document)["Seq"]["type"] = "string"


def remove_event_field(document: dict) -> None:
    for variant in defs(document)["SessionEvent"]["anyOf"]:
        variant["properties"].pop("text", None)
        if "text" in variant.get("required", []):
            variant["required"].remove("text")


def add_value(name: str) -> Callable[[dict], None]:
    def add(document: dict) -> None:
        enum = defs(document)[name]
        enum.get("oneOf", enum.get("anyOf")).insert(0, {"const": "paused", "type": "string"})

    return add


def close_enum(document: dict) -> None:
    enum = defs(document)["DetachReason"]
    enum["anyOf"] = [variant for variant in enum["anyOf"] if "x-rho-other" not in variant]


def remove_method(document: dict) -> None:
    del document["methods"]["rho/session/steer"]


def add_method(document: dict) -> None:
    document["methods"]["rho/session/fork"] = copy.deepcopy(
        document["methods"]["rho/session/cancel"]
    )


def remove_error(document: dict) -> None:
    del document["errors"]["-32011"]


def refuse_unknown_fields(document: dict) -> None:
    defs(document)["HelloResponse"]["additionalProperties"] = False


def cap_length(document: dict) -> None:
    defs(document)["SessionSteerParams"]["properties"]["text"]["maxLength"] = 10


def reword(document: dict) -> None:
    defs(document)["SessionEvent"]["description"] = "Something that happened."


def rename_error(document: dict) -> None:
    document["errors"]["-32003"]["name"] = "SessionUnknown"


def inline_option(document: dict) -> None:
    """`Option<JobId>` written as a nullable string, as `Option<String>` is."""
    parent = defs(document)["JobSummary"]["properties"]["parent"]
    del parent["anyOf"]
    parent["type"] = ["string", "null"]


def reorder_option(document: dict) -> None:
    defs(document)["ToolCallUpdate"]["properties"]["kind"]["anyOf"].reverse()


def unmodelled_keyword(document: dict) -> None:
    defs(document)["SessionSteerParams"]["properties"]["text"]["if"] = {"type": "string"}


def recursive_type(document: dict) -> None:
    defs(document)["Cursor"]["properties"]["next"] = {"$ref": "#/$defs/Cursor"}


def close_event(document: dict) -> None:
    defs(document)["SessionEvent"]["additionalProperties"] = False


def narrow_through_all_of(document: dict) -> None:
    defs(document)["Replay"]["allOf"] = [{"enum": ["tail"]}]


def cap_twice(document: dict) -> None:
    text = defs(document)["SessionSteerParams"]["properties"]["text"]
    text["maxLength"] = 10
    text["allOf"] = [{"maxLength": 3}]


def narrow_other(document: dict) -> None:
    """The open enum's catch-all back to the literal it replaces."""
    enum = defs(document)["DetachReason"]
    other = next(variant for variant in enum["anyOf"] if "x-rho-other" in variant)
    other["const"] = "unknown"


def other_under_one_of(document: dict) -> None:
    enum = defs(document)["JobKind"]
    enum["oneOf"] = enum.pop("anyOf")


def narrow_other_tag(document: dict) -> None:
    enum = defs(document)["ToolContent"]
    other = next(variant for variant in enum["anyOf"] if "x-rho-other" in variant)
    other["properties"]["type"]["const"] = "unknown"


class CompatibilityTests(unittest.TestCase):
    def check(self, current: dict) -> list[str]:
        return check_protocol_compat.check(RELEASED, current, "the release")

    def test_a_minor_may_only_add(self) -> None:
        for name, change in [
            ("an optional field", add_optional_field),
            ("a method", add_method),
            ("a value of an open enum", add_value("DetachReason")),
        ]:
            with self.subTest(name):
                self.assertEqual(self.check(changed(change)), [])

    def test_breaking_changes_need_a_major(self) -> None:
        for name, change, expected in [
            (
                "a new required field",
                add_required_field,
                "method rho/session/attach params.follow: added as required",
            ),
            (
                "a field made required",
                require_optional_field,
                "method rho/session/attach params.after required: optional became required",
            ),
            (
                "a retyped field",
                retype_field,
                "notification rho/session/event params.seq: "
                'integer (format "uint64", minimum 0) became string (format "uint64", minimum 0)',
            ),
            (
                "a removed event field",
                remove_event_field,
                'notification rho/session/event params.event <type="notice">.text: removed',
            ),
            (
                "a value added to a closed enum",
                add_value("Replay"),
                'method rho/session/attach result.replay: added "paused" to a closed enum',
            ),
            (
                "an open enum closed",
                close_enum,
                "notification rho/session/detached params.reason: "
                'one of "daemon_stopping", "lagged" (open) became one of "daemon_stopping", "lagged"',
            ),
            ("a removed method", remove_method, "method rho/session/steer params: removed"),
            ("a removed error code", remove_error, "error -32011: removed"),
            (
                "unknown fields refused",
                refuse_unknown_fields,
                "method rho/hello result: object became object (no unknown fields)",
            ),
            (
                "a constrained field",
                cap_length,
                "method rho/session/steer params.text: string became string (maxLength 10)",
            ),
        ]:
            with self.subTest(name):
                problems = self.check(changed(change))
                self.assertIn(f"{expected} (needs a new protocol major)", problems)
                self.assertEqual(self.check(changed(change, major=1)), [])

    def test_any_change_to_the_wire_needs_a_new_minor(self) -> None:
        self.assertEqual(
            self.check(changed(add_method, minor=0)),
            [
                "the protocol changed since the release but PROTOCOL_VERSION is still "
                f"{MAJOR}.{MINOR}; raise the minor"
            ],
        )

    def test_rewrites_that_keep_the_wire_change_nothing(self) -> None:
        for name, change in [
            ("a description", reword),
            ("an error's name", rename_error),
            ("an option written as a nullable type", inline_option),
            ("an option's variants reordered", reorder_option),
        ]:
            with self.subTest(name):
                self.assertEqual(self.check(changed(change, minor=0)), [])

    def test_schemas_the_check_cannot_judge_fail_loudly(self) -> None:
        for name, change, expected in [
            (
                "an unmodelled keyword",
                unmodelled_keyword,
                "method rho/session/steer params.text: the check does not model if",
            ),
            (
                "a closed tagged enum",
                close_event,
                "notification rho/session/event params.event: the check does not model "
                "additionalProperties, anyOf together",
            ),
            (
                "an enum narrowed through allOf",
                narrow_through_all_of,
                "method rho/session/attach result.replay: the check does not model enum, oneOf together",
            ),
            (
                "a constraint given twice",
                cap_twice,
                "method rho/session/steer params.text: the check does not model two values of maxLength",
            ),
            (
                "an open enum's catch-all narrowed",
                narrow_other,
                "notification rho/session/detached params.reason: its x-rho-other variant must be "
                "any string or any tagged object, under anyOf",
            ),
            (
                "an open enum under oneOf",
                other_under_one_of,
                'method rho/job/list result.jobs[].kind: its x-rho-other variant must be '
                "any string or any tagged object, under anyOf",
            ),
            (
                "an open tagged enum's catch-all tag narrowed",
                narrow_other_tag,
                'notification rho/session/event params.event <type="tool_call">.content[]: its '
                "x-rho-other variant must be any string or any tagged object, under anyOf",
            ),
            (
                "a recursive type",
                recursive_type,
                "method rho/session/attach params.after.next: Cursor contains itself; "
                "the check does not model recursion",
            ),
        ]:
            with self.subTest(name):
                with self.assertRaises(ValueError) as raised:
                    self.check(changed(change))
                self.assertEqual(str(raised.exception), expected)

    def test_a_field_added_to_a_closed_object_needs_a_major(self) -> None:
        closed = changed(refuse_unknown_fields, minor=0)
        with_field = copy.deepcopy(closed)
        defs(with_field)["HelloResponse"]["properties"]["uptime"] = {"type": ["integer", "null"]}
        with_field["protocol"]["minor"] += 1
        self.assertEqual(
            check_protocol_compat.check(closed, with_field, "the release"),
            [
                "method rho/hello result.uptime: added to an object that refuses unknown fields "
                "(needs a new protocol major)"
            ],
        )

    def test_the_version_never_goes_down(self) -> None:
        released = changed(lambda _: None, minor=3)
        self.assertEqual(
            check_protocol_compat.check(released, RELEASED, "the release"),
            [f"PROTOCOL_VERSION {MAJOR}.{MINOR} is below the release's {MAJOR}.{MINOR + 3}"],
        )


class LatestReleaseTests(unittest.TestCase):
    def test_picks_the_newest_version_not_the_last_in_text_order(self) -> None:
        tags = [
            "rho-coding-agent-v2.9.0",
            "rho-coding-agent-v2.33.1",
            "rho-coding-agent-v2.4.10",
            "rho-sdk-v6.0.0",
        ]
        self.assertEqual(check_protocol_compat.latest_release(tags), "rho-coding-agent-v2.33.1")
        self.assertIsNone(check_protocol_compat.latest_release(["rho-sdk-v6.0.0"]))


if __name__ == "__main__":
    unittest.main()
