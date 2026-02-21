#!/usr/bin/env python3
"""IFA artifact conformance gate.

This script validates the five Section 13 operational artifacts plus
the supplemental classification map used by the phase-0 guardrail plan:
1) Invariant Registry
2) Authority Boundary Map
3) Parametricity Rules
4) Move Semantics Rules
5) DRY Proof Map
6) Module Classification Map
"""

from __future__ import annotations

from pathlib import Path
import re
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
CLASSIFICATION_MAP = IFA_DIR / "classification_map.toml"

REQUIRED_FILES = [
    INVARIANT_REGISTRY,
    AUTHORITY_BOUNDARY_MAP,
    PARAMETRICITY_RULES,
    MOVE_SEMANTICS_RULES,
    DRY_PROOF_MAP,
    CLASSIFICATION_MAP,
]

ALLOWED_VISIBILITY = {"private", "pub(super)", "pub(crate)", "pub"}
VISIBILITY_RUNG_ORDER = {"private": 0, "pub(super)": 1, "pub(crate)": 2, "pub": 3}
ALLOWED_CLASSIFICATIONS = {"core", "boundary"}
BANNED_CORE_ENUM_VARIANTS = {"None", "Empty", "Unknown", "Default"}
CARGO_TOML = ROOT / "Cargo.toml"


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


def _workspace_members() -> list[str]:
    root = _load_toml(CARGO_TOML)
    workspace = root.get("workspace")
    if not isinstance(workspace, dict):
        raise ValueError(f"{CARGO_TOML}: missing [workspace] table")
    members = _require_non_empty_list(workspace.get("members"), "workspace.members", CARGO_TOML)
    out: list[str] = []
    for idx, member in enumerate(members):
        if not isinstance(member, str) or not member.strip():
            raise ValueError(f"{CARGO_TOML}: workspace.members[{idx}] must be a non-empty string")
        out.append(member)
    return out


def validate_workspace_readmes(workspace_members: list[str]) -> None:
    missing: list[Path] = []
    for crate in workspace_members:
        readme = ROOT / crate / "README.md"
        if not readme.exists():
            missing.append(readme)

    if missing:
        missing_paths = ", ".join(path.relative_to(ROOT).as_posix() for path in missing)
        raise ValueError(
            "workspace crates missing README.md: "
            f"{missing_paths}"
        )


def _crate_source_files(crate: str) -> list[Path]:
    src_dir = ROOT / crate / "src"
    if not src_dir.exists():
        raise ValueError(f"{CARGO_TOML}: workspace member '{crate}' has no src directory")
    return sorted(src_dir.rglob("*.rs"))


def _all_source_files(workspace_members: list[str]) -> list[str]:
    files: list[str] = []
    for crate in workspace_members:
        for path in _crate_source_files(crate):
            files.append(path.relative_to(ROOT).as_posix())
    if not files:
        raise ValueError("no Rust source files found in workspace members")
    return sorted(files)


def _validate_relative_prefix(prefix: str, path: Path, index: int) -> None:
    if prefix.startswith("/") or prefix.startswith("./") or ".." in prefix.split("/"):
        raise ValueError(f"{path}: rules[{index}].prefix must be a workspace-relative path prefix")


def validate_classification_map(path: Path, source_files: list[str]) -> dict[str, str]:
    data = _load_toml(path)
    _validate_common_metadata(data, path)
    rules = _require_non_empty_list(data.get("rules"), "rules", path)

    parsed_rules: list[tuple[str, str]] = []
    for idx, rule in enumerate(rules):
        if not isinstance(rule, dict):
            raise ValueError(f"{path}: rules[{idx}] must be a table")
        prefix = _require_non_empty_str(rule.get("prefix"), "prefix", path)
        classification = _require_non_empty_str(
            rule.get("classification"),
            "classification",
            path,
        )
        _validate_relative_prefix(prefix, path, idx)
        if classification not in ALLOWED_CLASSIFICATIONS:
            raise ValueError(
                f"{path}: rules[{idx}].classification must be one of {sorted(ALLOWED_CLASSIFICATIONS)}"
            )
        parsed_rules.append((prefix, classification))

    used_rule_keys: set[tuple[str, str]] = set()
    classification_by_file: dict[str, str] = {}

    for source_file in source_files:
        matches = [rule for rule in parsed_rules if source_file.startswith(rule[0])]
        if not matches:
            raise ValueError(f"{path}: no classification rule matches source file '{source_file}'")

        longest = max(len(prefix) for prefix, _classification in matches)
        longest_matches = [rule for rule in matches if len(rule[0]) == longest]
        if len(longest_matches) > 1:
            prefixes = [prefix for prefix, _classification in longest_matches]
            raise ValueError(
                f"{path}: ambiguous classification for '{source_file}' from equally-specific prefixes {prefixes}"
            )

        selected = longest_matches[0]
        used_rule_keys.add(selected)
        classification_by_file[source_file] = selected[1]

    for rule in parsed_rules:
        if rule not in used_rule_keys:
            raise ValueError(f"{path}: rule prefix '{rule[0]}' does not match any source file")

    return classification_by_file


def _strip_rust_comments(text: str) -> str:
    """Remove Rust comments while preserving line structure for diagnostics."""
    without_blocks = re.sub(
        r"/\*.*?\*/",
        lambda m: "\n" * m.group(0).count("\n"),
        text,
        flags=re.S,
    )
    out_lines: list[str] = []
    for line in without_blocks.splitlines():
        comment_idx = line.find("//")
        out_lines.append(line if comment_idx == -1 else line[:comment_idx])
    return "\n".join(out_lines)


def _collect_fn_signatures(lines: list[str]) -> list[tuple[int, str]]:
    signatures: list[tuple[int, str]] = []
    idx = 0
    while idx < len(lines):
        line = lines[idx]
        if re.match(r"^\s*(?:pub(?:\([^)]*\))?\s+)?fn\b", line):
            start_line = idx + 1
            signature = line.strip()
            while "{" not in signature and ";" not in signature and idx + 1 < len(lines):
                idx += 1
                signature = f"{signature} {lines[idx].strip()}"
            signatures.append((start_line, signature))
        idx += 1
    return signatures


def _validate_core_struct_bans(path: Path, lines: list[str]) -> None:
    idx = 0
    while idx < len(lines):
        line = lines[idx]
        struct_match = re.match(
            r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)\b",
            line,
        )
        if not struct_match:
            idx += 1
            continue

        struct_name = struct_match.group(1)
        struct_line = idx + 1
        declaration = line.strip()
        while "{" not in declaration and ";" not in declaration and idx + 1 < len(lines):
            idx += 1
            declaration = f"{declaration} {lines[idx].strip()}"

        if "(" in declaration and "{" not in declaration:
            tuple_body = declaration.split("(", 1)[1].rsplit(")", 1)[0]
            if re.search(r"\bOption\s*<", tuple_body):
                raise ValueError(
                    f"{path}: Core struct '{struct_name}' at line {struct_line} uses Option< in fields"
                )
            if re.search(r"\b(?:std::primitive::)?bool\b", tuple_body):
                raise ValueError(
                    f"{path}: Core struct '{struct_name}' at line {struct_line} uses bool field(s) "
                    "for lifecycle encoding"
                )
            idx += 1
            continue

        if "{" not in declaration:
            idx += 1
            continue

        brace_depth = declaration.count("{") - declaration.count("}")
        idx += 1
        while idx < len(lines) and brace_depth > 0:
            body_line = lines[idx]
            if brace_depth == 1:
                field_match = re.match(
                    r"^\s*(?:pub(?:\([^)]*\))?\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([^,]+),?\s*$",
                    body_line,
                )
                if field_match:
                    field_name = field_match.group(1)
                    field_type = field_match.group(2).strip()
                    if re.search(r"\bOption\s*<", field_type):
                        raise ValueError(
                            f"{path}: Core struct '{struct_name}' field '{field_name}' at line {idx + 1} "
                            "uses Option<"
                        )
                    if re.fullmatch(r"(?:std::primitive::)?bool", field_type):
                        raise ValueError(
                            f"{path}: Core struct '{struct_name}' field '{field_name}' at line {idx + 1} "
                            "uses bool lifecycle encoding"
                        )
            brace_depth += body_line.count("{") - body_line.count("}")
            idx += 1


def _looks_like_enum_type(field_type: str) -> bool:
    cleaned = re.sub(r"\s+", "", field_type)
    if cleaned.startswith(("&", "Box<", "Vec<", "Option<", "Result<")):
        return False
    return re.search(r"(?:^|::)[A-Z][A-Za-z0-9_]*", cleaned) is not None


def _validate_sum_type_with_parallel_bool(path: Path, lines: list[str]) -> None:
    idx = 0
    while idx < len(lines):
        line = lines[idx]
        struct_match = re.match(
            r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)\b",
            line,
        )
        if not struct_match:
            idx += 1
            continue

        struct_name = struct_match.group(1)
        declaration = line.strip()
        while "{" not in declaration and ";" not in declaration and idx + 1 < len(lines):
            idx += 1
            declaration = f"{declaration} {lines[idx].strip()}"

        if "(" in declaration and "{" not in declaration:
            idx += 1
            continue

        if "{" not in declaration:
            idx += 1
            continue

        has_bool_field = False
        has_enum_like_field = False
        brace_depth = declaration.count("{") - declaration.count("}")
        idx += 1
        while idx < len(lines) and brace_depth > 0:
            body_line = lines[idx]
            if brace_depth == 1:
                field_match = re.match(
                    r"^\s*(?:pub(?:\([^)]*\))?\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([^,]+),?\s*$",
                    body_line,
                )
                if field_match:
                    field_type = field_match.group(2).strip()
                    if re.fullmatch(r"(?:std::primitive::)?bool", field_type):
                        has_bool_field = True
                    elif _looks_like_enum_type(field_type):
                        has_enum_like_field = True

            brace_depth += body_line.count("{") - body_line.count("}")
            idx += 1

        if has_bool_field and has_enum_like_field:
            raise ValueError(
                f"{path}: Core struct '{struct_name}' uses sum-type-with-parallel-bool; "
                "encode lifecycle in enum variants only"
            )


def _validate_core_trait_option_duration_returns(path: Path, lines: list[str]) -> None:
    idx = 0
    while idx < len(lines):
        line = lines[idx]
        trait_match = re.match(
            r"^\s*(?:pub(?:\([^)]*\))?\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)\b",
            line,
        )
        if not trait_match:
            idx += 1
            continue

        trait_name = trait_match.group(1)
        declaration = line.strip()
        while "{" not in declaration and idx + 1 < len(lines):
            idx += 1
            declaration = f"{declaration} {lines[idx].strip()}"

        if "{" not in declaration:
            idx += 1
            continue

        brace_depth = declaration.count("{") - declaration.count("}")
        idx += 1
        while idx < len(lines) and brace_depth > 0:
            body_line = lines[idx]
            if brace_depth == 1 and re.match(r"^\s*(?:pub(?:\([^)]*\))?\s+)?fn\b", body_line):
                signature_start_line = idx + 1
                signature = body_line.strip()
                while "{" not in signature and ";" not in signature and idx + 1 < len(lines):
                    idx += 1
                    signature = f"{signature} {lines[idx].strip()}"
                if re.search(r"->\s*Option\s*<\s*(?:std::time::)?Duration\s*>", signature):
                    raise ValueError(
                        f"{path}: Core trait '{trait_name}' function at line {signature_start_line} "
                        "returns Option<Duration>; use ExecutionBudget"
                    )

            brace_depth += body_line.count("{") - body_line.count("}")
            idx += 1


def _validate_core_enum_variant_bans(path: Path, lines: list[str]) -> None:
    idx = 0
    while idx < len(lines):
        line = lines[idx]
        enum_match = re.match(
            r"^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)\b",
            line,
        )
        if not enum_match:
            idx += 1
            continue

        enum_name = enum_match.group(1)
        declaration = line.strip()
        while "{" not in declaration and idx + 1 < len(lines):
            idx += 1
            declaration = f"{declaration} {lines[idx].strip()}"

        if "{" not in declaration:
            idx += 1
            continue

        brace_depth = declaration.count("{") - declaration.count("}")
        idx += 1
        while idx < len(lines) and brace_depth > 0:
            body_line = lines[idx]
            if brace_depth == 1:
                stripped = body_line.strip()
                if stripped and not stripped.startswith("#"):
                    variant_match = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)\b", stripped)
                    if variant_match:
                        variant_name = variant_match.group(1)
                        if variant_name in BANNED_CORE_ENUM_VARIANTS:
                            raise ValueError(
                                f"{path}: Core enum '{enum_name}' at line {idx + 1} uses banned "
                                f"variant '{variant_name}'"
                            )
            brace_depth += body_line.count("{") - body_line.count("}")
            idx += 1


def validate_core_bans(classification_by_file: dict[str, str], source_cache: dict[Path, str]) -> None:
    core_files = sorted(path for path, classification in classification_by_file.items() if classification == "core")
    for rel_path in core_files:
        source_path = ROOT / rel_path
        source = source_cache.get(source_path)
        if source is None:
            source = source_path.read_text(encoding="utf-8")
            source_cache[source_path] = source
        stripped = _strip_rust_comments(source)
        lines = stripped.splitlines()

        _validate_sum_type_with_parallel_bool(source_path, lines)
        _validate_core_trait_option_duration_returns(source_path, lines)

        for line_number, signature in _collect_fn_signatures(lines):
            if re.search(r"\bOption\s*<", signature):
                raise ValueError(
                    f"{source_path}: Core function signature at line {line_number} uses Option<"
                )

        _validate_core_struct_bans(source_path, lines)
        _validate_core_enum_variant_bans(source_path, lines)


def validate_engine_warning_emission_bools(source_files: list[str], source_cache: dict[Path, str]) -> None:
    warning_field_pattern = re.compile(
        r"^\s*(?:pub(?:\([^)]*\))?\s+)?([A-Za-z_][A-Za-z0-9_]*(?:_warning_shown|_shown))\s*:\s*(?:std::primitive::)?bool\b"
    )
    for rel_path in source_files:
        if not rel_path.startswith("engine/src/"):
            continue
        source_path = ROOT / rel_path
        source = source_cache.get(source_path)
        if source is None:
            source = source_path.read_text(encoding="utf-8")
            source_cache[source_path] = source
        stripped = _strip_rust_comments(source)
        lines = stripped.splitlines()
        for idx, line in enumerate(lines):
            match = warning_field_pattern.match(line)
            if match:
                field_name = match.group(1)
                raise ValueError(
                    f"{source_path}: warning-emission bool field '{field_name}' at line {idx + 1} "
                    "is banned; dedupe in boundary notification layer"
                )


def _validate_common_metadata(data: dict, path: Path) -> None:
    version = data.get("version")
    if not isinstance(version, int) or version <= 0:
        raise ValueError(f"{path}: 'version' must be a positive integer")


def _split_path(value: str, field_name: str, path: Path) -> list[str]:
    segments = value.split("::")
    if not segments or any(not segment.strip() for segment in segments):
        raise ValueError(f"{path}: field '{field_name}' must be a valid Rust path (got '{value}')")
    return segments


def _module_candidates(crate: str, module_segments: list[str]) -> list[Path]:
    src = ROOT / crate / "src"
    cleaned = [segment for segment in module_segments if segment != "mod"]
    if not cleaned:
        return [src / "lib.rs", src / "main.rs"]
    base = src.joinpath(*cleaned)
    return [base.with_suffix(".rs"), base / "mod.rs"]


def _leading_module_segments(segments: list[str]) -> list[str]:
    module_segments: list[str] = []
    for segment in segments:
        if segment == "mod":
            module_segments.append(segment)
            continue
        if segment[:1].islower():
            module_segments.append(segment)
            continue
        break
    return module_segments


def _symbol_pattern(symbol_segment: str) -> re.Pattern[str]:
    if symbol_segment == "*":
        raise ValueError("wildcard symbol '*' is not valid without a named target")
    if "*" in symbol_segment:
        prefix = symbol_segment.split("*", 1)[0]
        if not prefix:
            raise ValueError(f"invalid wildcard symbol segment '{symbol_segment}'")
        return re.compile(rf"\b{re.escape(prefix)}[A-Za-z0-9_]*\b")
    return re.compile(rf"\b{re.escape(symbol_segment)}\b")


def _require_workspace_crate(crate: str, workspace_members: list[str], path: Path, field_name: str) -> None:
    if crate not in workspace_members:
        raise ValueError(
            f"{path}: field '{field_name}' references unknown workspace crate '{crate}'"
        )


def _validate_module_path(value: str, workspace_members: list[str], path: Path, field_name: str) -> None:
    segments = _split_path(value, field_name, path)
    crate = segments[0]
    _require_workspace_crate(crate, workspace_members, path, field_name)
    module_segments = segments[1:]
    candidates = _module_candidates(crate, module_segments)
    if not any(candidate.exists() for candidate in candidates):
        raise ValueError(
            f"{path}: field '{field_name}' references module path '{value}' with no matching source file"
        )


def _validate_symbol_path(
    value: str,
    workspace_members: list[str],
    crate_source_files: dict[str, list[Path]],
    source_cache: dict[Path, str],
    path: Path,
    field_name: str,
) -> None:
    segments = _split_path(value, field_name, path)
    if len(segments) < 2:
        raise ValueError(f"{path}: field '{field_name}' must include crate and symbol path (got '{value}')")

    crate = segments[0]
    _require_workspace_crate(crate, workspace_members, path, field_name)

    if segments[-1] == "*":
        if len(segments) < 3:
            raise ValueError(f"{path}: field '{field_name}' wildcard path '{value}' is missing symbol target")
        symbol_segment = segments[-2]
        pre_symbol = segments[1:-2]
    else:
        symbol_segment = segments[-1]
        pre_symbol = segments[1:-1]

    module_segments = _leading_module_segments(pre_symbol)
    module_candidates = [
        candidate for candidate in _module_candidates(crate, module_segments) if candidate.exists()
    ]
    if module_segments and not module_candidates:
        raise ValueError(
            f"{path}: field '{field_name}' references stale module path '{value}'"
        )

    search_files = module_candidates if module_candidates else crate_source_files[crate]
    pattern = _symbol_pattern(symbol_segment)
    for source_file in search_files:
        text = source_cache.get(source_file)
        if text is None:
            text = source_file.read_text(encoding="utf-8")
            source_cache[source_file] = text
        if pattern.search(text):
            return

    raise ValueError(
        f"{path}: field '{field_name}' references symbol path '{value}' but no matching symbol was found"
    )


def validate_invariant_registry(
    path: Path,
    workspace_members: list[str],
    crate_source_files: dict[str, list[Path]],
    source_cache: dict[Path, str],
) -> set[str]:
    data = _load_toml(path)
    _validate_common_metadata(data, path)
    entries = _require_non_empty_list(data.get("invariants"), "invariants", path)

    ids: set[str] = set()
    for idx, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise ValueError(f"{path}: invariants[{idx}] must be a table")
        inv_id = _require_non_empty_str(entry.get("id"), "id", path)
        _require_non_empty_str(entry.get("predicate"), "predicate", path)
        canonical = _require_non_empty_str(
            entry.get("canonical_proof_type_path"),
            "canonical_proof_type_path",
            path,
        )
        boundary_module = _require_non_empty_str(
            entry.get("authority_boundary_module_path"),
            "authority_boundary_module_path",
            path,
        )
        _validate_symbol_path(
            canonical,
            workspace_members,
            crate_source_files,
            source_cache,
            path,
            "canonical_proof_type_path",
        )
        _validate_module_path(
            boundary_module,
            workspace_members,
            path,
            "authority_boundary_module_path",
        )
        if inv_id in ids:
            raise ValueError(f"{path}: duplicate invariant id '{inv_id}'")
        ids.add(inv_id)
    return ids


def validate_authority_boundary_map(
    path: Path,
    workspace_members: list[str],
    crate_source_files: dict[str, list[Path]],
    source_cache: dict[Path, str],
) -> list[dict]:
    data = _load_toml(path)
    _validate_common_metadata(data, path)
    entries = _require_non_empty_list(data.get("entries"), "entries", path)

    for idx, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise ValueError(f"{path}: entries[{idx}] must be a table")
        controlled_type = _require_non_empty_str(
            entry.get("controlled_type_path"),
            "controlled_type_path",
            path,
        )
        boundary_module = _require_non_empty_str(
            entry.get("boundary_module_path"),
            "boundary_module_path",
            path,
        )
        constructor_paths = _require_non_empty_list(entry.get("constructor_paths"), "constructor_paths", path)
        allowed_callers = _require_non_empty_list(
            entry.get("allowed_caller_module_paths"),
            "allowed_caller_module_paths",
            path,
        )
        vis = _require_non_empty_str(
            entry.get("max_constructor_visibility_rung"),
            "max_constructor_visibility_rung",
            path,
        )
        if vis not in ALLOWED_VISIBILITY:
            raise ValueError(
                f"{path}: entries[{idx}].max_constructor_visibility_rung must be one of {sorted(ALLOWED_VISIBILITY)}"
            )
        _validate_symbol_path(
            controlled_type,
            workspace_members,
            crate_source_files,
            source_cache,
            path,
            "controlled_type_path",
        )
        _validate_module_path(boundary_module, workspace_members, path, "boundary_module_path")
        for constructor in constructor_paths:
            constructor_value = _require_non_empty_str(constructor, "constructor_paths[]", path)
            _validate_symbol_path(
                constructor_value,
                workspace_members,
                crate_source_files,
                source_cache,
                path,
                "constructor_paths[]",
            )
        for caller_module in allowed_callers:
            caller_value = _require_non_empty_str(
                caller_module,
                "allowed_caller_module_paths[]",
                path,
            )
            _validate_module_path(
                caller_value,
                workspace_members,
                path,
                "allowed_caller_module_paths[]",
            )

    return entries


def validate_controlled_struct_unforgeability(
    path: Path,
    entries: list[dict],
    crate_source_files: dict[str, list[Path]],
    source_cache: dict[Path, str],
) -> None:
    struct_decl_pattern = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)\b")
    public_field_pattern = re.compile(r"^\s*pub(?:\([^)]*\))?\s+[A-Za-z_][A-Za-z0-9_]*\s*:")

    for idx, entry in enumerate(entries):
        controlled_type = _require_non_empty_str(
            entry.get("controlled_type_path"),
            "controlled_type_path",
            path,
        )
        segments = _split_path(controlled_type, "controlled_type_path", path)
        crate = segments[0]
        type_name = segments[-1]
        if not type_name[:1].isupper():
            continue

        for source_file in crate_source_files[crate]:
            source = source_cache.get(source_file)
            if source is None:
                source = source_file.read_text(encoding="utf-8")
                source_cache[source_file] = source
            lines = _strip_rust_comments(source).splitlines()

            line_idx = 0
            while line_idx < len(lines):
                line = lines[line_idx]
                match = struct_decl_pattern.match(line)
                if not match or match.group(1) != type_name:
                    line_idx += 1
                    continue

                decl_line = line_idx + 1
                declaration = line.strip()
                while "{" not in declaration and ";" not in declaration and line_idx + 1 < len(lines):
                    line_idx += 1
                    declaration = f"{declaration} {lines[line_idx].strip()}"

                if "(" in declaration and "{" not in declaration:
                    tuple_body = declaration.split("(", 1)[1].rsplit(")", 1)[0]
                    if re.search(r"\bpub(?:\([^)]*\))?\b", tuple_body):
                        raise ValueError(
                            f"{path}: entries[{idx}] controlled type '{controlled_type}' has public tuple "
                            f"fields (line {decl_line})"
                        )
                    break

                if "{" not in declaration:
                    break

                brace_depth = declaration.count("{") - declaration.count("}")
                line_idx += 1
                while line_idx < len(lines) and brace_depth > 0:
                    body_line = lines[line_idx]
                    if brace_depth == 1 and public_field_pattern.match(body_line):
                        raise ValueError(
                            f"{path}: entries[{idx}] controlled type '{controlled_type}' exposes public "
                            f"named fields (line {line_idx + 1})"
                        )
                    brace_depth += body_line.count("{") - body_line.count("}")
                    line_idx += 1
                break

            # Enum-controlled types have no field visibility to inspect here.


def _extract_visibility(vis_prefix: str | None) -> str:
    """Map a Rust visibility modifier prefix to a rung string."""
    if not vis_prefix or not vis_prefix.strip():
        return "private"
    trimmed = vis_prefix.strip()
    if trimmed == "pub":
        return "pub"
    m = re.match(r"pub\(([^)]+)\)", trimmed)
    if m:
        inner = m.group(1).strip()
        if inner == "crate":
            return "pub(crate)"
        if inner == "super":
            return "pub(super)"
        return "pub(crate)"
    return "private"


def _find_enum_visibility(
    lines: list[str], enum_name: str, source_file: Path
) -> tuple[str, Path, int] | None:
    pattern = re.compile(
        rf"^\s*(pub(?:\([^)]*\))?\s+)?enum\s+{re.escape(enum_name)}\b"
    )
    for idx, line in enumerate(lines):
        m = pattern.match(line)
        if m:
            return (_extract_visibility(m.group(1)), source_file, idx + 1)
    return None


def _find_inherent_method_visibility(
    lines: list[str], type_name: str, method_name: str, source_file: Path
) -> tuple[str, Path, int] | None:
    fn_pattern = re.compile(
        rf"^\s*(pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+{re.escape(method_name)}\b"
    )
    idx = 0
    while idx < len(lines):
        line = lines[idx]
        if re.match(r"^\s*impl\b", line):
            header = line.strip()
            while "{" not in header and idx + 1 < len(lines):
                idx += 1
                header = f"{header} {lines[idx].strip()}"

            is_inherent = (
                re.match(
                    rf"impl(?:\s*<[^>]*>)?\s+{re.escape(type_name)}\s*(?:<[^>]*>)?\s*\{{",
                    header,
                )
                and " for " not in header
            )

            if is_inherent:
                brace_depth = header.count("{") - header.count("}")
                idx += 1
                while idx < len(lines) and brace_depth > 0:
                    body_line = lines[idx]
                    if brace_depth == 1:
                        fn_match = fn_pattern.match(body_line)
                        if fn_match:
                            return (
                                _extract_visibility(fn_match.group(1)),
                                source_file,
                                idx + 1,
                            )
                    brace_depth += body_line.count("{") - body_line.count("}")
                    idx += 1
                continue
        idx += 1
    return None


def _find_trait_impl_method(
    lines: list[str], type_name: str, method_name: str, source_file: Path
) -> tuple[str, Path, int] | None:
    fn_pattern = re.compile(
        rf"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+{re.escape(method_name)}\b"
    )
    idx = 0
    while idx < len(lines):
        line = lines[idx]
        if re.match(r"^\s*impl\b", line):
            header = line.strip()
            while "{" not in header and idx + 1 < len(lines):
                idx += 1
                header = f"{header} {lines[idx].strip()}"

            is_trait_impl = " for " in header and re.search(
                rf"\bfor\s+{re.escape(type_name)}\b", header
            )

            if is_trait_impl:
                brace_depth = header.count("{") - header.count("}")
                idx += 1
                while idx < len(lines) and brace_depth > 0:
                    body_line = lines[idx]
                    if brace_depth == 1:
                        fn_match = fn_pattern.match(body_line)
                        if fn_match:
                            return ("pub", source_file, idx + 1)
                    brace_depth += body_line.count("{") - body_line.count("}")
                    idx += 1
                continue
        idx += 1
    return None


def _resolve_constructor_visibility(
    constructor_path: str,
    crate_source_files: dict[str, list[Path]],
    source_cache: dict[Path, str],
    toml_path: Path,
) -> tuple[str, Path, int] | None:
    """Resolve the actual visibility rung of a constructor symbol.

    Returns ``(visibility, source_file, line)`` or ``None`` if the symbol
    cannot be resolved (already caught by symbol-path validation).
    """
    segments = _split_path(constructor_path, "constructor_paths[]", toml_path)
    crate = segments[0]

    is_wildcard = segments[-1] == "*"
    if is_wildcard:
        type_name = segments[-2]
        module_segments = _leading_module_segments(segments[1:-2])
    else:
        symbol_name = segments[-1]
        type_name = segments[-2] if len(segments) >= 3 else None
        pre_symbol = segments[1:-1] if type_name else segments[1:]
        module_segments = _leading_module_segments(pre_symbol)

    module_candidates = [
        c for c in _module_candidates(crate, module_segments) if c.exists()
    ]
    search_files = module_candidates if module_candidates else crate_source_files.get(crate, [])

    for source_file in search_files:
        text = source_cache.get(source_file)
        if text is None:
            text = source_file.read_text(encoding="utf-8")
            source_cache[source_file] = text
        lines = _strip_rust_comments(text).splitlines()

        if is_wildcard or (not is_wildcard and symbol_name[:1].isupper()):
            result = _find_enum_visibility(lines, type_name or symbol_name, source_file)
            if result:
                return result

        if not is_wildcard and type_name:
            result = _find_inherent_method_visibility(lines, type_name, symbol_name, source_file)
            if result:
                return result
            result = _find_trait_impl_method(lines, type_name, symbol_name, source_file)
            if result:
                return result

    return None


def validate_constructor_visibility_rungs(
    path: Path,
    entries: list[dict],
    crate_source_files: dict[str, list[Path]],
    source_cache: dict[Path, str],
) -> None:
    """Validate that each constructor's actual visibility does not exceed
    the declared ``max_constructor_visibility_rung``."""
    for idx, entry in enumerate(entries):
        max_rung = _require_non_empty_str(
            entry.get("max_constructor_visibility_rung"),
            "max_constructor_visibility_rung",
            path,
        )
        max_rung_val = VISIBILITY_RUNG_ORDER.get(max_rung)
        if max_rung_val is None:
            continue

        constructor_paths = _require_non_empty_list(
            entry.get("constructor_paths"), "constructor_paths", path
        )
        controlled_type = entry.get("controlled_type_path", "<unknown>")

        for constructor in constructor_paths:
            constructor_value = _require_non_empty_str(
                constructor, "constructor_paths[]", path
            )
            result = _resolve_constructor_visibility(
                constructor_value, crate_source_files, source_cache, path
            )
            if result is None:
                continue

            actual_vis, source_file, line_number = result
            actual_val = VISIBILITY_RUNG_ORDER.get(actual_vis, -1)
            if actual_val > max_rung_val:
                raise ValueError(
                    f"{path}: entries[{idx}] ({controlled_type}) constructor "
                    f"'{constructor_value}' has visibility '{actual_vis}' which "
                    f"exceeds max_constructor_visibility_rung '{max_rung}' "
                    f"(at {source_file}:{line_number})"
                )


def validate_parametricity_rules(path: Path) -> None:
    data = _load_toml(path)
    _validate_common_metadata(data, path)
    _require_non_empty_list(data.get("banned_patterns"), "banned_patterns", path)
    _require_non_empty_list(data.get("required_interface_disclosures"), "required_interface_disclosures", path)


def validate_move_semantics_rules(
    path: Path,
    workspace_members: list[str],
    crate_source_files: dict[str, list[Path]],
    source_cache: dict[Path, str],
) -> None:
    data = _load_toml(path)
    _validate_common_metadata(data, path)
    entries = _require_non_empty_list(data.get("state_bearing_types"), "state_bearing_types", path)
    for idx, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise ValueError(f"{path}: state_bearing_types[{idx}] must be a table")
        type_path = _require_non_empty_str(entry.get("type_path"), "type_path", path)
        _validate_symbol_path(
            type_path,
            workspace_members,
            crate_source_files,
            source_cache,
            path,
            "type_path",
        )
        transitions = _require_non_empty_list(entry.get("consumed_transition_methods"), "consumed_transition_methods", path)
        for t_idx, transition in enumerate(transitions):
            if not isinstance(transition, dict):
                raise ValueError(
                    f"{path}: state_bearing_types[{idx}].consumed_transition_methods[{t_idx}] must be a table"
                )
            method_path = _require_non_empty_str(transition.get("method_path"), "method_path", path)
            _validate_symbol_path(
                method_path,
                workspace_members,
                crate_source_files,
                source_cache,
                path,
                "method_path",
            )
            consumes_self = transition.get("consumes_self")
            if not isinstance(consumes_self, bool) or not consumes_self:
                raise ValueError(f"{path}: transition '{transition}' must set consumes_self=true")
            _require_non_empty_str(transition.get("post_move_unusability_guarantee"), "post_move_unusability_guarantee", path)


def validate_dry_proof_map(
    path: Path,
    known_ids: set[str],
    workspace_members: list[str],
    crate_source_files: dict[str, list[Path]],
    source_cache: dict[Path, str],
) -> None:
    data = _load_toml(path)
    _validate_common_metadata(data, path)
    entries = _require_non_empty_list(data.get("entries"), "entries", path)
    seen_ids: set[str] = set()
    for idx, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise ValueError(f"{path}: entries[{idx}] must be a table")
        inv_id = _require_non_empty_str(entry.get("invariant_id"), "invariant_id", path)
        canonical = _require_non_empty_str(
            entry.get("canonical_proof_type_path"),
            "canonical_proof_type_path",
            path,
        )
        boundary_module = _require_non_empty_str(
            entry.get("authority_boundary_module_path"),
            "authority_boundary_module_path",
            path,
        )
        _validate_symbol_path(
            canonical,
            workspace_members,
            crate_source_files,
            source_cache,
            path,
            "canonical_proof_type_path",
        )
        _validate_module_path(
            boundary_module,
            workspace_members,
            path,
            "authority_boundary_module_path",
        )
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
        workspace_members = _workspace_members()
        validate_workspace_readmes(workspace_members)
        source_files = _all_source_files(workspace_members)
        crate_source_files = {crate: _crate_source_files(crate) for crate in workspace_members}
        source_cache: dict[Path, str] = {}

        classifications = validate_classification_map(CLASSIFICATION_MAP, source_files)
        validate_core_bans(classifications, source_cache)
        validate_engine_warning_emission_bools(source_files, source_cache)
        invariant_ids = validate_invariant_registry(
            INVARIANT_REGISTRY,
            workspace_members,
            crate_source_files,
            source_cache,
        )
        authority_entries = validate_authority_boundary_map(
            AUTHORITY_BOUNDARY_MAP,
            workspace_members,
            crate_source_files,
            source_cache,
        )
        validate_controlled_struct_unforgeability(
            AUTHORITY_BOUNDARY_MAP,
            authority_entries,
            crate_source_files,
            source_cache,
        )
        validate_constructor_visibility_rungs(
            AUTHORITY_BOUNDARY_MAP,
            authority_entries,
            crate_source_files,
            source_cache,
        )
        validate_parametricity_rules(PARAMETRICITY_RULES)
        validate_move_semantics_rules(
            MOVE_SEMANTICS_RULES,
            workspace_members,
            crate_source_files,
            source_cache,
        )
        validate_dry_proof_map(
            DRY_PROOF_MAP,
            invariant_ids,
            workspace_members,
            crate_source_files,
            source_cache,
        )
    except ValueError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1

    print("IFA artifact check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
