//! The workbar: live workflow progress under the composer.
//!
//! The transcript carries one line per run — `started` while it runs, replaced
//! by its finish line when it settles (`history.rs`, `App::announce_settled_workflows`);
//! everything in between lives here, one row per run between the posture bar
//! and the metrics line. The workbar draws no rules of its own: it is footer
//! chrome like the rows around it, and a rule above and below it stacked with
//! the work surface's divider into a ladder of separators.
//!
//! ```text
//!  • Compare Cline with Codewhale  ████████××░░░░░░░░░░  4/10 done · 1 failed  2m 14s  ↓1.2M
//!  ✕ Release-readiness audit       ××××××××××××××××××××  0/2 done · 2 failed  355ms  Authorization failed: …
//! ```
//!
//! Row grammar: state mark · short title · 20-cell bar · outcome counts ·
//! elapsed · ↓tokens · tail. The bar paints succeeded agents `█` and failed
//! agents `×` in their own inks, so a run whose agents all failed never reads
//! as a finished bar. The counts say what succeeded, never what merely
//! settled. The tail is only ever true: `Large workflow` past
//! [`LARGE_WORKFLOW_AGENTS`], `· N queued` when the runtime reports follow-ups
//! waiting on a busy agent of this run, and for a run that fell short, the one
//! reason worth reading — the first sentence, cut only at a word boundary.
//!
//! Every state reads without colour (a distinct mark, a distinct bar glyph,
//! and a word for anything that needs you). Nothing animates, so reduced
//! motion needs no second path; ASCII-safe terminals get the backend's
//! per-cell fallbacks (`#`, `X`, `:`).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::tui::ui_text::{semantic_truncate, truncate_line_to_width};
use crate::tui::widgets::workflow_panel::{WorkflowPanel, WorkflowPanelLifecycle};
use codewhale_localization::{Locale, MessageId, tr};
use codewhale_palette::UiTheme;

/// Cells in one progress bar.
pub(crate) const BAR_CELLS: usize = 20;
/// Agents a run must have admitted before the workbar calls it large. At this
/// size a run is no longer something to watch row by row, and its token use
/// is the thing worth a glance.
pub(crate) const LARGE_WORKFLOW_AGENTS: usize = 25;
/// Most run rows painted at once; the rest fold into one `+N more` row.
pub(crate) const MAX_RUN_ROWS: usize = 6;
/// The name column never shrinks below this before other columns go.
const MIN_NAME_COLS: usize = 12;
/// Nor grows past this: a long goal must not push the facts off the row.
const MAX_NAME_COLS: usize = 40;
/// Columns kept for the tail before the name takes the rest of the row.
const TAIL_RESERVE_COLS: usize = 24;
/// A reason squeezed below this many columns says nothing; it is left out.
const MIN_REASON_COLS: usize = 12;
/// Below this width the bar gives its columns to the name.
const BAR_MIN_WIDTH: usize = 72;
const GAP: &str = "  ";
const DONE_CELL: &str = "█";
const FAILED_CELL: &str = "×";
const OPEN_CELL: &str = "░";

/// One run as the workbar paints it: the run's own state plus the runtime's
/// count of follow-ups waiting on its busy agents.
pub(crate) struct WorkbarRun<'a> {
    pub panel: &'a WorkflowPanel,
    pub queued: usize,
}

/// Rows the workbar wants for `runs` runs, before the frame's budget: one per
/// run, folded past [`MAX_RUN_ROWS`]. No runs, no rows.
#[must_use]
pub(crate) fn desired_rows(runs: usize) -> u16 {
    let rows = if runs > MAX_RUN_ROWS {
        MAX_RUN_ROWS + 1
    } else {
        runs
    };
    u16::try_from(rows).unwrap_or(u16::MAX)
}

/// Paint the workbar into `area`: one row per run.
pub(crate) fn render(
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    runs: &[WorkbarRun<'_>],
    now_ms: u64,
    theme: &UiTheme,
    locale: Locale,
) {
    use ratatui::widgets::{Paragraph, Widget};
    if area.width == 0 || area.height == 0 || runs.is_empty() {
        return;
    }
    let rows = lines(
        runs,
        area.width,
        usize::from(area.height),
        now_ms,
        theme,
        locale,
    );
    Paragraph::new(rows).render(area, buf);
}

/// Paint `runs` into at most `max_rows` lines of `width` columns. When the
/// runs do not all fit, the last row says how many are hidden and names the
/// key that lists them.
#[must_use]
pub(crate) fn lines(
    runs: &[WorkbarRun<'_>],
    width: u16,
    max_rows: usize,
    now_ms: u64,
    theme: &UiTheme,
    locale: Locale,
) -> Vec<Line<'static>> {
    let width = usize::from(width);
    if runs.is_empty() || max_rows == 0 || width < 8 {
        return Vec::new();
    }
    let shown = if runs.len() > max_rows {
        max_rows.saturating_sub(1)
    } else {
        runs.len()
    };
    let cells: Vec<RowCells> = runs[..shown]
        .iter()
        .map(|run| RowCells::new(run, now_ms, locale))
        .collect();
    let layout = Layout::fit(&cells, width.saturating_sub(1));
    let mut out: Vec<Line<'static>> = cells
        .iter()
        .map(|cells| cells.line(&layout, width, theme))
        .collect();
    let hidden = runs.len() - shown;
    if hidden > 0 {
        let arrow = if crate::tui::color_compat::ascii_safe_enabled() {
            "v"
        } else {
            "↓"
        };
        let text = format!(
            " {} · {arrow} {}",
            tr(locale, MessageId::WorkbarMoreRuns).replace("{count}", &hidden.to_string()),
            tr(locale, MessageId::FooterHintToManage),
        );
        out.push(Line::from(Span::styled(
            truncate_line_to_width(&text, width),
            Style::default().fg(theme.text_hint),
        )));
    }
    out
}

/// The text of one row, before layout.
struct RowCells {
    lifecycle: WorkflowPanelLifecycle,
    mark: &'static str,
    name: String,
    /// Bar cells for succeeded and failed agents; the rest are open.
    done_cells: usize,
    failed_cells: usize,
    /// `k/n done`, then ` · m failed · c cancelled` when not zero.
    done: String,
    problems: String,
    elapsed: String,
    tokens: Option<String>,
    /// `(text, needs attention)` chips, then the reason, in that order.
    chips: Vec<(String, bool)>,
    reason: Option<String>,
}

impl RowCells {
    fn new(run: &WorkbarRun<'_>, now_ms: u64, locale: Locale) -> Self {
        let panel = run.panel;
        let (succeeded, failed_rows, _, total) = panel.row_outcomes();
        let lifecycle = panel.lifecycle;
        let (done_cells, failed_cells) = bar_cells(succeeded, failed_rows, total);
        let counts = panel.outcome_counts_text();
        // The done count reads quiet; failures and cancels read in error ink.
        let (done, problems) = match counts.split_once(" · ") {
            Some((done, rest)) if total > 0 => (done.to_string(), format!(" · {rest}")),
            _ if total == 0 && panel.failure_cancel_counts() != (0, 0) => (String::new(), counts),
            _ => (counts, String::new()),
        };
        let end = panel.completed_at_ms.unwrap_or(now_ms);
        let elapsed = if panel.started_at_ms == 0 {
            String::new()
        } else {
            crate::elapsed::format_elapsed_ms(end.saturating_sub(panel.started_at_ms))
        };
        let tokens = panel.tokens_so_far().map(|tokens| {
            format!(
                "↓{}",
                crate::tui::footer_ui::format_token_count_compact(tokens)
            )
        });

        let mut chips = Vec::new();
        if total >= LARGE_WORKFLOW_AGENTS {
            chips.push((
                format!("⚠ {}", tr(locale, MessageId::WorkbarLargeWorkflow)),
                true,
            ));
        }
        if run.queued > 0 {
            chips.push((
                format!(
                    "· {}",
                    tr(locale, MessageId::AgentRailQueuedCount)
                        .replace("{count}", &run.queued.to_string())
                ),
                true,
            ));
        }
        // A settled run that fell short says why. Failed leads with the reason
        // (its mark and `N failed` already say it failed); gaps and stops keep
        // their word, because their marks alone are not enough to tell them
        // from the others on an ASCII terminal.
        let reason = panel.outcome_reason();
        let word = |id: MessageId| tr(locale, id).into_owned();
        let reason = match lifecycle {
            WorkflowPanelLifecycle::Failed => {
                Some(reason.unwrap_or_else(|| word(MessageId::WorkflowLineFailed)))
            }
            WorkflowPanelLifecycle::Degraded => Some(match reason {
                Some(reason) => format!(
                    "{} · {reason}",
                    word(MessageId::WorkflowLineFinishedWithGaps)
                ),
                None => word(MessageId::WorkflowLineFinishedWithGaps),
            }),
            WorkflowPanelLifecycle::Cancelled => Some(word(MessageId::WorkflowLineStopped)),
            _ => None,
        };
        Self {
            lifecycle,
            mark: lifecycle_mark(lifecycle),
            name: semantic_truncate(&panel.short_title(), MAX_NAME_COLS),
            done_cells,
            failed_cells,
            done,
            problems,
            elapsed,
            tokens,
            chips,
            reason,
        }
    }

    fn progress_width(&self) -> usize {
        self.done.width() + self.problems.width()
    }

    fn tail_width(&self) -> usize {
        self.chips
            .iter()
            .map(|(chip, _)| chip.width() + 1)
            .sum::<usize>()
            + self
                .reason
                .as_deref()
                .map_or(0, |reason| reason.width() + 1)
    }

    fn line(&self, layout: &Layout, width: usize, theme: &UiTheme) -> Line<'static> {
        let state_ink = lifecycle_ink(self.lifecycle, theme);
        let quiet = Style::default().fg(theme.text_muted);
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(format!("{} ", self.mark), Style::default().fg(state_ink)),
            Span::styled(
                pad(
                    &semantic_truncate(&self.name, layout.name_room),
                    layout.name_cols,
                ),
                Style::default()
                    .fg(theme.text_body)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if layout.bar {
            let open = BAR_CELLS - self.done_cells - self.failed_cells;
            spans.push(Span::raw(GAP));
            spans.push(Span::styled(
                DONE_CELL.repeat(self.done_cells),
                Style::default().fg(theme.success),
            ));
            spans.push(Span::styled(
                FAILED_CELL.repeat(self.failed_cells),
                Style::default().fg(theme.error_fg),
            ));
            spans.push(Span::styled(
                OPEN_CELL.repeat(open),
                Style::default().fg(theme.text_hint),
            ));
        }
        spans.push(Span::raw(GAP));
        spans.push(Span::styled(self.done.clone(), quiet));
        spans.push(Span::styled(
            self.problems.clone(),
            Style::default().fg(theme.error_fg),
        ));
        spans.push(Span::raw(" ".repeat(
            layout.progress_cols.saturating_sub(self.progress_width()),
        )));
        if layout.elapsed_cols > 0 {
            spans.push(Span::raw(GAP));
            spans.push(Span::styled(pad(&self.elapsed, layout.elapsed_cols), quiet));
        }
        if layout.tokens_cols > 0 {
            spans.push(Span::raw(GAP));
            spans.push(Span::styled(
                pad(self.tokens.as_deref().unwrap_or(""), layout.tokens_cols),
                quiet,
            ));
        }
        let mut used: usize = spans.iter().map(|span| span.content.width()).sum();
        for (index, (chip, attention)) in self.chips.iter().enumerate() {
            let gap = if index == 0 { GAP } else { " " };
            spans.push(Span::raw(gap));
            let ink = if *attention {
                theme.warning
            } else {
                theme.text_muted
            };
            spans.push(Span::styled(chip.clone(), Style::default().fg(ink)));
            used += gap.width() + chip.width();
        }
        if let Some(reason) = self.reason.as_deref() {
            let gap = if self.chips.is_empty() { GAP } else { " " };
            // The reason takes what the row has left, cut at a word boundary.
            let room = width.saturating_sub(used + gap.width());
            if room >= MIN_REASON_COLS.min(reason.width()) {
                spans.push(Span::raw(gap));
                spans.push(Span::styled(
                    semantic_truncate(reason, room),
                    Style::default().fg(state_ink),
                ));
            }
        }
        clip_line(spans, width)
    }
}

/// Split [`BAR_CELLS`] between succeeded and failed agents of `total`. A
/// non-zero count always gets at least one cell, so one failure in a large run
/// is still visible, and the two segments never overflow the bar.
fn bar_cells(succeeded: usize, failed: usize, total: usize) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let cells = |count: usize| {
        let cells = (count * BAR_CELLS + total / 2) / total;
        if count > 0 { cells.max(1) } else { 0 }.min(BAR_CELLS)
    };
    let failed = cells(failed);
    let done = cells(succeeded).min(BAR_CELLS - failed);
    (done, failed)
}

/// Column widths shared by every visible row, so the columns line up.
struct Layout {
    /// Room the name is truncated into, and the column it is padded to (the
    /// widest name as shown, which a word-boundary cut can leave shorter).
    name_room: usize,
    name_cols: usize,
    bar: bool,
    progress_cols: usize,
    elapsed_cols: usize,
    tokens_cols: usize,
}

impl Layout {
    /// Fit the shared columns into `width`. The name and the progress count
    /// always stay; the bar goes first, then tokens, then the room kept for
    /// the tail, then elapsed — so a narrow terminal keeps the facts and loses
    /// the picture. The tail takes whatever the row has left after the name.
    fn fit(rows: &[RowCells], width: usize) -> Self {
        let widest = |f: &dyn Fn(&RowCells) -> usize| rows.iter().map(f).max().unwrap_or(0);
        let name_want = widest(&|row| row.name.width());
        let mut tail_want = widest(&RowCells::tail_width).min(TAIL_RESERVE_COLS);
        let mut layout = Self {
            name_room: 0,
            name_cols: 0,
            bar: width >= BAR_MIN_WIDTH,
            progress_cols: widest(&RowCells::progress_width),
            elapsed_cols: widest(&|row| row.elapsed.width()),
            tokens_cols: widest(&|row| row.tokens.as_deref().map_or(0, UnicodeWidthStr::width)),
        };
        let name_cols = loop {
            let fixed = 2 // mark + space
                + if layout.bar { BAR_CELLS + GAP.len() } else { 0 }
                + GAP.len() + layout.progress_cols
                + if layout.elapsed_cols > 0 { GAP.len() + layout.elapsed_cols } else { 0 }
                + if layout.tokens_cols > 0 { GAP.len() + layout.tokens_cols } else { 0 };
            let room = width.saturating_sub(fixed + tail_want);
            if room >= MIN_NAME_COLS.min(name_want) {
                break name_want.min(room.max(MIN_NAME_COLS.min(name_want)));
            }
            if layout.bar {
                layout.bar = false;
            } else if layout.tokens_cols > 0 {
                layout.tokens_cols = 0;
            } else if tail_want > 0 {
                tail_want = 0;
            } else if layout.elapsed_cols > 0 {
                layout.elapsed_cols = 0;
            } else {
                break width.saturating_sub(fixed).max(1).min(name_want.max(1));
            }
        };
        // Word-boundary truncation can land short of the room; the column is
        // as wide as the widest name it actually shows, not the room it had.
        layout.name_room = name_cols;
        layout.name_cols = widest(&|row| semantic_truncate(&row.name, name_cols).width());
        layout
    }
}

/// The run's mark: distinct shapes, so the state reads without colour.
fn lifecycle_mark(lifecycle: WorkflowPanelLifecycle) -> &'static str {
    match lifecycle {
        WorkflowPanelLifecycle::Pending => crate::tui::glyphs::AVAILABLE,
        WorkflowPanelLifecycle::Running => "•",
        WorkflowPanelLifecycle::Succeeded => crate::tui::glyphs::DONE,
        WorkflowPanelLifecycle::Degraded => crate::tui::glyphs::ATTENTION,
        WorkflowPanelLifecycle::Failed => crate::tui::glyphs::FAILED,
        WorkflowPanelLifecycle::Cancelled => "⊘",
    }
}

/// Running is working ink, not attention; only trouble spends warning/error.
fn lifecycle_ink(lifecycle: WorkflowPanelLifecycle, theme: &UiTheme) -> ratatui::style::Color {
    match lifecycle {
        WorkflowPanelLifecycle::Pending | WorkflowPanelLifecycle::Cancelled => theme.text_muted,
        WorkflowPanelLifecycle::Running => theme.accent_action,
        WorkflowPanelLifecycle::Succeeded => theme.success,
        WorkflowPanelLifecycle::Degraded => theme.warning,
        WorkflowPanelLifecycle::Failed => theme.error_fg,
    }
}

fn pad(text: &str, cols: usize) -> String {
    let width = text.width();
    if width >= cols {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(cols - width))
    }
}

/// Clip a styled row at `width` columns without splitting a wide glyph.
fn clip_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut used = 0usize;
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        let span_width = span.content.width();
        if used + span_width <= width {
            used += span_width;
            out.push(span);
            continue;
        }
        let room = width.saturating_sub(used);
        if room > 0 {
            let text = truncate_line_to_width(&span.content, room);
            out.push(Span::styled(text, span.style));
        }
        break;
    }
    Line::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::widgets::workflow_panel::{WorkflowPanelEvent, WorkflowRowStatus};
    use ratatui::{Terminal, backend::TestBackend};

    const NOW: u64 = 1_000_000;

    fn run(label: &str, started: u64, agents: usize, settled: usize) -> WorkflowPanel {
        let mut panel = WorkflowPanel::new(format!("run-{label}"), label, started);
        panel.apply_event(WorkflowPanelEvent::PhaseStarted {
            title: "Survey".to_string(),
            at_ms: started,
        });
        for index in 0..agents {
            let value = serde_json::json!({
                "type": "task_started",
                "task_id": format!("{label}-{index}"),
                "workflow_task_label": format!("agent-{index}"),
                "at_ms": started,
            });
            panel.apply_event(WorkflowPanelEvent::from_json_value(&value).expect("task_started"));
        }
        for index in 0..settled {
            complete(&mut panel, label, index, WorkflowRowStatus::Succeeded, None);
        }
        panel
    }

    fn complete(
        panel: &mut WorkflowPanel,
        label: &str,
        index: usize,
        status: WorkflowRowStatus,
        reason: Option<&str>,
    ) {
        panel.apply_event(WorkflowPanelEvent::TaskCompleted {
            task_id: format!("{label}-{index}"),
            status,
            usage: None,
            reason: reason.map(str::to_string),
            at_ms: panel.started_at_ms + 1_000,
        });
    }

    fn settled(
        label: &str,
        status: WorkflowPanelLifecycle,
        agents: usize,
        done: usize,
    ) -> WorkflowPanel {
        let mut panel = run(label, NOW - 60_000, agents, done);
        panel.apply_event(WorkflowPanelEvent::RunCompleted {
            status,
            error: None,
            at_ms: NOW - 1_000,
        });
        panel
    }

    /// The founder's run from 2026-09-28, as its status record came back: a
    /// read-only audit whose two agents both failed auth 355 ms in.
    fn founder_all_failed() -> WorkflowPanel {
        let goal = "Read-only release-readiness audit for Codewhale v0.10.1. Determine \
                    actionable remaining blockers from live evidence, separating engine \
                    release from desktop and deferred backlog. No edits.";
        let reason = "[auth] Authorization failed: You have run out of credits or need a \
                      Grok subscription. Add credits at https://grok.com/?_s=usage.\n\
                      (provider `xAI` · requested model `grok-4.6`)";
        let error = "no task produced a result: all 2 task(s) failed and 1 fan-out(s) lost \
                     every slot (no work survived them); the recorded result reflects no \
                     completed work";
        WorkflowPanel::from_run_json(&serde_json::json!({
            "run_id": "workflow_6409ebe6",
            "status": "failed",
            "started_at_ms": 1_790_651_979_185_u64,
            "completed_at_ms": 1_790_651_979_540_u64,
            "workflow_goal": goal,
            "error": error,
            "events": [
                {"at_ms": 1_790_651_979_185_u64, "type": "run_started", "workflow_goal": goal},
                {"at_ms": 1_790_651_979_187_u64, "type": "phase_started", "title": "parallel-evidence"},
                {"at_ms": 1_790_651_979_265_u64, "type": "task_started", "task_id": "agent_f6665537",
                 "workflow_task_label": "engine-readiness"},
                {"at_ms": 1_790_651_979_302_u64, "type": "task_started", "task_id": "agent_94fe8e6a",
                 "workflow_task_label": "desktop-readiness"},
                {"at_ms": 1_790_651_979_514_u64, "type": "task_completed", "task_id": "agent_f6665537",
                 "status": "failed", "reason": reason},
                {"at_ms": 1_790_651_979_523_u64, "type": "task_completed", "task_id": "agent_94fe8e6a",
                 "status": "failed", "reason": reason},
                {"at_ms": 1_790_651_979_540_u64, "type": "run_completed", "status": "failed",
                 "error": error},
            ],
        }))
        .expect("run record")
    }

    fn partial_success() -> WorkflowPanel {
        let mut panel = run("Port the fixture suite", NOW - 134_000, 4, 3);
        complete(
            &mut panel,
            "Port the fixture suite",
            3,
            WorkflowRowStatus::Failed,
            Some("Timed out waiting for the model after 600s. Retried once."),
        );
        panel.apply_event(WorkflowPanelEvent::RunCompleted {
            status: WorkflowPanelLifecycle::Degraded,
            error: Some("completed with dropped slots".to_string()),
            at_ms: NOW,
        });
        panel
    }

    fn render(runs: &[WorkbarRun<'_>], width: u16, rows: u16) -> String {
        buffer_rows(&render_band(runs, width, rows)).join("\n")
    }

    fn render_band(runs: &[WorkbarRun<'_>], width: u16, height: u16) -> ratatui::buffer::Buffer {
        let area = ratatui::layout::Rect::new(0, 0, width, height);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| {
                super::render(
                    area,
                    frame.buffer_mut(),
                    runs,
                    NOW,
                    &codewhale_palette::UI_THEME,
                    Locale::En,
                );
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    fn buffer_rows(buf: &ratatui::buffer::Buffer) -> Vec<String> {
        let area = buf.area;
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn one(panel: &WorkflowPanel) -> [WorkbarRun<'_>; 1] {
        [WorkbarRun { panel, queued: 0 }]
    }

    /// The four states the founder's screenshot needed, as the buffer paints
    /// them. These strings are the PR's evidence; keep them exact.
    #[test]
    fn snapshot_running_all_failed_partial_and_narrow() {
        let mut running = run("Compare Cline with Codewhale", NOW - 134_000, 10, 4);
        complete(
            &mut running,
            "Compare Cline with Codewhale",
            4,
            WorkflowRowStatus::Failed,
            Some("rate limited"),
        );
        running.budget_spent = 1_234_567;
        let founder = founder_all_failed();
        let partial = partial_success();
        let cases = [
            (
                "running",
                render(&one(&running), 110, 1),
                " • Compare Cline with Codewhale  ████████××░░░░░░░░░░  4/10 done · 1 failed  2m 14s  ↓1.2M",
            ),
            (
                "all failed",
                render(&one(&founder), 140, 1),
                " ✕ Read-only release-readiness audit for…  ××××××××××××××××××××  0/2 done · 2 failed  355ms  Authorization failed: You have run out of…",
            ),
            (
                "partial success",
                render(&one(&partial), 140, 1),
                " ◆ Port the fixture suite  ███████████████×××××  3/4 done · 1 failed  2m 14s  finished with gaps · Timed out waiting for the model after…",
            ),
            (
                "narrow 60",
                render(&one(&founder), 60, 1),
                " ✕ Read-only release-readiness…  0/2 done · 2 failed  355ms",
            ),
            (
                "narrow 40",
                render(&one(&founder), 40, 1),
                " ✕ Read-only…  0/2 done · 2 failed",
            ),
        ];
        let wrong: Vec<String> = cases
            .iter()
            .filter(|(_, actual, expected)| actual != expected)
            .map(|(name, actual, expected)| {
                format!("{name}:\n  got  {actual:?}\n  want {expected:?}")
            })
            .collect();
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
        for (_, row, _) in &cases[3..] {
            assert!(!row.contains('█') && !row.contains('×'), "{row}");
        }
        assert!(cases[3].1.width() <= 60 && cases[4].1.width() <= 40);
    }

    #[test]
    fn a_failed_run_never_paints_a_success_bar_or_rounds_to_zero_seconds() {
        let founder = founder_all_failed();
        let band = render_band(&one(&founder), 140, 1);
        let row = &buffer_rows(&band)[0];
        assert!(!row.contains('█'), "no agent succeeded: {row}");
        assert!(row.contains("0/2 done · 2 failed"), "{row}");
        assert!(row.contains("355ms") && !row.contains(" 0s"), "{row}");
        // The failed cells carry error ink, not the success ink of done cells.
        let theme = codewhale_palette::UI_THEME;
        let failed_cell = (0..140)
            .find(|&x| band[(x, 0)].symbol() == FAILED_CELL)
            .expect("failed cells");
        assert_eq!(band[(failed_cell, 0)].fg, theme.error_fg);
        // The agents' own reason, not the run's aggregate, and not cut mid-word.
        assert!(row.contains("Authorization failed"), "{row}");
        assert!(!row.contains("no task produced"), "{row}");
        assert!(row.ends_with('…') || row.ends_with("subscription"), "{row}");
    }

    #[test]
    fn settled_rows_read_without_colour() {
        let done = settled("audit", WorkflowPanelLifecycle::Succeeded, 3, 3);
        let mut failed = run("migrate", NOW - 60_000, 2, 0);
        failed.apply_event(WorkflowPanelEvent::RunCompleted {
            status: WorkflowPanelLifecycle::Failed,
            error: Some("script error".to_string()),
            at_ms: NOW - 1_000,
        });
        let gaps = settled("review", WorkflowPanelLifecycle::Degraded, 2, 1);
        let runs = [
            WorkbarRun {
                panel: &done,
                queued: 0,
            },
            WorkbarRun {
                panel: &failed,
                queued: 0,
            },
            WorkbarRun {
                panel: &gaps,
                queued: 0,
            },
        ];
        assert_eq!(
            render(&runs, 80, 3),
            [
                " ✓ audit    ████████████████████  3/3 done  59s",
                " ✕ migrate  ░░░░░░░░░░░░░░░░░░░░  0/2 done  59s  script error",
                " ◆ review   ██████████░░░░░░░░░░  1/2 done  59s  finished with gaps",
            ]
            .join("\n")
        );
    }

    #[test]
    fn chips_are_only_ever_true() {
        // Small and healthy: no chip at all.
        let small = run("small", NOW - 5_000, 3, 1);
        let plain = render(&one(&small), 100, 1);
        assert!(
            !plain.contains('⚠') && !plain.contains("queued") && !plain.contains("failed"),
            "{plain}"
        );

        // Large only at the threshold, a failure count only after a failure,
        // queued only when the runtime reports waiting follow-ups.
        let mut large = run("large", NOW - 5_000, LARGE_WORKFLOW_AGENTS, 0);
        complete(&mut large, "large", 0, WorkflowRowStatus::Failed, None);
        let busy = render(
            &[WorkbarRun {
                panel: &large,
                queued: 2,
            }],
            120,
            1,
        );
        assert!(busy.contains("0/25 done · 1 failed"), "{busy}");
        assert!(busy.contains("⚠ Large workflow"), "{busy}");
        assert!(busy.ends_with("· 2 queued"), "{busy}");
        let under = run("under", NOW - 5_000, LARGE_WORKFLOW_AGENTS - 1, 0);
        let under = render(&one(&under), 120, 1);
        assert!(!under.contains("Large"), "{under}");
    }

    #[test]
    fn one_failure_in_a_large_run_still_gets_a_bar_cell() {
        assert_eq!(bar_cells(0, 1, 100), (0, 1));
        assert_eq!(bar_cells(99, 1, 100), (19, 1));
        assert_eq!(bar_cells(0, 2, 2), (0, BAR_CELLS));
        assert_eq!(bar_cells(3, 1, 4), (15, 5));
        assert_eq!(bar_cells(0, 0, 0), (0, 0));
    }

    #[test]
    fn many_runs_fold_into_a_more_row_that_names_the_key() {
        let panels: Vec<WorkflowPanel> = (0..12)
            .map(|index| run(&format!("wf-{index:02}"), NOW - 10_000, 8, index % 8))
            .collect();
        let runs: Vec<WorkbarRun<'_>> = panels
            .iter()
            .map(|panel| WorkbarRun { panel, queued: 0 })
            .collect();
        // Six run rows and the fold row; no rules.
        assert_eq!(desired_rows(runs.len()), MAX_RUN_ROWS as u16 + 1);
        let snapshot = render(&runs, 100, MAX_RUN_ROWS as u16 + 1);
        let rows: Vec<&str> = snapshot.lines().collect();
        assert_eq!(rows.len(), MAX_RUN_ROWS + 1);
        assert_eq!(rows[MAX_RUN_ROWS], " +6 more · ↓ to manage");
    }

    #[test]
    fn the_band_is_one_row_per_run_and_draws_no_rules() {
        let live = run("Audit the parser", NOW - 30_000, 4, 1);
        let done = settled("Port fixtures", WorkflowPanelLifecycle::Succeeded, 2, 2);
        let runs = [
            WorkbarRun {
                panel: &live,
                queued: 3,
            },
            WorkbarRun {
                panel: &done,
                queued: 0,
            },
        ];
        assert_eq!(desired_rows(runs.len()), 2);
        assert_eq!(desired_rows(0), 0);
        let rows = buffer_rows(&render_band(&runs, 90, desired_rows(runs.len())));
        assert_eq!(
            rows,
            vec![
                " • Audit the parser  █████░░░░░░░░░░░░░░░  1/4 done  30s  · 3 queued".to_string(),
                " ✓ Port fixtures     ████████████████████  2/2 done  59s".to_string(),
            ]
        );
        assert!(!rows.iter().any(|row| row.contains('─')), "{rows:?}");
    }

    /// NO_COLOR / 16-colour / ASCII terminals: every state still reads. Under
    /// monochrome the text is unchanged and each state keeps its own mark;
    /// at 16 colours the state inks stay apart; ASCII-safe marks stay apart
    /// except failed/stopped, which their words tell apart.
    #[test]
    fn states_read_on_no_color_sixteen_colour_and_ascii_terminals() {
        use crate::tui::color_compat::{adapt_cell_colors, adapt_cell_symbol_for_ascii};
        use codewhale_palette::{ColorDepth, PaletteMode, ThemeId};
        let theme = codewhale_palette::UI_THEME;
        let running = run("running", NOW - 10_000, 4, 1);
        let ok = settled("ok", WorkflowPanelLifecycle::Succeeded, 2, 2);
        let gaps = settled("gaps", WorkflowPanelLifecycle::Degraded, 2, 1);
        let failed = settled("failed", WorkflowPanelLifecycle::Failed, 2, 0);
        let stopped = settled("stopped", WorkflowPanelLifecycle::Cancelled, 2, 0);
        let panels = [&running, &ok, &gaps, &failed, &stopped];
        let runs: Vec<WorkbarRun<'_>> = panels
            .iter()
            .map(|panel| WorkbarRun { panel, queued: 0 })
            .collect();
        let source = render_band(&runs, 100, desired_rows(runs.len()));
        let text = buffer_rows(&source);
        let marks: Vec<String> = text[0..5]
            .iter()
            .map(|row| row.chars().nth(1).expect("mark").to_string())
            .collect();
        for (index, mark) in marks.iter().enumerate() {
            assert!(
                !marks[index + 1..].contains(mark),
                "state marks collide: {marks:?}"
            );
        }
        assert!(text[2].contains("finished with gaps"), "{text:?}");
        assert!(text[3].ends_with("failed"), "{text:?}");
        assert!(text[4].contains("stopped"), "{text:?}");

        let adapted = |depth: ColorDepth| {
            let mut buf = source.clone();
            for cell in buf.content.iter_mut() {
                adapt_cell_colors(cell, depth, PaletteMode::Dark, ThemeId::Whale, &theme, None);
            }
            buf
        };
        let mono = adapted(ColorDepth::Monochrome);
        assert_eq!(buffer_rows(&mono), text, "monochrome changes no text");
        assert!(
            mono.content.iter().all(|cell| {
                cell.fg == ratatui::style::Color::Reset && cell.bg == ratatui::style::Color::Reset
            }),
            "monochrome paints no colour"
        );
        let ansi16 = adapted(ColorDepth::Ansi16);
        let mark_ink: Vec<ratatui::style::Color> = (0..5).map(|y| ansi16[(1, y)].fg).collect();
        for (index, ink) in mark_ink.iter().enumerate() {
            assert!(
                !mark_ink[index + 1..].contains(ink),
                "16-colour state inks collide: {mark_ink:?}"
            );
        }

        let mut ascii = source.clone();
        for cell in ascii.content.iter_mut() {
            adapt_cell_symbol_for_ascii(cell);
        }
        let ascii_rows = buffer_rows(&ascii);
        assert!(
            ascii_rows.iter().all(|row| row.is_ascii()),
            "{ascii_rows:?}"
        );
        let ascii_marks: Vec<char> = ascii_rows[0..4]
            .iter()
            .map(|row| row.chars().nth(1).expect("mark"))
            .collect();
        for (index, mark) in ascii_marks.iter().enumerate() {
            assert!(
                !ascii_marks[index + 1..].contains(mark),
                "ASCII marks collide: {ascii_marks:?}"
            );
        }
    }

    #[test]
    fn seventy_five_agents_across_ten_runs_render_in_one_pass() {
        let panels: Vec<WorkflowPanel> = (0..10)
            .map(|index| run(&format!("wf-{index}"), NOW - 10_000, 8, 3))
            .collect();
        let runs: Vec<WorkbarRun<'_>> = panels
            .iter()
            .map(|panel| WorkbarRun { panel, queued: 0 })
            .collect();
        let started = std::time::Instant::now();
        for _ in 0..100 {
            let _ = lines(
                &runs,
                120,
                MAX_RUN_ROWS + 1,
                NOW,
                &codewhale_palette::UI_THEME,
                Locale::En,
            );
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "100 frames of 10 runs × 8 agents took {:?}",
            started.elapsed()
        );
    }
}
