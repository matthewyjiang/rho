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
    for variant in defs(document)["SessionEvent"]["oneOf"]:
        variant["properties"].pop("text", None)
        if "text" in variant.get("required", []):
            variant["required"].remove("text")


def add_value(name: str) -> Callable[[dict], None]:
    def add(document: dict) -> None:
        defs(document)[name]["oneOf"].append({"const": "paused", "type": "string"})

    return add


def remove_method(document: dict) -> None:
    del document["methods"]["rho/session/steer"]


def add_method(document: dict) -> None:
    document["methods"]["rho/session/fork"] = copy.deepcopy(
        document["methods"]["rho/session/cancel"]
    )


def reword(document: dict) -> None:
    defs(document)["SessionEvent"]["description"] = "Something that happened."


class CompatibilityTests(unittest.TestCase):
    def check(self, current: dict) -> list[str]:
        return check_protocol_compat.check(RELEASED, current, "the release")

    def test_a_minor_may_only_add(self) -> None:
        for name, change in [
            ("an optional field", add_optional_field),
            ("a method", add_method),
            ("a value of an enum that reads `unknown`", add_value("DetachReason")),
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
                "notification rho/session/event params.seq: integer (uint64) became string (uint64)",
            ),
            (
                "a removed event field",
                remove_event_field,
                'notification rho/session/event params.event <type="notice">.text: removed',
            ),
            (
                "a value added to an enum without `unknown`",
                add_value("Replay"),
                'method rho/session/attach result.replay: added "paused" to an enum without `unknown`',
            ),
            ("a removed method", remove_method, "method rho/session/steer params: removed"),
        ]:
            with self.subTest(name):
                problems = self.check(changed(change))
                self.assertIn(f"{expected} (needs a new protocol major)", problems)
                self.assertEqual(self.check(changed(change, major=1)), [])

    def test_any_change_but_a_description_needs_a_new_minor(self) -> None:
        self.assertEqual(
            self.check(changed(add_method, minor=0)),
            [
                "the protocol changed since the release but PROTOCOL_VERSION is still "
                f"{MAJOR}.{MINOR}; raise the minor"
            ],
        )
        self.assertEqual(self.check(changed(reword, minor=0)), [])

    def test_the_version_never_goes_down(self) -> None:
        released = changed(lambda _: None, minor=3)
        self.assertEqual(
            check_protocol_compat.check(released, RELEASED, "the release"),
            [f"PROTOCOL_VERSION {MAJOR}.{MINOR} is below the release's {MAJOR}.{MINOR + 3}"],
        )


if __name__ == "__main__":
    unittest.main()
