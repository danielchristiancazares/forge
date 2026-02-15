use crate::theme::Palette;
use forge_engine::{App, PlanState};
use forge_types::{PlanStep, StepStatus};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;

const STEP_HEIGHT: u16 = 3; // Text line + Timer line + Gap

pub fn draw(frame: &mut Frame, app: &App, area: Rect, palette: &Palette) {
    let plan = match app.plan_state() {
        PlanState::Proposed(p) | PlanState::Active(p) => p,
        PlanState::Inactive => return,
    };

    // Flatten steps for linear carousel
    let steps: Vec<&PlanStep> = plan.phases().iter().flat_map(|p| &p.steps).collect();

    if steps.is_empty() {
        return;
    }

    // Find active step index for centering
    // Default to the first pending step if no active step, or last step if all done
    let active_index = steps
        .iter()
        .position(|s| matches!(s.status, StepStatus::Active))
        .or_else(|| {
            steps
                .iter()
                .position(|s| matches!(s.status, StepStatus::Pending))
        })
        .unwrap_or(steps.len().saturating_sub(1));

    let center_y = area.y + (area.height / 2);

    // Calculate scroll offset: we want the active step's center to be at center_y
    // Step visual center is y + 0 (since it's 2 lines of text/timer)
    // Absolute Y of step I = area.y + offset + (i * height)
    // We want: area.y + offset + (active * height) = center_y
    // offset = center_y - area.y - (active * height)
    let offset_y =
        (center_y as i32) - (area.y as i32) - ((active_index as i32) * (STEP_HEIGHT as i32));

    for (i, step) in steps.iter().enumerate() {
        let step_top_y = (area.y as i32) + offset_y + ((i as i32) * (STEP_HEIGHT as i32));

        // Culling
        if step_top_y < (area.y as i32) - (STEP_HEIGHT as i32)
            || step_top_y > (area.y as i32) + (area.height as i32)
        {
            continue;
        }

        let distance = (i as isize - active_index as isize).abs();
        let (style, icon) = step_appearance(&step.status, distance, palette);

        // Draw Step Text
        // Center vertically within the step slot? No, top align for consistency
        let text_area = Rect {
            x: area.x,
            y: step_top_y as u16,
            width: area.width,
            height: 1,
        };

        let content = format!("{}  {}", icon, step.description);
        frame.render_widget(
            Paragraph::new(content)
                .style(style)
                .alignment(Alignment::Center),
            text_area,
        );

        // Draw Timer (only for active step)
        if i == active_index && matches!(step.status, StepStatus::Active) {
            let timer_area = Rect {
                x: area.x,
                y: (step_top_y + 1) as u16,
                width: area.width,
                height: 1,
            };
            // TODO: Connect real elapsed time
            frame.render_widget(
                Paragraph::new("0s")
                    .style(Style::default().fg(palette.accent))
                    .alignment(Alignment::Center),
                timer_area,
            );
        }
    }
}

fn step_appearance(
    status: &StepStatus,
    distance: isize,
    palette: &Palette,
) -> (Style, &'static str) {
    let base_style = Style::default();

    match status {
        StepStatus::Active => (
            base_style
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD),
            "⠸", // Spinner placeholder
        ),
        StepStatus::Pending => {
            let color = match distance {
                0 => palette.text_primary, // Should be unreachable if active exists
                1 => palette.text_secondary,
                2 => palette.text_muted,
                _ => palette.text_disabled,
            };
            (base_style.fg(color), "○")
        }
        StepStatus::Complete(_) => (base_style.fg(palette.text_disabled), "✓"),
        StepStatus::Failed(_) => (base_style.fg(palette.error), "✗"),
        StepStatus::Skipped(_) => (base_style.fg(palette.text_disabled), "⊘"),
    }
}
