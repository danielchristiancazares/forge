#!/usr/bin/env python3
"""Generate CONTEXT.md for inclusion in source zips.

Produces a single file containing:
1. File tree with crate boundaries
2. Major state transitions index
3. Cargo metadata (workspace graph, resolve, dependency edges)
4. Enabled features per crate

Usage:
    cargo metadata --format-version 1 --no-deps | python scripts/gen_context.py > CONTEXT.md

Or with pre-saved JSON:
    python scripts/gen_context.py < cargo_metadata.json > CONTEXT.md
"""

import json
import os
import sys
from pathlib import Path

# Force UTF-8 output on Windows
if sys.stdout.encoding != "utf-8":
    sys.stdout.reconfigure(encoding="utf-8")


def load_metadata():
    data = json.load(sys.stdin)
    return data


# ── Section 1: File tree with crate boundaries ──────────────────────

IGNORE_DIRS = {
    "target", ".git", "node_modules", "coverage", "gemini-review",
    "__pycache__", ".claude",
}
IGNORE_FILES = {
    "lcov.info", "sha256.txt", "forge-source.zip",
}
CRATE_MARKERS = {"Cargo.toml"}


def build_file_tree(root: Path, prefix: str = "", max_depth: int = 4) -> list[str]:
    lines = []
    try:
        entries = sorted(root.iterdir(), key=lambda e: (not e.is_dir(), e.name.lower()))
    except PermissionError:
        return lines

    dirs = [e for e in entries if e.is_dir() and e.name not in IGNORE_DIRS and not e.name.startswith(".")]
    files = [e for e in entries if e.is_file() and e.name not in IGNORE_FILES and not e.name.startswith(".")]

    items = dirs + files
    for i, entry in enumerate(items):
        connector = "└── " if i == len(items) - 1 else "├── "
        extension = "    " if i == len(items) - 1 else "│   "

        if entry.is_dir():
            # Mark crate boundaries
            is_crate = (entry / "Cargo.toml").exists()
            marker = " [crate]" if is_crate else ""
            lines.append(f"{prefix}{connector}{entry.name}/{marker}")
            if max_depth > 1:
                lines.extend(build_file_tree(entry, prefix + extension, max_depth - 1))
        else:
            lines.append(f"{prefix}{connector}{entry.name}")

    return lines


# ── Section 2: State transitions index ───────────────────────────────

import re

# Enum names containing these substrings are considered "state-related"
STATE_KEYWORDS = {"State", "Mode", "Phase", "Kind", "Status", "Level"}

ENUM_RE = re.compile(r"enum\s+(\w+)")


def extract_enums_from_file(path: Path) -> list[tuple[str, list[str]]]:
    """Extract all enum { Name, ... } blocks from a Rust source file."""
    content = path.read_text(encoding="utf-8", errors="replace")
    results = []
    in_enum = False
    brace_depth = 0
    enum_name = ""
    variants: list[str] = []

    for line in content.splitlines():
        stripped = line.strip()

        if not in_enum:
            m = ENUM_RE.search(stripped)
            if m and "{" in stripped:
                enum_name = m.group(1)
                in_enum = True
                brace_depth = stripped.count("{") - stripped.count("}")
                variants = []
                # Check for single-line variants on the same line after {
                after_brace = stripped.split("{", 1)[1] if "{" in stripped else ""
                for part in after_brace.split(","):
                    part = part.strip().rstrip("}").strip()
                    if part and part[0].isupper() and not part.startswith("//"):
                        variant = part.split("(")[0].split("{")[0].strip()
                        if variant:
                            variants.append(variant)
                if brace_depth <= 0:
                    if variants:
                        results.append((enum_name, variants))
                    in_enum = False
                continue
            elif m:
                # enum declaration without opening brace on same line
                enum_name = m.group(1)
                in_enum = True
                brace_depth = 0
                variants = []
                continue

        if in_enum:
            brace_depth += stripped.count("{") - stripped.count("}")

            if brace_depth <= 0:
                # closing brace
                if variants:
                    results.append((enum_name, variants))
                in_enum = False
                continue

            # Skip comments, attributes, inner struct fields
            if stripped.startswith("//") or stripped.startswith("#") or stripped.startswith("*"):
                continue
            # Skip inner struct fields (lines with colons that aren't variant declarations)
            if ":" in stripped and not stripped.endswith(",") or stripped.startswith("pub "):
                # Could be a struct variant field like `name: Type,`
                # Only count as variant if at brace_depth == 1
                if brace_depth > 1:
                    continue

            # Extract variant: take word before ( or { or ,
            variant = stripped.split("(")[0].split("{")[0].split(",")[0].strip()
            if variant and variant[0].isupper() and variant.isidentifier():
                variants.append(variant)

    if in_enum and variants:
        results.append((enum_name, variants))

    return results


def extract_state_transitions(root: Path) -> list[str]:
    """Scan engine crate for state-related enums and list their variants."""
    lines = []
    engine_src = root / "engine" / "src"
    if not engine_src.exists():
        return lines

    found: list[tuple[str, str, list[str]]] = []  # (file, enum_name, variants)

    for rs_file in sorted(engine_src.rglob("*.rs")):
        rel = rs_file.relative_to(root).as_posix()
        for enum_name, variants in extract_enums_from_file(rs_file):
            if any(kw in enum_name for kw in STATE_KEYWORDS):
                found.append((rel, enum_name, variants))

    for rel, enum_name, variants in found:
        joiner = " | " if "Kind" in enum_name else " -> "
        lines.append(f"  {enum_name}: {joiner.join(variants)}  ({rel})")

    return lines


# ── Section 3 & 4: Cargo metadata + features ────────────────────────

def format_metadata(data: dict) -> tuple[list[str], list[str]]:
    """Returns (metadata_lines, features_lines)."""
    meta_lines = []
    feat_lines = []

    packages = sorted(data.get("packages", []), key=lambda p: p["name"])

    # Workspace members (strip local paths, show crate@version)
    members = data.get("workspace_members", [])
    meta_lines.append("Workspace members:")
    clean_members = []
    for m in sorted(members):
        # Cargo format: "path+file:///abs/path#name@version"
        if "#" in m:
            clean_members.append(m.split("#", 1)[1])
        else:
            clean_members.append(m)
    for m in sorted(clean_members):
        meta_lines.append(f"  {m}")
    meta_lines.append("")

    # Dependency edges (workspace-internal only)
    pkg_names = {p["name"] for p in packages}
    meta_lines.append("Crate dependency graph (workspace-internal):")
    for pkg in packages:
        deps = [d["name"] for d in pkg.get("dependencies", []) if d["name"] in pkg_names]
        if deps:
            meta_lines.append(f"  {pkg['name']} -> {', '.join(sorted(deps))}")
        else:
            meta_lines.append(f"  {pkg['name']} (leaf)")
    meta_lines.append("")

    # External dependencies (name + version + features, grouped by crate)
    meta_lines.append("External dependencies per crate:")
    for pkg in packages:
        ext_deps = []
        for d in pkg.get("dependencies", []):
            if d["name"] in pkg_names:
                continue
            feats = d.get("features", [])
            label = f"{d['name']} {d.get('req', '')}"
            if feats:
                label += f" [{', '.join(sorted(feats))}]"
            ext_deps.append(label)
        ext_deps = sorted(set(ext_deps), key=str.lower)
        if ext_deps:
            meta_lines.append(f"  {pkg['name']}:")
            for dep in ext_deps:
                meta_lines.append(f"    {dep}")
    meta_lines.append("")

    # Dependency feature flags activated per crate
    feat_lines.append("Dependency feature flags per crate:")
    for pkg in packages:
        dep_feats: list[str] = []
        for d in pkg.get("dependencies", []):
            feats = d.get("features", [])
            if feats:
                dep_feats.append(f"    {d['name']}: {', '.join(sorted(feats))}")
        # Also show crate-declared features if any
        own_features = pkg.get("features", {})
        if dep_feats or own_features:
            feat_lines.append(f"  {pkg['name']}:")
            if own_features:
                for fname, fdeps in sorted(own_features.items()):
                    if fdeps:
                        feat_lines.append(f"    [own] {fname} = [{', '.join(fdeps)}]")
                    else:
                        feat_lines.append(f"    [own] {fname}")
            for line in sorted(dep_feats, key=str.lower):
                feat_lines.append(line)

    return meta_lines, feat_lines


# ── Main ─────────────────────────────────────────────────────────────

def main():
    data = load_metadata()

    # Determine project root (from workspace_root in metadata)
    root = Path(data.get("workspace_root", "."))

    out = []
    out.append("# Forge Project Context")
    out.append("")
    out.append("Auto-generated pre-flight context for LLM consumption.")
    out.append("")

    # Section 1: File tree
    out.append("## 1. File Tree (Crate Boundaries)")
    out.append("")
    out.append("```")
    out.append(f"{root.name}/")
    out.extend(build_file_tree(root))
    out.append("```")
    out.append("")

    # Section 2: State transitions
    out.append("## 2. Major State Transitions")
    out.append("")
    transitions = extract_state_transitions(root)
    if transitions:
        out.append("```")
        out.extend(transitions)
        out.append("```")
    else:
        out.append("(no state enums found)")
    out.append("")

    # Sections 3 & 4: Metadata + features
    meta_lines, feat_lines = format_metadata(data)

    out.append("## 3. Cargo Metadata")
    out.append("")
    out.append("```")
    out.extend(meta_lines)
    out.append("```")
    out.append("")

    out.append("## 4. Enabled Features")
    out.append("")
    out.append("```")
    out.extend(feat_lines)
    out.append("```")
    out.append("")

    print("\n".join(out))


if __name__ == "__main__":
    main()
