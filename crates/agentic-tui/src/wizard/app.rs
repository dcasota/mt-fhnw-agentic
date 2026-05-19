//! Ratatui app that drives the wizard.
//!
//! Layout: vertical step-list on the left, current-step body on the right,
//! key-hint bar across the bottom.
//!
//! Keys (global):
//!   * `Esc` or `Ctrl-C` — quit (cancel)
//!   * `←` / `Backspace`-from-empty — step back
//!
//! Per-step keys are shown in the hint bar at the bottom.

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use rusqlite::Connection;

use super::state::{LANGS, PROJECT_KINDS, PROVIDERS, Step, WizardState, save_draft};

type Backend = CrosstermBackend<Stdout>;

/// Outcome returned to the caller.
#[derive(Debug)]
pub enum WizardOutcome {
    /// User reviewed and confirmed; project is ready to be created.
    Confirmed(Box<WizardState>),
    /// User pressed `Esc` / `Ctrl-C` before confirming.
    Cancelled,
}

/// Per-step UI cursors. Held by the App, not persisted.
#[derive(Debug, Default)]
struct Cursors {
    /// Current row index for the Kind, Lang and ProviderKeys list views.
    kind_idx: usize,
    lang_idx: usize,
    provider_idx: usize,
    /// `0` = institution, `1` = track on the InstitutionTrack step.
    inst_field: usize,
    /// When set, the wizard is editing a provider key for `PROVIDERS[idx]`.
    editing_provider: Option<usize>,
    /// Buffer for the provider-key edit field (kept in memory only).
    edit_buffer: String,
}

#[derive(Debug)]
pub struct App {
    state: WizardState,
    cursors: Cursors,
}

impl App {
    #[must_use]
    pub fn new(state: WizardState) -> Self {
        let cursors = Cursors {
            kind_idx: PROJECT_KINDS
                .iter()
                .position(|k| *k == state.kind)
                .unwrap_or(0),
            lang_idx: LANGS
                .iter()
                .position(|l| *l == state.working_lang)
                .unwrap_or(0),
            ..Cursors::default()
        };
        Self { state, cursors }
    }

    /// Run the wizard. Terminal setup + teardown happen here; the caller
    /// just hands over a DB connection for draft saves.
    pub fn run(mut self, conn: &Connection) -> Result<WizardOutcome> {
        enable_raw_mode().context("enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend).context("create terminal")?;

        let result = self.event_loop(&mut terminal, conn);

        // Always restore the terminal, even on error.
        disable_raw_mode().ok();
        execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
        terminal.show_cursor().ok();

        result
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<Backend>,
        conn: &Connection,
    ) -> Result<WizardOutcome> {
        loop {
            terminal.draw(|f| self.render(f))?;
            if !event::poll(Duration::from_millis(250))? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            // Global shortcuts.
            if matches!(key.code, KeyCode::Esc)
                || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                if self.cursors.editing_provider.is_some() {
                    self.cursors.editing_provider = None;
                    self.cursors.edit_buffer.clear();
                    continue;
                }
                return Ok(WizardOutcome::Cancelled);
            }

            if let Some(outcome) = self.handle_key(key) {
                save_draft(conn, &self.state).ok();
                return Ok(outcome);
            }
            // Auto-save the draft on every interaction.
            save_draft(conn, &self.state).ok();
        }
    }

    /// Dispatch a key by current step. Returns `Some(outcome)` when the
    /// wizard is complete / cancelled.
    fn handle_key(&mut self, key: KeyEvent) -> Option<WizardOutcome> {
        match self.state.current_step {
            Step::Welcome => self.handle_welcome(key),
            Step::Name => self.handle_name(key),
            Step::Kind => self.handle_kind(key),
            Step::Lang => self.handle_lang(key),
            Step::InstitutionTrack => self.handle_inst_track(key),
            Step::ProviderKeys => self.handle_provider_keys(key),
            Step::Review => self.handle_review(key),
            Step::Done => Some(WizardOutcome::Confirmed(Box::new(self.state.clone()))),
        }
    }

    // ----- step handlers ----------------------------------------------------

    fn handle_welcome(&mut self, key: KeyEvent) -> Option<WizardOutcome> {
        if matches!(key.code, KeyCode::Enter | KeyCode::Right) {
            self.state.current_step = Step::Name;
        }
        None
    }

    fn handle_name(&mut self, key: KeyEvent) -> Option<WizardOutcome> {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.state.project_name.push(c);
            }
            KeyCode::Backspace => {
                self.state.project_name.pop();
            }
            KeyCode::Enter => {
                if !self.state.project_name.trim().is_empty() {
                    self.state.current_step = Step::Kind;
                }
            }
            KeyCode::Left => {
                self.state.current_step = Step::Welcome;
            }
            _ => {}
        }
        None
    }

    fn handle_kind(&mut self, key: KeyEvent) -> Option<WizardOutcome> {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursors.kind_idx + 1 < PROJECT_KINDS.len() {
                    self.cursors.kind_idx += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursors.kind_idx > 0 {
                    self.cursors.kind_idx -= 1;
                }
            }
            KeyCode::Enter | KeyCode::Right => {
                self.state.kind = PROJECT_KINDS[self.cursors.kind_idx].to_owned();
                self.state.current_step = Step::Lang;
            }
            KeyCode::Left => {
                self.state.current_step = Step::Name;
            }
            _ => {}
        }
        None
    }

    fn handle_lang(&mut self, key: KeyEvent) -> Option<WizardOutcome> {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursors.lang_idx + 1 < LANGS.len() {
                    self.cursors.lang_idx += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursors.lang_idx > 0 {
                    self.cursors.lang_idx -= 1;
                }
            }
            KeyCode::Enter | KeyCode::Right => {
                self.state.working_lang = LANGS[self.cursors.lang_idx].to_owned();
                self.state.current_step = Step::InstitutionTrack;
            }
            KeyCode::Left => {
                self.state.current_step = Step::Kind;
            }
            _ => {}
        }
        None
    }

    fn handle_inst_track(&mut self, key: KeyEvent) -> Option<WizardOutcome> {
        let field: &mut String = if self.cursors.inst_field == 0 {
            &mut self.state.institution
        } else {
            &mut self.state.track
        };
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                field.push(c);
            }
            KeyCode::Backspace => {
                field.pop();
            }
            KeyCode::Tab | KeyCode::Down => {
                self.cursors.inst_field = (self.cursors.inst_field + 1) % 2;
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.cursors.inst_field = (self.cursors.inst_field + 1) % 2;
            }
            KeyCode::Enter => {
                self.state.current_step = Step::ProviderKeys;
            }
            KeyCode::Left => {
                self.state.current_step = Step::Lang;
            }
            _ => {}
        }
        None
    }

    fn handle_provider_keys(&mut self, key: KeyEvent) -> Option<WizardOutcome> {
        if let Some(idx) = self.cursors.editing_provider {
            // In edit mode for a single provider's key.
            match key.code {
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.cursors.edit_buffer.push(c);
                }
                KeyCode::Backspace => {
                    self.cursors.edit_buffer.pop();
                }
                KeyCode::Enter => {
                    let key_val = std::mem::take(&mut self.cursors.edit_buffer);
                    if !key_val.trim().is_empty() {
                        self.state
                            .set_key(PROVIDERS[idx], key_val.trim().to_owned());
                    }
                    self.cursors.editing_provider = None;
                }
                _ => {}
            }
            return None;
        }
        // Browse mode.
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursors.provider_idx + 1 < PROVIDERS.len() {
                    self.cursors.provider_idx += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursors.provider_idx > 0 {
                    self.cursors.provider_idx -= 1;
                }
            }
            KeyCode::Enter => {
                self.cursors.editing_provider = Some(self.cursors.provider_idx);
                self.cursors.edit_buffer.clear();
            }
            KeyCode::Char('d') => {
                self.state.forget_key(PROVIDERS[self.cursors.provider_idx]);
            }
            KeyCode::Char('s') | KeyCode::Right => {
                self.state.current_step = Step::Review;
            }
            KeyCode::Left => {
                self.state.current_step = Step::InstitutionTrack;
            }
            _ => {}
        }
        None
    }

    fn handle_review(&mut self, key: KeyEvent) -> Option<WizardOutcome> {
        match key.code {
            KeyCode::Enter => {
                if self.state.is_complete() {
                    self.state.current_step = Step::Done;
                    return Some(WizardOutcome::Confirmed(Box::new(self.state.clone())));
                }
            }
            KeyCode::Left | KeyCode::Char('b') => {
                self.state.current_step = Step::ProviderKeys;
            }
            _ => {}
        }
        None
    }

    // ----- rendering --------------------------------------------------------

    fn render(&self, f: &mut Frame<'_>) {
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(2)])
            .split(f.area());

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(0)])
            .split(outer[0]);

        self.render_step_list(f, body[0]);
        self.render_step_body(f, body[1]);
        self.render_hints(f, outer[1]);
    }

    fn render_step_list(&self, f: &mut Frame<'_>, area: Rect) {
        let items: Vec<ListItem<'_>> = Step::all()
            .iter()
            .map(|s| {
                let marker = if *s == self.state.current_step {
                    "▶ "
                } else {
                    "  "
                };
                let style = if *s == self.state.current_step {
                    Style::default()
                        .add_modifier(Modifier::BOLD)
                        .fg(Color::Cyan)
                } else {
                    Style::default()
                };
                ListItem::new(format!("{marker}{}", s.label())).style(style)
            })
            .collect();
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" agentic init "),
        );
        f.render_widget(list, area);
    }

    fn render_step_body(&self, f: &mut Frame<'_>, area: Rect) {
        match self.state.current_step {
            Step::Welcome => self.render_welcome(f, area),
            Step::Name => self.render_name(f, area),
            Step::Kind => self.render_kind(f, area),
            Step::Lang => self.render_lang(f, area),
            Step::InstitutionTrack => self.render_inst_track(f, area),
            Step::ProviderKeys => self.render_provider_keys(f, area),
            Step::Review | Step::Done => self.render_review(f, area),
        }
    }

    fn render_welcome(&self, f: &mut Frame<'_>, area: Rect) {
        let text = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Welcome to agentic init.",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("This wizard will set up a thesis-scale project:"),
            Line::from("  • project name, kind, working language"),
            Line::from("  • institution + (optional) track"),
            Line::from("  • API keys for any LLM providers you plan to use"),
            Line::from(""),
            Line::from("Anything you skip can be added later via the CLI."),
            Line::from(""),
            Line::from("Press Enter to start."),
        ];
        let p = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(" Welcome "))
            .wrap(Wrap { trim: false });
        f.render_widget(p, area);
    }

    fn render_name(&self, f: &mut Frame<'_>, area: Rect) {
        let cursor = if self.state.project_name.is_empty() {
            "▏"
        } else {
            "▏"
        };
        let text = vec![
            Line::from(""),
            Line::from(
                "Give your project a name. This is what shows up in `agentic project list`.",
            ),
            Line::from(""),
            Line::from(Span::styled(
                "Examples:",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  \"MAS Thesis: Agentic AI for Research\"",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Name: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(&self.state.project_name),
                Span::styled(cursor, Style::default().fg(Color::Cyan)),
            ]),
        ];
        let p = Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" 1 / 6  Project name "),
            )
            .wrap(Wrap { trim: false });
        f.render_widget(p, area);
    }

    fn render_kind(&self, f: &mut Frame<'_>, area: Rect) {
        let items: Vec<ListItem<'_>> = PROJECT_KINDS
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let style = if i == self.cursors.kind_idx {
                    Style::default()
                        .add_modifier(Modifier::REVERSED)
                        .fg(Color::Cyan)
                } else {
                    Style::default()
                };
                let descr = match *k {
                    "thesis" => "Single master/PhD thesis",
                    "sub_paper" => "Paper inside a portfolio",
                    "standalone" => "One-off research note",
                    "portfolio_root" => "Container for several sub-papers",
                    _ => "",
                };
                ListItem::new(format!("  {k:<16} {descr}")).style(style)
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.cursors.kind_idx));
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" 2 / 6  Project kind "),
        );
        f.render_stateful_widget(list, area, &mut state);
    }

    fn render_lang(&self, f: &mut Frame<'_>, area: Rect) {
        let descr = |l: &str| match l {
            "en" => "English",
            "de" => "Deutsch",
            "fr" => "Français",
            "it" => "Italiano",
            "rm" => "Rumantsch",
            "hi" => "हिन्दी",
            _ => "",
        };
        let items: Vec<ListItem<'_>> = LANGS
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let style = if i == self.cursors.lang_idx {
                    Style::default()
                        .add_modifier(Modifier::REVERSED)
                        .fg(Color::Cyan)
                } else {
                    Style::default()
                };
                ListItem::new(format!("  {l}   {}", descr(l))).style(style)
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.cursors.lang_idx));
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" 3 / 6  Working language "),
        );
        f.render_stateful_widget(list, area, &mut state);
    }

    fn render_inst_track(&self, f: &mut Frame<'_>, area: Rect) {
        let active_inst = self.cursors.inst_field == 0;
        let inst_marker = if active_inst { "▶" } else { " " };
        let track_marker = if !active_inst { "▶" } else { " " };
        let text = vec![
            Line::from(""),
            Line::from("Optional institutional context. Used for templates and overrides."),
            Line::from(""),
            Line::from(vec![
                Span::raw(format!("{inst_marker} ")),
                Span::styled(
                    "Institution: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(&self.state.institution),
                if active_inst {
                    Span::styled("▏", Style::default().fg(Color::Cyan))
                } else {
                    Span::raw("")
                },
            ]),
            Line::from(vec![
                Span::raw(format!("{track_marker} ")),
                Span::styled(
                    "Track:       ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(&self.state.track),
                if !active_inst {
                    Span::styled("▏", Style::default().fg(Color::Cyan))
                } else {
                    Span::raw("")
                },
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Examples: institution=\"fhnw-mas\", track=\"lincyber\" / \"dlinit\"",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let p = Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" 4 / 6  Institution & track "),
            )
            .wrap(Wrap { trim: false });
        f.render_widget(p, area);
    }

    fn render_provider_keys(&self, f: &mut Frame<'_>, area: Rect) {
        if let Some(idx) = self.cursors.editing_provider {
            // Edit-one-key sub-screen.
            let mask: String = "•".repeat(self.cursors.edit_buffer.chars().count());
            let text = vec![
                Line::from(""),
                Line::from(format!("Enter API key for {}.", PROVIDERS[idx])),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Key: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(mask),
                    Span::styled("▏", Style::default().fg(Color::Cyan)),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "Enter saves and returns. Esc cancels.",
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            let p = Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" 5 / 6  Provider keys → {} ", PROVIDERS[idx])),
                )
                .wrap(Wrap { trim: false });
            f.render_widget(p, area);
            return;
        }
        // Browse-all-providers view.
        let items: Vec<ListItem<'_>> = PROVIDERS
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let has = self.state.providers_keyed.contains(&i);
                let mark = if has { "[set]    " } else { "[skipped]" };
                let mark_style = if has {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let row_style = if i == self.cursors.provider_idx {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(mark, mark_style),
                    Span::raw("  "),
                    Span::raw(*p),
                ]))
                .style(row_style)
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.cursors.provider_idx));
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" 5 / 6  Provider keys "),
        );
        f.render_stateful_widget(list, area, &mut state);
    }

    fn render_review(&self, f: &mut Frame<'_>, area: Rect) {
        let configured: Vec<&str> = self
            .state
            .providers_keyed
            .iter()
            .filter_map(|i| PROVIDERS.get(*i).copied())
            .collect();
        let configured_line = if configured.is_empty() {
            "  (none — you can run `agentic config set-key <provider> <value>` later)".to_owned()
        } else {
            format!("  {}", configured.join(", "))
        };
        let track_line = if self.state.track.is_empty() {
            "(none)".to_owned()
        } else {
            self.state.track.clone()
        };
        let text = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Review your answers before I create the project.",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(format!("  Name:         {}", self.state.project_name)),
            Line::from(format!("  Kind:         {}", self.state.kind)),
            Line::from(format!("  Language:     {}", self.state.working_lang)),
            Line::from(format!("  Institution:  {}", self.state.institution)),
            Line::from(format!("  Track:        {track_line}")),
            Line::from("  Provider keys:"),
            Line::from(configured_line),
            Line::from(""),
            if self.state.is_complete() {
                Line::from(Span::styled(
                    "Enter creates the project. Left arrow goes back.",
                    Style::default().fg(Color::Green),
                ))
            } else {
                Line::from(Span::styled(
                    "Missing: name/kind/language. Press Left arrow to go back.",
                    Style::default().fg(Color::Red),
                ))
            },
        ];
        let p = Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" 6 / 6  Review "),
            )
            .wrap(Wrap { trim: false });
        f.render_widget(p, area);
    }

    fn render_hints(&self, f: &mut Frame<'_>, area: Rect) {
        let hint = match self.state.current_step {
            Step::Welcome => "Enter: start  •  Esc: quit",
            Step::Name => "Type name  •  Enter: next  •  ←: back  •  Esc: quit",
            Step::Kind | Step::Lang => "↑/↓: select  •  Enter: next  •  ←: back  •  Esc: quit",
            Step::InstitutionTrack => {
                "Type field  •  Tab: switch field  •  Enter: next  •  ←: back  •  Esc: quit"
            }
            Step::ProviderKeys => {
                if self.cursors.editing_provider.is_some() {
                    "Type key  •  Enter: save  •  Esc: cancel"
                } else {
                    "↑/↓: pick  •  Enter: set key  •  d: delete  •  s: skip rest  •  ←: back  •  Esc: quit"
                }
            }
            Step::Review => "Enter: create  •  ←: back  •  Esc: quit",
            Step::Done => "Creating…",
        };
        let p = Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        )))
        .block(Block::default().borders(Borders::TOP));
        f.render_widget(p, area);
    }
}
