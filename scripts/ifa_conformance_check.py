#!/usr/bin/env python3
"""IFA Section 13 artifact conformance gate.

This script validates the five mandatory operational artifacts:
1) Invariant Registry
2) Authority Boundary Map
3) Parametricity Rules
4) Move Semantics Rules
5) DRY Proof Map
"""

from __future__ import annotations

from pathlib import Path
import sys

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:
    print("error: Python 3.11+ is required (missing tomllib).", file=sys.stderr)
    sys.exit(2)


ROOT = Path(__file__).resolve().parent.parent
IFA_DIR = ROOT / "ifa"

INVARIANT_REGISTRY = IFA_DIR / "invariant_registry.toml"
AUTHORITY_BOUNDARY_MAP = IFA_DIR / "authority_boundary_map.toml"
PARAMETRICITY_RULES = IFA_DIR / "parametricity_rules.toml"
MOVE_SEMANTICS_RULES = IFA_DIR / "move_semantics_rules.toml"
DRY_PROOF_MAP = IFA_DIR / "dry_proof_map.toml"

REQUIRED_FILES = [
    INVARIANT_REGISTRY,
    AUTHORITY_BOUNDARY_MAP,
    PARAMETRICITY_RULES,
    MOVE_SEMANTICS_RULES,
    DRY_PROOF_MAP,
]

ALLOWED_VISIBILITY = {"private", "pub(super)", "pub(crate)"}


def _load_toml(path: Path) -> dict:
    with path.open("rb") as f:
        data = tomllib.load(f)
    if not isinstance(data, dict):
        raise ValueError(f"{path}: top-level TOML value must be a table")
    return data


def _require_non_empty_str(value: object, field_name: str, path: Path) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{path}: field '{field_name}' must be a non-empty string")
    return value


def _require_non_empty_list(value: object, field_name: str, path: Path) -> list:
    if not isinstance(value, list) or not value:
        raise ValueError(f"{path}: field '{field_name}' must be a non-empty list")
    return value


def _validate_common_metadata(data: dict, path: Path) -> None:
    version = data.get("version")
    if not isinstance(version, int) or version <= 0:
        raise ValueError(f"{path}: 'version' must be a positive integer")


def validate_invariant_registry(path: Path) -> set[str]:
    data = _load_toml(path)
    _validate_common_metadata(data, path)
    entries = _require_non_empty_list(data.get("invariants"), "invariants", path)

    ids: set[str] = set()
    for idx, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise ValueError(f"{path}: invariants[{idx}] must be a table")
        inv_id = _require_non_empty_str(entry.get("id"), "id", path)
        _require_non_empty_str(entry.get("predicate"), "predicate", path)
        _require_non_empty_str(entry.get("canonical_proof_type_path"), "canonical_proof_type_path", path)
        _require_non_empty_str(entry.get("authority_boundary_module_path"), "authority_boundary_module_path", path)
        if inv_id in ids:
            raise ValueError(f"{path}: duplicate invariant id '{inv_id}'")
        ids.add(inv_id)
    return ids


def validate_authority_boundary_map(path: Path) -> None:
    data = _load_toml(path)
    _validate_common_metadata(data, path)
    entries = _require_non_empty_list(data.get("entries"), "entries", path)

    for idx, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise ValueError(f"{path}: entries[{idx}] must be a table")
        _require_non_empty_str(entry.get("controlled_type_path"), "controlled_type_path", path)
        _require_non_empty_str(entry.get("boundary_module_path"), "boundary_module_path", path)
        _require_non_empty_list(entry.get("constructor_paths"), "constructor_paths", path)
        _require_non_empty_list(entry.get("allowed_caller_module_paths"), "allowed_caller_module_paths", path)
        vis = _require_non_empty_str(entry.get("max_constructor_visibility_rung"), "max_constructor_visibility_rung", path)
        if vis not in ALLOWED_VISIBILITY:
            raise ValueError(
                f"{path}: entries[{idx}].max_constructor_visibility_rung must be one of {sorted(ALLOWED_VISIBILITY)}"
            )


def validate_parametricity_rules(path: Path) -> None:
    data = _load_toml(path)
    _validate_common_metadata(data, path)
    _require_non_empty_list(data.get("banned_patterns"), "banned_patterns", path)
    _require_non_empty_list(data.get("required_interface_disclosures"), "required_interface_disclosures", path)


def validate_move_semantics_rules(path: Path) -> None:
    data = _load_toml(path)
    _validate_common_metadata(data, path)
    entries = _require_non_empty_list(data.get("state_bearing_types"), "state_bearing_types", path)
    for idx, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise ValueError(f"{path}: state_bearing_types[{idx}] must be a table")
        _require_non_empty_str(entry.get("type_path"), "type_path", path)
        transitions = _require_non_empty_list(entry.get("consumed_transition_methods"), "consumed_transition_methods", path)
        for t_idx, transition in enumerate(transitions):
            if not isinstance(transition, dict):
                raise ValueError(
                    f"{path}: state_bearing_types[{idx}].consumed_transition_methods[{t_idx}] must be a table"
                )
            _require_non_empty_str(transition.get("method_path"), "method_path", path)
            consumes_self = transition.get("consumes_self")
            if not isinstance(consumes_self, bool) or not consumes_self:
                raise ValueError(f"{path}: transition '{transition}' must set consumes_self=true")
            _require_non_empty_str(transition.get("post_move_unusability_guarantee"), "post_move_unusability_guarantee", path)


def validate_dry_proof_map(path: Path, known_ids: set[str]) -> None:
    data = _load_toml(path)
    _validate_common_metadata(data, path)
    entries = _require_non_empty_list(data.get("entries"), "entries", path)
    seen_ids: set[str] = set()
    for idx, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise ValueError(f"{path}: entries[{idx}] must be a table")
        inv_id = _require_non_empty_str(entry.get("invariant_id"), "invariant_id", path)
        _require_non_empty_str(entry.get("canonical_proof_type_path"), "canonical_proof_type_path", path)
        _require_non_empty_str(entry.get("authority_boundary_module_path"), "authority_boundary_module_path", path)
        if inv_id in seen_ids:
            raise ValueError(f"{path}: duplicate invariant_id '{inv_id}'")
        seen_ids.add(inv_id)
        if inv_id not in known_ids:
            raise ValueError(f"{path}: invariant_id '{inv_id}' not present in invariant registry")

    missing = known_ids - seen_ids
    if missing:
        raise ValueError(f"{path}: missing invariant IDs from DRY proof map: {sorted(missing)}")


def main() -> int:
    missing = [path for path in REQUIRED_FILES if not path.exists()]
    if missing:
        for path in missing:
            print(f"error: missing required IFA artifact: {path}", file=sys.stderr)
        return 1

    try:
        invariant_ids = validate_invariant_registry(INVARIANT_REGISTRY)
        validate_authority_boundary_map(AUTHORITY_BOUNDARY_MAP)
        validate_parametricity_rules(PARAMETRICITY_RULES)
        validate_move_semantics_rules(MOVE_SEMANTICS_RULES)
        validate_dry_proof_map(DRY_PROOF_MAP, invariant_ids)
    except ValueError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1

    print("IFA artifact check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
