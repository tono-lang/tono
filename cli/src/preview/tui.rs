//! The preview's terminal shell: a split-pane view (source on the left, generated
//! code on the right) with a status line carrying the compile verdict.
//!
//! The pipeline pass is slow (it shells out to a real compiler), so it runs on a
//! worker thread and the UI stays responsive: the main loop draws, handles keys,
//! and watches the source file's mtime, kicking off a fresh pass on save (or a
//! target switch) and swapping in the result when the worker reports back.
//!
//! Keys: `q`/`Esc` quit, `Tab` cycles the target, `j`/`k` (or the arrows) scroll
//! the generated pane, `r` forces a refresh. No `Alt`-chords (they collide with
//! tiling window managers).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, SystemTime};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use tono_backend::codegen::TargetKind;

use crate::preview::highlight;
use crate::preview::pipeline::{self, Snapshot, Verdict};

/// How long the main loop blocks waiting for input before looping to poll the
/// worker channel and the file's mtime. Short enough to feel live, long enough to
/// leave the CPU idle.
const TICK: Duration = Duration::from_millis(150);

/// Launch the preview UI for `source_path`, starting on the first of `targets`
/// (`Tab` cycles through them in the order given). `scratch_base` is the parent
/// of the per-target throwaway build directories. Returns when the user quits.
pub fn run(
    source_path: PathBuf,
    targets: Vec<TargetKind>,
    scratch_base: PathBuf,
) -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new(source_path, targets, scratch_base);
    app.trigger_refresh();
    let result = app.event_loop(&mut terminal);
    ratatui::restore();
    result
}

/// The live preview state: what to draw, where the generated pane is scrolled, and
/// the machinery for the background pass (the channel it reports on, and whether
/// one is in flight).
struct App {
    source_path: PathBuf,
    /// The targets `Tab` cycles through, in the order the user requested them.
    targets: Vec<TargetKind>,
    target: TargetKind,
    scratch_base: PathBuf,
    snapshot: Option<Snapshot>,
    /// The panes' rendered text, highlighted once per snapshot (highlighting per
    /// frame would parse the whole buffer 60 times a second for nothing).
    source_view: Text<'static>,
    generated_view: Text<'static>,
    scroll: u16,
    refreshing: bool,
    /// The source mtime at the last refresh, so an edit is detected as a change.
    watched_mtime: Option<SystemTime>,
    tx: Sender<Snapshot>,
    rx: Receiver<Snapshot>,
}

impl App {
    fn new(source_path: PathBuf, targets: Vec<TargetKind>, scratch_base: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let target = targets.first().copied().unwrap_or(TargetKind::Rust);
        Self {
            source_path,
            targets,
            target,
            scratch_base,
            snapshot: None,
            source_view: Text::default(),
            generated_view: Text::default(),
            scroll: 0,
            refreshing: false,
            watched_mtime: None,
            tx,
            rx,
        }
    }

    fn event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<()> {
        loop {
            terminal.draw(|frame| self.draw(frame))?;

            if event::poll(TICK)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press && self.on_key(key.code) == Flow::Quit {
                        return Ok(());
                    }
                }
            }

            self.drain_worker();
            self.watch_for_edits();
        }
    }

    /// Apply a keypress; returns whether the loop should quit.
    fn on_key(&mut self, code: KeyCode) -> Flow {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return Flow::Quit,
            KeyCode::Tab => self.cycle_target(),
            KeyCode::Char('r') => self.trigger_refresh(),
            KeyCode::Char('j') | KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            _ => {}
        }
        Flow::Continue
    }

    /// Advance to the next requested target and re-render. The generated pane
    /// resets to the top because it now holds different code. With a single
    /// target the cycle is a refresh of the same one.
    fn cycle_target(&mut self) {
        let next = self
            .targets
            .iter()
            .position(|&t| t == self.target)
            .unwrap_or(0);
        self.target = self.targets[(next + 1) % self.targets.len()];
        self.scroll = 0;
        self.trigger_refresh();
    }

    /// Start a background pass for the current source and target. Marks the pass in
    /// flight so the status line can say so and so a second pass is not launched
    /// while one runs.
    fn trigger_refresh(&mut self) {
        self.refreshing = true;
        self.watched_mtime = current_mtime(&self.source_path);
        let tx = self.tx.clone();
        let source = self.source_path.clone();
        let target = self.target;
        let scratch = self.scratch_base.join(target.dir());
        // A fresh frontend handle per pass: it only holds the resolved program
        // name, so this is cheap and keeps the worker self-contained.
        std::thread::spawn(move || {
            let frontend = crate::frontend::Frontend::from_env();
            let snapshot = pipeline::run(&frontend, &source, target, &scratch);
            // The receiver is gone only when the app is quitting; dropping the
            // result then is fine.
            let _ = tx.send(snapshot);
        });
    }

    /// Swap in any finished pass. Only the latest is kept: an intervening edit may
    /// have queued several, and the freshest wins.
    fn drain_worker(&mut self) {
        let mut fresh = None;
        while let Ok(snapshot) = self.rx.try_recv() {
            fresh = Some(snapshot);
            self.refreshing = false;
        }
        if let Some(snapshot) = fresh {
            // The generated pane is highlighted with the snapshot's own target
            // (the user may have already cycled `self.target` onward).
            self.source_view = highlight::source(&expand_tabs(&snapshot.source));
            self.generated_view =
                highlight::generated(&expand_tabs(&snapshot.generated), snapshot.target);
            self.snapshot = Some(snapshot);
        }
    }

    /// Refresh when the source file changed on disk since the last pass, so saving
    /// in another editor updates the preview. Skipped while a pass is in flight; the
    /// next tick re-checks and catches an edit made during a build.
    fn watch_for_edits(&mut self) {
        if self.refreshing {
            return;
        }
        let now = current_mtime(&self.source_path);
        if now != self.watched_mtime {
            self.trigger_refresh();
        }
    }

    fn draw(&self, frame: &mut Frame) {
        let [body, status] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(frame.area());
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(body);

        self.draw_source(frame, left);
        self.draw_right(frame, right);
        self.draw_status(frame, status);
    }

    fn draw_source(&self, frame: &mut Frame, area: Rect) {
        let title = format!(" source: {} ", self.source_path.display());
        frame.render_widget(
            Paragraph::new(self.source_view.clone()).block(Block::bordered().title(title)),
            area,
        );
    }

    /// The right column: the generated code, and beneath it the diagnostics panel
    /// when the verdict carries any (a rejected source or a rejected build).
    fn draw_right(&self, frame: &mut Frame, area: Rect) {
        let detail = self.snapshot.as_ref().and_then(|s| s.verdict.detail());
        match detail {
            Some(diagnostics) => {
                let [code, diag] =
                    Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)])
                        .areas(area);
                self.draw_generated(frame, code);
                frame.render_widget(
                    Paragraph::new(expand_tabs(diagnostics))
                        .block(Block::bordered().title(" diagnostics "))
                        .wrap(Wrap { trim: false }),
                    diag,
                );
            }
            None => self.draw_generated(frame, area),
        }
    }

    fn draw_generated(&self, frame: &mut Frame, area: Rect) {
        let title = format!(" generated: {} ", self.target.dir());
        frame.render_widget(
            Paragraph::new(self.generated_view.clone())
                .block(Block::bordered().title(title))
                .scroll((self.scroll, 0)),
            area,
        );
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let verdict_span = if self.refreshing {
            Span::styled("checking...", Style::default().fg(Color::Yellow))
        } else if let Some(snapshot) = &self.snapshot {
            Span::styled(
                snapshot.verdict.headline(),
                Style::default()
                    .fg(verdict_color(&snapshot.verdict))
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("starting...")
        };

        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", self.target.dir()),
                Style::default().fg(Color::Black).bg(Color::Cyan),
            ),
            Span::raw("  "),
            verdict_span,
        ]);
        let hints = Line::from(Span::styled(
            "q quit  Tab target  j/k scroll  r refresh",
            Style::default().fg(Color::DarkGray),
        ));
        frame.render_widget(
            Paragraph::new(vec![line, hints]).block(Block::bordered().title(" tono preview ")),
            area,
        );
    }
}

/// Whether the event loop should keep running or quit.
#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Quit,
}

/// The status color for a verdict: green when it compiles, red when something was
/// rejected (the source or the build), yellow when a verdict could not be reached
/// (a missing toolchain or frontend, or a setup error).
fn verdict_color(verdict: &Verdict) -> Color {
    match verdict {
        Verdict::Compiles => Color::Green,
        Verdict::SourceError(_) | Verdict::DoesNotCompile(_) | Verdict::GenerateRejected(_) => {
            Color::Red
        }
        Verdict::SourceUnreadable(_)
        | Verdict::FrontendUnavailable(_)
        | Verdict::IrError(_)
        | Verdict::ToolchainMissing(_)
        | Verdict::CheckError(_) => Color::Yellow,
    }
}

/// The source file's last-modified time, or `None` if it cannot be read (a
/// transient during a save); an unreadable file simply defers the next refresh.
fn current_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Replace tabs with spaces before rendering: ratatui's `Paragraph` does not lay
/// tabs out (they render zero-width, overwriting neighboring text), and gofmt'd
/// Go is tab-indented. A fixed four spaces per tab keeps nesting readable; column
/// alignment is not worth emulating in a preview pane.
fn expand_tabs(text: &str) -> String {
    text.replace('\t', "    ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_are_expanded_for_rendering() {
        assert_eq!(expand_tabs("\tLast4 string\n"), "    Last4 string\n");
        assert_eq!(expand_tabs("no tabs"), "no tabs");
    }

    #[test]
    fn each_verdict_has_a_color() {
        // The palette groups verdicts into three signals; spot-check one of each.
        assert_eq!(verdict_color(&Verdict::Compiles), Color::Green);
        assert_eq!(
            verdict_color(&Verdict::DoesNotCompile("x".into())),
            Color::Red
        );
        assert_eq!(
            verdict_color(&Verdict::ToolchainMissing("cargo".into())),
            Color::Yellow
        );
    }

    #[test]
    fn cycling_the_target_rotates_and_returns_to_the_start() {
        let mut app = App::new(
            PathBuf::from("x.tono"),
            vec![TargetKind::Rust, TargetKind::Go, TargetKind::TypeScript],
            std::env::temp_dir(),
        );
        // Draining the refresh each cycle triggers keeps the channel from filling;
        // we only assert the target rotation here.
        app.cycle_target();
        assert_eq!(app.target, TargetKind::Go);
        app.cycle_target();
        assert_eq!(app.target, TargetKind::TypeScript);
        app.cycle_target();
        assert_eq!(app.target, TargetKind::Rust);
    }

    #[test]
    fn scrolling_up_from_the_top_stays_at_zero() {
        let mut app = App::new(
            PathBuf::from("x.tono"),
            vec![TargetKind::Rust, TargetKind::Go, TargetKind::TypeScript],
            std::env::temp_dir(),
        );
        assert_eq!(app.on_key(KeyCode::Char('k')), Flow::Continue);
        assert_eq!(app.scroll, 0);
        app.on_key(KeyCode::Char('j'));
        app.on_key(KeyCode::Char('j'));
        assert_eq!(app.scroll, 2);
        app.on_key(KeyCode::Up);
        assert_eq!(app.scroll, 1);
    }

    #[test]
    fn q_and_esc_quit() {
        let mut app = App::new(
            PathBuf::from("x.tono"),
            vec![TargetKind::Rust, TargetKind::Go, TargetKind::TypeScript],
            std::env::temp_dir(),
        );
        assert_eq!(app.on_key(KeyCode::Char('q')), Flow::Quit);
        assert_eq!(app.on_key(KeyCode::Esc), Flow::Quit);
        assert_eq!(app.on_key(KeyCode::Char('x')), Flow::Continue);
    }
}
