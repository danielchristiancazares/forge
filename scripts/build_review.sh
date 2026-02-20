#!/usr/bin/env bash
# Build review files for IFA adherence review
# Default: one file per logical group in review/
# --paste: single file with all groups for inline pasting into UI prompts

set -euo pipefail
cd "$(dirname "$0")/.."

PASTE_MODE=0
for arg in "$@"; do
    case "$arg" in
        --paste) PASTE_MODE=1 ;;
    esac
done

emit_file() {
    local path="$1"
    if [ -f "$path" ]; then
        local lines
        lines=$(wc -l < "$path")
        echo ""
        echo "## FILE: $path ($lines lines)"
        echo ""
        echo '```rust'
        cat "$path"
        echo '```'
        echo ""
    fi
}

# ── Groups ────────────────────────────────────────────────────────────
# Define once, used by both modes. Format: emit_groups callback
# callback receives (num, slug, name, file...) per group.

emit_groups() {
    local cb="$1"

    "$cb" 01 cli "CLI" \
        cli/src/main.rs cli/src/crash_hardening.rs cli/src/assets.rs

    "$cb" 02 core "Core" \
        core/src/lib.rs core/src/environment.rs core/src/env_context.rs \
        core/src/notifications.rs core/src/errors.rs core/src/display.rs \
        core/src/thinking.rs core/src/security.rs core/src/util.rs

    "$cb" 03 types_core "Types — Core Domain" \
        types/src/lib.rs types/src/message.rs types/src/model.rs \
        types/src/budget.rs types/src/text.rs types/src/proofs.rs \
        types/src/confusables.rs

    "$cb" 04 types_ui "Types — UI, Plan, Sanitization" \
        types/src/plan.rs types/src/sanitize.rs \
        types/src/ui/mod.rs types/src/ui/input.rs types/src/ui/modal.rs \
        types/src/ui/view_state.rs types/src/ui/scroll.rs \
        types/src/ui/animation.rs types/src/ui/panel.rs types/src/ui/history.rs

    "$cb" 05 config "Config" \
        config/src/lib.rs

    "$cb" 06 engine_lib "Engine — Lib & State" \
        engine/src/lib.rs engine/src/state.rs engine/src/config.rs \
        engine/src/session_state.rs engine/src/runtime/mod.rs \
        engine/src/ui/mod.rs engine/src/ui/file_picker.rs

    "$cb" 07 engine_app_core "Engine — App Core & Streaming" \
        engine/src/app/mod.rs engine/src/app/init.rs engine/src/app/streaming.rs

    "$cb" 08 engine_commands "Engine — Commands & Input" \
        engine/src/app/commands.rs engine/src/app/input_modes.rs

    "$cb" 09 engine_toolloop "Engine — Tool Loop & Gate" \
        engine/src/app/tool_loop.rs engine/src/app/tool_gate.rs

    "$cb" 10 engine_persistence "Engine — Persistence, Plans, Integration" \
        engine/src/app/persistence.rs engine/src/app/plan.rs \
        engine/src/app/checkpoints.rs engine/src/app/distillation.rs \
        engine/src/app/lsp_integration.rs engine/src/app/tests.rs

    "$cb" 11 context "Context" \
        context/src/lib.rs context/src/manager.rs context/src/history.rs \
        context/src/working_context.rs context/src/stream_journal.rs \
        context/src/tool_journal.rs context/src/distillation.rs \
        context/src/fact_store.rs context/src/librarian.rs \
        context/src/model_limits.rs context/src/token_counter.rs \
        context/src/sqlite_security.rs context/src/time_utils.rs

    "$cb" 12 providers_claude "Providers — Dispatch & Claude" \
        providers/src/lib.rs providers/src/claude.rs \
        providers/src/sse_types.rs providers/src/retry.rs

    "$cb" 13 providers_openai_gemini "Providers — OpenAI & Gemini" \
        providers/src/openai.rs providers/src/gemini.rs

    "$cb" 14 tools_framework "Tools — Framework & Builtins" \
        tools/src/lib.rs tools/src/builtins.rs tools/src/config.rs \
        tools/src/sandbox.rs tools/src/command_blacklist.rs \
        tools/src/phase_gate.rs

    "$cb" 15 tools_execution "Tools — Execution" \
        tools/src/shell.rs tools/src/process.rs \
        tools/src/powershell_ast.rs tools/src/windows_run.rs \
        tools/src/windows_run_host.rs

    "$cb" 16 tools_features "Tools — Git, Search, LP1, Memory" \
        tools/src/git.rs tools/src/search.rs tools/src/lp1.rs \
        tools/src/region_hash.rs tools/src/memory.rs tools/src/recall.rs \
        tools/src/change_recording.rs

    "$cb" 17 tools_webfetch "Tools — Webfetch" \
        tools/src/webfetch/mod.rs tools/src/webfetch/types.rs \
        tools/src/webfetch/resolved.rs tools/src/webfetch/http.rs \
        tools/src/webfetch/extract.rs tools/src/webfetch/chunk.rs \
        tools/src/webfetch/cache.rs tools/src/webfetch/robots.rs

    "$cb" 18 tui_core "TUI — Core & Theme" \
        tui/src/lib.rs tui/src/theme.rs tui/src/effects.rs \
        tui/src/shared.rs tui/src/format.rs

    "$cb" 19 tui_input "TUI — Input & Focus" \
        tui/src/input.rs tui/src/approval.rs \
        tui/src/focus/mod.rs tui/src/focus/idle.rs \
        tui/src/focus/content.rs tui/src/focus/reviewing.rs \
        tui/src/focus/executing.rs

    "$cb" 20 tui_content "TUI — Content Rendering" \
        tui/src/markdown.rs tui/src/messages.rs \
        tui/src/tool_display.rs tui/src/tool_result_summary.rs \
        tui/src/diff_render.rs

    "$cb" 21 utils "Utils" \
        utils/src/lib.rs utils/src/security.rs utils/src/diff.rs \
        utils/src/atomic_write.rs utils/src/windows_acl.rs

    "$cb" 22 lsp "LSP" \
        lsp/src/lib.rs lsp/src/manager.rs lsp/src/server.rs \
        lsp/src/codec.rs lsp/src/protocol.rs \
        lsp/src/diagnostics.rs lsp/src/types.rs
}

# ── Default mode: one file per group ──────────────────────────────────

REVIEW_DIR="review"

group_to_file() {
    local num="$1" slug="$2" name="$3"; shift 3
    local out="$REVIEW_DIR/${num}_${slug}.md"
    {
        cat <<EOF
# $num. $name

> Review this code for IFA adherence. Cite specific IFA sections violated (e.g., §2.1, §9.2).
> Prioritize: structural violations > missing proof types > optional fields in core > cosmetic.

EOF
        for path in "$@"; do
            emit_file "$path"
        done
    } > "$out"
    local lines
    lines=$(wc -l < "$out")
    printf "  %-44s %5d lines\n" "$out" "$lines"
}

run_default() {
    rm -rf "$REVIEW_DIR"
    mkdir -p "$REVIEW_DIR"
    emit_groups group_to_file
}

# ── Paste mode: single file, one section per group ────────────────────

PASTE_NUM=0

group_to_paste() {
    local num="$1" slug="$2" name="$3"; shift 3
    PASTE_NUM=$((PASTE_NUM + 1))
    echo ""
    echo "---"
    echo ""
    echo "# PASTE $PASTE_NUM: $name"
    echo ""
    echo "---"
    echo ""
    for path in "$@"; do
        emit_file "$path"
    done
}

run_paste() {
    {
        cat <<'PREAMBLE'
# Forge — IFA Adherence Review (Paste Mode)

Logically grouped file contents for pasting into deep-reasoning UI prompts.
Each paste is one logical section — submit alongside `INVARIANT_FIRST_ARCHITECTURE.md`.

**Review instructions:** For each paste, ask:

> Review this code for IFA adherence. Cite specific IFA sections violated (e.g., §2.1, §9.2).
> Prioritize: structural violations > missing proof types > optional fields in core > cosmetic.

PREAMBLE
        emit_groups group_to_paste
    } > "review_paste.md"
}

# ── Main ──────────────────────────────────────────────────────────────

if [ "$PASTE_MODE" -eq 1 ]; then
    run_paste
    total_lines=$(wc -l < "review_paste.md")
    echo "Generated review_paste.md ($total_lines lines)"
else
    echo "Generating review/ files:"
    run_default
    echo "Done."
fi
