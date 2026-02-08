#!/usr/bin/env python3
"""Convert rustdoc JSON output into a flat DIGEST.md reference.

Usage:
    # Generate JSON (requires nightly):
    cargo +nightly rustdoc -p <crate> -- -Z unstable-options --output-format json
    
    # Then run this script:
    python scripts/rustdoc_digest.py target/doc/*.json > DIGEST.md
"""

import json
import sys
from pathlib import Path
from collections import defaultdict


class RustdocDigest:
    def __init__(self, data: dict):
        self.data = data
        self.index = data["index"]
        self.paths = data["paths"]
        self.root_id = str(data["root"])
        self.crate_name = self._get_item(self.root_id)["name"]

    def _get_item(self, item_id) -> dict | None:
        return self.index.get(str(item_id))

    # ── Type rendering ──────────────────────────────────────────────

    def _render_type(self, ty: dict | None) -> str:
        if ty is None:
            return "()"

        if "primitive" in ty:
            return ty["primitive"]
        if "generic" in ty:
            return ty["generic"]
        if "resolved_path" in ty:
            rp = ty["resolved_path"]
            name = rp["path"]
            args = self._render_generic_args(rp.get("args"))
            return f"{name}{args}"
        if "qualified_path" in ty:
            qp = ty["qualified_path"]
            self_type = self._render_type(qp.get("self_type"))
            trait_name = qp.get("trait", {}).get("path", "")
            assoc = qp.get("name", "")
            if trait_name:
                return f"<{self_type} as {trait_name}>::{assoc}"
            return f"{self_type}::{assoc}"
        if "borrowed_ref" in ty:
            br = ty["borrowed_ref"]
            lt = f"{br['lifetime']} " if br.get("lifetime") else ""
            mut = "mut " if br.get("is_mutable") else ""
            inner = self._render_type(br["type"])
            return f"&{lt}{mut}{inner}"
        if "raw_pointer" in ty:
            rp = ty["raw_pointer"]
            mut = "mut" if rp.get("is_mutable") else "const"
            inner = self._render_type(rp["type"])
            return f"*{mut} {inner}"
        if "slice" in ty:
            inner = self._render_type(ty["slice"])
            return f"[{inner}]"
        if "array" in ty:
            arr = ty["array"]
            inner = self._render_type(arr["type"])
            length = arr.get("len", "_")
            return f"[{inner}; {length}]"
        if "tuple" in ty:
            parts = [self._render_type(t) for t in ty["tuple"]]
            return f"({', '.join(parts)})"
        if "impl_trait" in ty:
            bounds = []
            for bound in ty["impl_trait"]:
                if "trait_bound" in bound:
                    tb = bound["trait_bound"]["trait"]
                    name = tb["path"]
                    args = self._render_generic_args(tb.get("args"))
                    bounds.append(f"{name}{args}")
            return f"impl {' + '.join(bounds)}"
        if "dyn_trait" in ty:
            bounds = []
            for bound in ty["dyn_trait"].get("traits", []):
                tb = bound.get("trait", {})
                name = tb.get("path", "?")
                args = self._render_generic_args(tb.get("args"))
                bounds.append(f"{name}{args}")
            lt = ty["dyn_trait"].get("lifetime")
            lt_str = f" + {lt}" if lt else ""
            return f"dyn {' + '.join(bounds)}{lt_str}"
        if "function_pointer" in ty:
            return "fn(...)"
        if "infer" in ty:
            return "_"

        return "?"

    def _render_generic_args(self, args) -> str:
        if not args:
            return ""
        if "angle_bracketed" in args:
            ab = args["angle_bracketed"]
            parts = []
            for arg in ab.get("args", []):
                if "type" in arg:
                    parts.append(self._render_type(arg["type"]))
                elif "lifetime" in arg:
                    parts.append(arg["lifetime"])
                elif "const" in arg:
                    parts.append(str(arg["const"]))
            for c in ab.get("constraints", []):
                name = c.get("name", "")
                if "type" in c:
                    parts.append(f"{name} = {self._render_type(c['type'])}")
            if parts:
                return f"<{', '.join(parts)}>"
        if "parenthesized" in args:
            p = args["parenthesized"]
            inputs = ", ".join(self._render_type(t) for t in p.get("inputs", []))
            output = self._render_type(p.get("output"))
            ret = f" -> {output}" if output != "()" else ""
            return f"({inputs}){ret}"
        return ""

    def _render_generics(self, generics: dict) -> str:
        params = generics.get("params", [])
        if not params:
            return ""
        parts = []
        for p in params:
            name = p.get("name", "")
            kind = p.get("kind", {})
            if "type" in kind:
                bounds = kind["type"].get("bounds", [])
                bound_strs = []
                for b in bounds:
                    if "trait_bound" in b:
                        tb = b["trait_bound"]["trait"]
                        bound_strs.append(tb["path"] + self._render_generic_args(tb.get("args")))
                if bound_strs:
                    parts.append(f"{name}: {' + '.join(bound_strs)}")
                else:
                    parts.append(name)
            elif "lifetime" in kind:
                parts.append(name)
            elif "const" in kind:
                ty = self._render_type(kind["const"].get("type"))
                parts.append(f"const {name}: {ty}")
        return f"<{', '.join(parts)}>"

    def _render_where(self, generics: dict) -> str:
        preds = generics.get("where_predicates", [])
        if not preds:
            return ""
        parts = []
        for pred in preds:
            if "bound_predicate" in pred:
                bp = pred["bound_predicate"]
                ty = self._render_type(bp.get("type"))
                bounds = []
                for b in bp.get("bounds", []):
                    if "trait_bound" in b:
                        tb = b["trait_bound"]["trait"]
                        bounds.append(tb["path"] + self._render_generic_args(tb.get("args")))
                parts.append(f"{ty}: {' + '.join(bounds)}")
        if parts:
            return "\n    where " + ", ".join(parts)
        return ""

    # ── Signature rendering ─────────────────────────────────────────

    def _render_fn_sig(self, name: str, func: dict, is_method: bool = False) -> str:
        sig = func["sig"]
        header = func.get("header", {})
        generics = func.get("generics", {})

        quals = []
        if header.get("is_const"):
            quals.append("const ")
        if header.get("is_async"):
            quals.append("async ")
        if header.get("is_unsafe"):
            quals.append("unsafe ")
        qual_str = "".join(quals)

        gen_str = self._render_generics(generics)

        params = []
        for pname, pty in sig["inputs"]:
            if pname == "self":
                rendered = self._render_type(pty)
                if rendered == "&Self":
                    params.append("&self")
                elif rendered == "&mut Self":
                    params.append("&mut self")
                elif rendered == "Self":
                    params.append("self")
                else:
                    params.append(rendered)
            else:
                params.append(f"{pname}: {self._render_type(pty)}")
        params_str = ", ".join(params)

        output = sig.get("output")
        ret = ""
        if output:
            rendered = self._render_type(output)
            if rendered != "()":
                ret = f" -> {rendered}"

        where = self._render_where(generics)
        return f"{qual_str}fn {name}{gen_str}({params_str}){ret}{where}"

    # ── Item collection ─────────────────────────────────────────────

    def _collect_module_items(self, mod_id: str, path_prefix: str = "") -> dict:
        """Recursively collect items organized by kind."""
        result = {
            "structs": [],
            "enums": [],
            "traits": [],
            "functions": [],
            "type_aliases": [],
            "constants": [],
            "modules": [],
        }

        item = self._get_item(mod_id)
        if not item:
            return result

        inner = item.get("inner", {})
        mod_data = inner.get("module")
        if not mod_data:
            return result

        child_ids = mod_data.get("items", [])
        for child_id in child_ids:
            child = self._get_item(child_id)
            if not child:
                continue

            vis = child.get("visibility", "default")
            child_inner = child.get("inner", {})

            if "struct" in child_inner:
                result["structs"].append(child)
            elif "enum" in child_inner:
                result["enums"].append(child)
            elif "function" in child_inner:
                result["functions"].append(child)
            elif "module" in child_inner:
                result["modules"].append(child)
            elif "use" in child_inner:
                # Follow re-exports to their target items
                use_data = child_inner["use"]
                target_id = str(use_data.get("id", ""))
                target = self._get_item(target_id)
                if target:
                    target_inner = target.get("inner", {})
                    if "struct" in target_inner:
                        result["structs"].append(target)
                    elif "enum" in target_inner:
                        result["enums"].append(target)
                    elif "function" in target_inner:
                        result["functions"].append(target)
            # assoc_type, assoc_const handled inside impls

        return result

    def _get_impl_methods(self, impl_ids: list) -> tuple[list, list]:
        """Returns (inherent_methods, trait_impls_summary)."""
        methods = []
        traits = []
        for impl_id in impl_ids:
            impl_item = self._get_item(impl_id)
            if not impl_item:
                continue
            impl_data = impl_item.get("inner", {}).get("impl", {})
            if not impl_data:
                continue

            trait_info = impl_data.get("trait")

            # Skip auto-derived trait impls (Clone, Debug, etc.) and blanket impls
            if impl_data.get("is_synthetic") or impl_data.get("blanket_impl"):
                continue
            if trait_info:
                attrs = impl_item.get("attrs", [])
                is_derived = "automatically_derived" in attrs
                trait_name = trait_info.get("path", "")
                trait_args = self._render_generic_args(trait_info.get("args"))

                # Collect non-trivially-derived trait impls
                items = impl_data.get("items", [])
                if not is_derived and items:
                    trait_methods = []
                    for mid in items:
                        m = self._get_item(mid)
                        if m and "function" in m.get("inner", {}):
                            trait_methods.append(m)
                    if trait_methods:
                        traits.append((f"{trait_name}{trait_args}", trait_methods))
                elif not is_derived:
                    traits.append((f"{trait_name}{trait_args}", []))
                continue

            # Inherent impl
            for mid in impl_data.get("items", []):
                m = self._get_item(mid)
                if not m:
                    continue
                if m.get("visibility") != "public":
                    continue
                if "function" in m.get("inner", {}):
                    methods.append(m)

        return methods, traits

    # ── Markdown generation ─────────────────────────────────────────

    def _format_doc(self, docs: str | None, indent: str = "") -> str:
        if not docs:
            return ""
        # Take first paragraph only for digest brevity
        first_para = docs.split("\n\n")[0].strip()
        # Truncate very long docs
        if len(first_para) > 200:
            first_para = first_para[:197] + "..."
        lines = first_para.split("\n")
        return "\n".join(f"{indent}/// {line.strip()}" for line in lines)

    def _span_loc(self, item: dict) -> str:
        span = item.get("span")
        if not span:
            return ""
        filename = span["filename"].replace("\\", "/")
        line = span["begin"][0]
        return f" @ {filename}:{line}"

    def generate(self) -> str:
        lines = []
        root = self._get_item(self.root_id)
        if not root:
            return ""

        items = self._collect_module_items(self.root_id)

        # Submodules (recurse one level)
        for mod_item in sorted(items["modules"], key=lambda x: x["name"]):
            mod_name = mod_item["name"]
            mod_inner = mod_item["inner"]["module"]
            child_ids = mod_inner.get("items", [])
            sub_items = {"structs": [], "enums": [], "functions": []}
            for cid in child_ids:
                c = self._get_item(cid)
                if not c:
                    continue
                ci = c.get("inner", {})
                if "struct" in ci:
                    sub_items["structs"].append(c)
                elif "enum" in ci:
                    sub_items["enums"].append(c)
                elif "function" in ci:
                    sub_items["functions"].append(c)
            # Merge into top-level items for flat output
            items["structs"].extend(sub_items["structs"])
            items["enums"].extend(sub_items["enums"])
            items["functions"].extend(sub_items["functions"])

        # Deduplicate by item ID (re-exports and submodule items can overlap)
        for key in ("structs", "enums", "functions"):
            seen = set()
            deduped = []
            for item in items[key]:
                iid = item["id"]
                if iid not in seen:
                    seen.add(iid)
                    deduped.append(item)
            items[key] = deduped

        # ── Free functions ──
        if items["functions"]:
            for func in sorted(items["functions"], key=lambda x: x["name"]):
                vis = func.get("visibility", "default")
                if vis != "public":
                    continue
                doc = self._format_doc(func.get("docs"))
                sig = self._render_fn_sig(func["name"], func["inner"]["function"])
                loc = self._span_loc(func)
                if doc:
                    lines.append(doc)
                lines.append(f"pub {sig};{loc}")
                lines.append("")

        # ── Structs ──
        for struct_item in sorted(items["structs"], key=lambda x: x["name"]):
            vis = struct_item.get("visibility", "default")
            if vis != "public":
                continue
            struct_data = struct_item["inner"]["struct"]
            generics = struct_data.get("generics", {})
            gen_str = self._render_generics(generics)
            where_str = self._render_where(generics)

            doc = self._format_doc(struct_item.get("docs"))
            loc = self._span_loc(struct_item)
            if doc:
                lines.append(doc)
            lines.append(f"pub struct {struct_item['name']}{gen_str}{where_str} {{{loc}")

            # Fields
            kind = struct_data.get("kind", {})
            if "plain" in kind:
                for fid in kind["plain"].get("fields", []):
                    field = self._get_item(fid)
                    if not field:
                        continue
                    fvis = field.get("visibility", "default")
                    vis_prefix = "pub " if fvis == "public" else ""
                    field_type = self._render_type(field["inner"].get("struct_field"))
                    fdoc = self._format_doc(field.get("docs"), "    ")
                    if fdoc:
                        lines.append(fdoc)
                    lines.append(f"    {vis_prefix}{field['name']}: {field_type},")
            elif "tuple" in kind:
                for fid in kind["tuple"]:
                    if fid is None:
                        continue
                    field = self._get_item(fid)
                    if not field:
                        continue
                    field_type = self._render_type(field["inner"].get("struct_field"))
                    lines.append(f"    {field_type},")

            lines.append("}")
            lines.append("")

            # Methods
            impl_ids = struct_data.get("impls", [])
            methods, trait_impls = self._get_impl_methods(impl_ids)

            if methods:
                lines.append(f"impl {struct_item['name']}{gen_str} {{")
                for m in sorted(methods, key=lambda x: x["name"]):
                    doc = self._format_doc(m.get("docs"), "    ")
                    sig = self._render_fn_sig(m["name"], m["inner"]["function"], is_method=True)
                    loc = self._span_loc(m)
                    if doc:
                        lines.append(doc)
                    lines.append(f"    pub {sig};{loc}")
                lines.append("}")
                lines.append("")

            if trait_impls:
                for trait_name, trait_methods in trait_impls:
                    if trait_methods:
                        lines.append(f"impl {trait_name} for {struct_item['name']}{gen_str} {{")
                        for m in trait_methods:
                            sig = self._render_fn_sig(m["name"], m["inner"]["function"], is_method=True)
                            lines.append(f"    {sig};")
                        lines.append("}")
                        lines.append("")

        # ── Enums ──
        for enum_item in sorted(items["enums"], key=lambda x: x["name"]):
            vis = enum_item.get("visibility", "default")
            if vis != "public":
                continue
            enum_data = enum_item["inner"]["enum"]
            generics = enum_data.get("generics", {})
            gen_str = self._render_generics(generics)

            doc = self._format_doc(enum_item.get("docs"))
            loc = self._span_loc(enum_item)
            if doc:
                lines.append(doc)
            lines.append(f"pub enum {enum_item['name']}{gen_str} {{{loc}")

            for vid in enum_data.get("variants", []):
                variant = self._get_item(vid)
                if not variant:
                    continue
                vname = variant["name"]
                vkind = variant["inner"]["variant"].get("kind", "plain")
                vdoc = self._format_doc(variant.get("docs"), "    ")

                if vdoc:
                    lines.append(vdoc)

                if isinstance(vkind, str) and vkind == "plain":
                    lines.append(f"    {vname},")
                elif isinstance(vkind, dict):
                    if "tuple" in vkind:
                        fields = []
                        for fid in vkind["tuple"]:
                            if fid is None:
                                fields.append("_")
                                continue
                            f = self._get_item(fid)
                            if f:
                                fields.append(self._render_type(f["inner"].get("struct_field")))
                            else:
                                fields.append("?")
                        lines.append(f"    {vname}({', '.join(fields)}),")
                    elif "struct" in vkind:
                        lines.append(f"    {vname} {{")
                        for fid in vkind["struct"].get("fields", []):
                            f = self._get_item(fid)
                            if not f:
                                continue
                            ft = self._render_type(f["inner"].get("struct_field"))
                            fdoc = self._format_doc(f.get("docs"), "        ")
                            if fdoc:
                                lines.append(fdoc)
                            lines.append(f"        {f['name']}: {ft},")
                        lines.append("    },")
                    else:
                        lines.append(f"    {vname},")
                else:
                    lines.append(f"    {vname},")

            lines.append("}")
            lines.append("")

            # Enum methods
            impl_ids = enum_data.get("impls", [])
            methods, trait_impls = self._get_impl_methods(impl_ids)

            if methods:
                lines.append(f"impl {enum_item['name']}{gen_str} {{")
                for m in sorted(methods, key=lambda x: x["name"]):
                    doc = self._format_doc(m.get("docs"), "    ")
                    sig = self._render_fn_sig(m["name"], m["inner"]["function"], is_method=True)
                    loc = self._span_loc(m)
                    if doc:
                        lines.append(doc)
                    lines.append(f"    pub {sig};{loc}")
                lines.append("}")
                lines.append("")

            if trait_impls:
                for trait_name, trait_methods in trait_impls:
                    if trait_methods:
                        lines.append(f"impl {trait_name} for {enum_item['name']}{gen_str} {{")
                        for m in trait_methods:
                            sig = self._render_fn_sig(m["name"], m["inner"]["function"], is_method=True)
                            lines.append(f"    {sig};")
                        lines.append("}")
                        lines.append("")

        return "\n".join(lines)


def main():
    if len(sys.argv) < 2:
        print("Usage: python rustdoc_digest.py target/doc/*.json [> DIGEST.md]", file=sys.stderr)
        sys.exit(1)

    json_files = sys.argv[1:]

    # Order crates: types first (foundation), then providers, context, engine, tui, webfetch, lsp, cli
    crate_order = {
        "forge_types": 0,
        "forge_providers": 1,
        "forge_context": 2,
        "forge_engine": 3,
        "forge_tui": 4,
        "forge_webfetch": 5,
        "forge_lsp": 6,
        "forge": 7,
    }

    crates = []
    for path in json_files:
        with open(path) as f:
            data = json.load(f)
        root = data["index"][str(data["root"])]
        name = root["name"]
        crates.append((crate_order.get(name, 99), name, data))

    crates.sort(key=lambda x: x[0])

    output = []
    output.append("# Forge API Digest")
    output.append("")
    output.append("Auto-generated from rustdoc JSON. Public API surface for all workspace crates.")
    output.append("")

    for _, name, data in crates:
        digest = RustdocDigest(data)
        content = digest.generate()
        if not content.strip():
            continue
        display_name = name.replace("_", "-")
        output.append(f"## {display_name}")
        output.append("")
        output.append("```rust")
        output.append(content.rstrip())
        output.append("```")
        output.append("")

    print("\n".join(output))


if __name__ == "__main__":
    main()
