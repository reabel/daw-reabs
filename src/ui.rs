use std::{
    io,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table, TableState,
    },
    Frame, Terminal,
};

use crate::{
    project::Project,
    track::Track,
};

// ── Tick rate for the TUI event loop ─────────────────────────────────────────
const TICK_MS: u64 = 50; // 20 fps

// ─────────────────────────────────────────────────────────────────────────────
// Application state
// ─────────────────────────────────────────────────────────────────────────────

/// Playback state tracked purely inside the TUI (no audio hardware).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Playing,
    Recording,
}

/// Which panel currently has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tracks,
    Timeline,
}

/// The top-level TUI application.
pub struct App {
    pub project: Project,
    /// Simulated playhead position in samples.
    pub playhead: u64,
    pub play_state: PlayState,
    /// Wall-clock instant when play was last started (for advancing playhead).
    play_started_at: Option<Instant>,
    /// Playhead position when play was started.
    play_started_pos: u64,
    /// Index of the selected track.
    pub selected_track: usize,
    /// Horizontal scroll offset of the timeline in samples.
    pub timeline_offset: u64,
    /// Pixels-per-sample zoom (samples shown per terminal column).
    pub samples_per_col: u64,
    focus: Focus,
    /// Show the help overlay.
    show_help: bool,
    /// Whether the app should quit.
    pub should_quit: bool,
    /// Scroll state for the track table scrollbar.
    track_scroll: ScrollbarState,
}

impl App {
    pub fn new(project: Project) -> Self {
        let samples_per_col = (project.sample_rate as u64).max(1); // 1 col = 1 second at default zoom
        Self {
            project,
            playhead: 0,
            play_state: PlayState::Stopped,
            play_started_at: None,
            play_started_pos: 0,
            selected_track: 0,
            timeline_offset: 0,
            samples_per_col,
            focus: Focus::Tracks,
            show_help: false,
            should_quit: false,
            track_scroll: ScrollbarState::default(),
        }
    }

    // ── Playback control ───────────────────────────────────────────────────

    pub fn toggle_play(&mut self) {
        match self.play_state {
            PlayState::Playing | PlayState::Recording => self.stop(),
            PlayState::Stopped => self.play(),
        }
    }

    pub fn play(&mut self) {
        self.play_state = PlayState::Playing;
        self.play_started_at = Some(Instant::now());
        self.play_started_pos = self.playhead;
    }

    pub fn stop(&mut self) {
        self.play_state = PlayState::Stopped;
        self.play_started_at = None;
    }

    pub fn record(&mut self) {
        self.play_state = PlayState::Recording;
        self.play_started_at = Some(Instant::now());
        self.play_started_pos = self.playhead;
    }

    pub fn rewind(&mut self) {
        self.stop();
        self.playhead = 0;
    }

    /// Advance the simulated playhead based on elapsed wall-clock time.
    pub fn tick(&mut self) {
        if let Some(started) = self.play_started_at {
            if self.play_state != PlayState::Stopped {
                let elapsed = started.elapsed().as_secs_f64();
                let new_pos =
                    self.play_started_pos + (elapsed * self.project.sample_rate as f64) as u64;
                let total = self.project.length_samples();
                if total > 0 && new_pos >= total {
                    self.playhead = 0;
                    self.stop();
                } else {
                    self.playhead = new_pos;
                }
            }
        }
    }

    // ── Track editing ──────────────────────────────────────────────────────

    pub fn selected_track_mut(&mut self) -> Option<&mut Track> {
        self.project.tracks.get_mut(self.selected_track)
    }

    pub fn toggle_mute(&mut self) {
        if let Some(t) = self.selected_track_mut() {
            t.muted = !t.muted;
        }
    }

    pub fn toggle_solo(&mut self) {
        if let Some(t) = self.selected_track_mut() {
            t.soloed = !t.soloed;
        }
    }

    pub fn toggle_arm(&mut self) {
        if let Some(t) = self.selected_track_mut() {
            t.arm = !t.arm;
        }
    }

    pub fn volume_up(&mut self) {
        if let Some(t) = self.selected_track_mut() {
            t.volume = (t.volume + 0.05).min(2.0);
        }
    }

    pub fn volume_down(&mut self) {
        if let Some(t) = self.selected_track_mut() {
            t.volume = (t.volume - 0.05).max(0.0);
        }
    }

    pub fn pan_left(&mut self) {
        if let Some(t) = self.selected_track_mut() {
            t.pan = (t.pan - 0.05).max(-1.0);
        }
    }

    pub fn pan_right(&mut self) {
        if let Some(t) = self.selected_track_mut() {
            t.pan = (t.pan + 0.05).min(1.0);
        }
    }

    // ── Navigation ─────────────────────────────────────────────────────────

    pub fn track_up(&mut self) {
        if self.selected_track > 0 {
            self.selected_track -= 1;
        }
        self.track_scroll = self.track_scroll.position(self.selected_track);
    }

    pub fn track_down(&mut self) {
        if self.selected_track + 1 < self.project.tracks.len() {
            self.selected_track += 1;
        }
        self.track_scroll = self.track_scroll.position(self.selected_track);
    }

    pub fn zoom_in(&mut self) {
        self.samples_per_col = (self.samples_per_col / 2).max(1);
    }

    pub fn zoom_out(&mut self) {
        self.samples_per_col = (self.samples_per_col * 2)
            .min(self.project.sample_rate as u64 * 3600);
    }

    pub fn scroll_left(&mut self) {
        let step = self.samples_per_col * 10;
        self.timeline_offset = self.timeline_offset.saturating_sub(step);
    }

    pub fn scroll_right(&mut self) {
        let step = self.samples_per_col * 10;
        self.timeline_offset += step;
    }

    /// Make sure the playhead is visible in the timeline.
    pub fn follow_playhead(&mut self, timeline_cols: u16) {
        let tl_end = self.timeline_offset + self.samples_per_col * timeline_cols as u64;
        if self.playhead >= tl_end {
            self.timeline_offset = self
                .playhead
                .saturating_sub(self.samples_per_col * (timeline_cols as u64 / 4));
        }
        if self.playhead < self.timeline_offset {
            self.timeline_offset = self.playhead;
        }
    }

    // ── Formatting helpers ─────────────────────────────────────────────────

    /// Format a sample position as `mm:ss.cc` (centiseconds).
    pub fn format_pos(&self, samples: u64) -> String {
        let secs = samples / self.project.sample_rate as u64;
        let cs = (samples % self.project.sample_rate as u64) * 100
            / self.project.sample_rate as u64;
        let m = secs / 60;
        let s = secs % 60;
        format!("{m:02}:{s:02}.{cs:02}")
    }

    pub fn bpm_str(&self) -> String {
        format!("{:.1} BPM", self.project.bpm)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

pub fn run(project: Project) -> Result<()> {
    // Set up terminal.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(project);
    let tick = Duration::from_millis(TICK_MS);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        let timeout = tick.checked_sub(last_tick.elapsed()).unwrap_or(Duration::ZERO);

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                handle_key(&mut app, key.code, key.modifiers);
            }
        }

        if last_tick.elapsed() >= tick {
            app.tick();
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal.
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Key handling
// ─────────────────────────────────────────────────────────────────────────────

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    if app.show_help {
        app.show_help = false;
        return;
    }

    match code {
        // Quit
        KeyCode::Char('q') | KeyCode::Char('Q') => app.should_quit = true,
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => app.should_quit = true,

        // Transport
        KeyCode::Char(' ') => app.toggle_play(),
        KeyCode::Char('r') | KeyCode::Char('R') => app.record(),
        KeyCode::Home => app.rewind(),

        // Track navigation
        KeyCode::Up | KeyCode::Char('k') => app.track_up(),
        KeyCode::Down | KeyCode::Char('j') => app.track_down(),

        // Track controls
        KeyCode::Char('m') => app.toggle_mute(),
        KeyCode::Char('s') => app.toggle_solo(),
        KeyCode::Char('a') => app.toggle_arm(),
        // Volume
        KeyCode::Char('+') | KeyCode::Char('=') => app.volume_up(),
        KeyCode::Char('-') => app.volume_down(),
        // Pan
        KeyCode::Char('[') => app.pan_left(),
        KeyCode::Char(']') => app.pan_right(),

        // Timeline navigation
        KeyCode::Left | KeyCode::Char('h') => app.scroll_left(),
        KeyCode::Right | KeyCode::Char('l') => app.scroll_right(),
        // Zoom
        KeyCode::Char('i') => app.zoom_in(),
        KeyCode::Char('o') => app.zoom_out(),

        // Focus toggle
        KeyCode::Tab => {}

        // Help
        KeyCode::Char('?') => app.show_help = true,

        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Drawing
// ─────────────────────────────────────────────────────────────────────────────

fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // ── Outer layout: transport / main / status ───────────────────────────
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // transport bar
            Constraint::Min(5),    // tracks + timeline
            Constraint::Length(1), // status line
        ])
        .split(area);

    draw_transport(f, app, outer[0]);

    // ── Middle: track list | timeline ─────────────────────────────────────
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(28), // track list
            Constraint::Min(10),    // timeline
        ])
        .split(outer[1]);

    app.follow_playhead(mid[1].width.saturating_sub(2));
    draw_track_list(f, app, mid[0]);
    draw_timeline(f, app, mid[1]);

    draw_status(f, app, outer[2]);

    if app.show_help {
        draw_help(f, area);
    }
}

// ── Transport bar ─────────────────────────────────────────────────────────────

fn draw_transport(f: &mut Frame, app: &App, area: Rect) {
    let state_icon = match app.play_state {
        PlayState::Stopped => Span::styled("■ STOP", Style::default().fg(Color::DarkGray)),
        PlayState::Playing => Span::styled("▶ PLAY", Style::default().fg(Color::Green)),
        PlayState::Recording => {
            Span::styled("⏺ REC ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
        }
    };

    let pos_str = app.format_pos(app.playhead);
    let total_str = app.format_pos(app.project.length_samples());
    let bpm_str = app.bpm_str();
    let sig_str = format!(
        "{}/{}",
        app.project.time_sig_numerator, app.project.time_sig_denominator
    );

    let progress = if app.project.length_samples() > 0 {
        app.playhead as f64 / app.project.length_samples() as f64
    } else {
        0.0
    };

    // Split transport area into: state | position | gauge | bpm/sig
    let _chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(10), // state
            Constraint::Length(20), // position text
            Constraint::Min(10),    // progress gauge
            Constraint::Length(20), // bpm + sig
        ])
        .split(area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" {} ", app.project.name),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    f.render_widget(block, area);

    // State text (inside bordered block, offset by 1)
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    let transport_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(8),  // state
            Constraint::Length(22), // pos
            Constraint::Min(10),    // gauge
            Constraint::Length(22), // bpm/sig
        ])
        .split(inner);

    f.render_widget(Paragraph::new(state_icon), transport_chunks[0]);

    f.render_widget(
        Paragraph::new(format!("{pos_str} / {total_str}")),
        transport_chunks[1],
    );

    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(progress.clamp(0.0, 1.0));
    f.render_widget(gauge, transport_chunks[2]);

    f.render_widget(
        Paragraph::new(format!(" {bpm_str}  {sig_str}")),
        transport_chunks[3],
    );
}

// ── Track list ────────────────────────────────────────────────────────────────

fn draw_track_list(f: &mut Frame, app: &mut App, area: Rect) {
    let header = Row::new(vec![
        Cell::from("Track").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Vol").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Pan").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Flg").style(Style::default().add_modifier(Modifier::BOLD)),
    ])
    .style(Style::default().fg(Color::Yellow))
    .height(1);

    let rows: Vec<Row> = app
        .project
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let is_sel = i == app.selected_track;
            let name_style = if is_sel {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let vol_pct = (t.volume * 100.0).round() as u8;
            let pan_str = if t.pan.abs() < 0.01 {
                "  C  ".to_string()
            } else if t.pan < 0.0 {
                format!("L{:.0} ", (-t.pan * 100.0).round())
            } else {
                format!(" R{:.0}", (t.pan * 100.0).round())
            };

            let mut flags = String::new();
            if t.muted { flags.push('M'); } else { flags.push(' '); }
            if t.soloed { flags.push('S'); } else { flags.push(' '); }
            if t.arm { flags.push('●'); } else { flags.push(' '); }

            let flag_style = if t.muted {
                Style::default().fg(Color::Red)
            } else if t.soloed {
                Style::default().fg(Color::Yellow)
            } else if t.arm {
                Style::default().fg(Color::Magenta)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let name = if t.name.len() > 10 {
                format!("{}…", &t.name[..9])
            } else {
                t.name.clone()
            };

            Row::new(vec![
                Cell::from(name).style(name_style),
                Cell::from(format!("{vol_pct:>3}%")),
                Cell::from(pan_str),
                Cell::from(flags).style(flag_style),
            ])
            .height(1)
        })
        .collect();

    let n = app.project.tracks.len();
    app.track_scroll = app.track_scroll.content_length(n);

    let widths = [
        Constraint::Length(10),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Length(3),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Tracks "),
        )
        .row_highlight_style(Style::default());

    let mut state = TableState::default().with_selected(Some(app.selected_track));
    f.render_stateful_widget(table, area, &mut state);

    if n > 0 {
        let scroll_area = Rect {
            x: area.x + area.width - 1,
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            scroll_area,
            &mut app.track_scroll,
        );
    }
}

// ── Timeline ──────────────────────────────────────────────────────────────────

fn draw_timeline(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Timeline (i/o zoom  ◀▶ scroll) ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 || inner.width < 2 {
        return;
    }

    // Reserve top row for the ruler.
    let ruler_area = Rect { height: 1, ..inner };
    let tracks_area = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };

    draw_ruler(f, app, ruler_area);
    draw_clips(f, app, tracks_area);
    draw_playhead(f, app, inner);
}

fn draw_ruler(f: &mut Frame, app: &App, area: Rect) {
    let spc = app.samples_per_col as f64;
    let sr = app.project.sample_rate as f64;
    // How many samples wide is the ruler?
    let total_cols = area.width as u64;

    let mut spans: Vec<Span> = Vec::new();
    for col in 0..total_cols {
        let sample = app.timeline_offset + col * app.samples_per_col;
        let secs = sample as f64 / sr;
        // Place a label at every whole second (or every N seconds for coarse zoom).
        let label_every = ((spc / sr) as u64 + 1).max(1); // seconds between labels
        let whole_sec = (secs as u64) / label_every * label_every;
        let col_of_sec = if app.samples_per_col > 0 {
            (whole_sec as f64 * sr / spc) as u64
        } else {
            0
        };

        if col == col_of_sec.saturating_sub(app.timeline_offset / app.samples_per_col) {
            let label = format!("{:02}:{:02}", whole_sec / 60, whole_sec % 60);
            spans.push(Span::styled(
                format!("{:<8}", label),
                Style::default().fg(Color::DarkGray),
            ));
        } else if spans.len() < total_cols as usize {
            // pad to keep columns aligned — only push if we haven't already from a label
        }
    }

    // Simpler: just render label every N cols
    let mut text = String::new();
    let label_every_cols = (sr / spc).ceil() as u64; // cols per second
    let label_step = label_every_cols.max(8);
    for col in 0..total_cols {
        let sample = app.timeline_offset + col * app.samples_per_col;
        let secs = sample as f64 / sr;
        if col % label_step == 0 {
            let label = format!("{:02}:{:02}", secs as u64 / 60, secs as u64 % 60);
            // pad/truncate to label_step chars
            let s = format!("{:<width$}", label, width = label_step as usize);
            text.push_str(&s);
            // skip label_step-1 cols
        }
    }
    text.truncate(total_cols as usize);

    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_clips(f: &mut Frame, app: &App, area: Rect) {
    let track_height: u16 = 2;
    let _sr = app.project.sample_rate as u64;

    for (ti, track) in app.project.tracks.iter().enumerate() {
        let row_y = area.y + (ti as u16) * track_height;
        if row_y >= area.y + area.height {
            break;
        }
        let row_h = track_height.min(area.y + area.height - row_y);

        let is_sel = ti == app.selected_track;
        let _track_bg = if track.muted {
            Color::DarkGray
        } else if is_sel {
            Color::DarkGray
        } else {
            Color::Reset
        };

        for clip in &track.clips {
            if clip.muted { continue; }

            let clip_start_col = if clip.timeline_start >= app.timeline_offset {
                ((clip.timeline_start - app.timeline_offset) / app.samples_per_col) as u16
            } else if clip.timeline_end() > app.timeline_offset {
                0
            } else {
                continue;
            };

            if clip_start_col >= area.width { continue; }

            let clip_len_cols =
                (clip.effective_length() / app.samples_per_col).max(1) as u16;
            let vis_start = if clip.timeline_start < app.timeline_offset {
                let hidden = (app.timeline_offset - clip.timeline_start) / app.samples_per_col;
                clip_len_cols.saturating_sub(hidden as u16)
            } else {
                clip_len_cols
            };
            let vis_cols = vis_start.min(area.width - clip_start_col);

            if vis_cols == 0 { continue; }

            let clip_rect = Rect {
                x: area.x + clip_start_col,
                y: row_y,
                width: vis_cols,
                height: row_h,
            };

            let clip_color = if is_sel { Color::Cyan } else { Color::Blue };
            let clip_block = Block::default()
                .style(Style::default().fg(Color::Black).bg(clip_color))
                .borders(Borders::LEFT | Borders::RIGHT);

            // Clip label
            let label = if vis_cols > 4 {
                let max_len = (vis_cols as usize).saturating_sub(2);
                if clip.name.len() > max_len {
                    clip.name[..max_len].to_string()
                } else {
                    clip.name.clone()
                }
            } else {
                String::new()
            };

            let para = Paragraph::new(label)
                .block(clip_block)
                .style(Style::default().fg(Color::Black).bg(clip_color));

            f.render_widget(para, clip_rect);
        }
    }
}

fn draw_playhead(f: &mut Frame, app: &App, area: Rect) {
    if app.playhead < app.timeline_offset { return; }
    let col = ((app.playhead - app.timeline_offset) / app.samples_per_col) as u16;
    if col >= area.width { return; }

    for row in 0..area.height {
        let cell_area = Rect {
            x: area.x + col,
            y: area.y + row,
            width: 1,
            height: 1,
        };
        f.render_widget(
            Paragraph::new("│").style(Style::default().fg(Color::Yellow)),
            cell_area,
        );
    }
}

// ── Status bar ────────────────────────────────────────────────────────────────

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let zoom_label = {
        let sr = app.project.sample_rate as u64;
        if app.samples_per_col >= sr {
            format!("{}s/col", app.samples_per_col / sr)
        } else {
            format!("{:.0}ms/col", app.samples_per_col as f64 / sr as f64 * 1000.0)
        }
    };

    let track_info = app
        .project
        .tracks
        .get(app.selected_track)
        .map(|t| {
            format!(
                " Track: {}  Vol: {:.0}%  Pan: {:.2}",
                t.name,
                t.volume * 100.0,
                t.pan
            )
        })
        .unwrap_or_default();

    let left = format!("{track_info}  Zoom: {zoom_label}");
    let right = " [?] Help  [q] Quit  [Space] Play  [r] Rec  [m] Mute  [s] Solo ";

    let width = area.width as usize;
    let pad = width.saturating_sub(left.len() + right.len());
    let status = format!("{left}{}{right}", " ".repeat(pad));

    f.render_widget(
        Paragraph::new(status).style(Style::default().bg(Color::DarkGray).fg(Color::White)),
        area,
    );
}

// ── Help overlay ──────────────────────────────────────────────────────────────

fn draw_help(f: &mut Frame, area: Rect) {
    let popup_w = 52u16;
    let popup_h = 22u16;
    let x = area.x + area.width.saturating_sub(popup_w) / 2;
    let y = area.y + area.height.saturating_sub(popup_h) / 2;
    let popup = Rect { x, y, width: popup_w.min(area.width), height: popup_h.min(area.height) };

    f.render_widget(Clear, popup);

    let help_text = vec![
        Line::from(Span::styled("  Keyboard Shortcuts", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))),
        Line::from(""),
        Line::from(Span::styled("  Transport", Style::default().fg(Color::Yellow))),
        Line::from("  Space          Play / Stop"),
        Line::from("  r              Record"),
        Line::from("  Home           Rewind to start"),
        Line::from(""),
        Line::from(Span::styled("  Track List", Style::default().fg(Color::Yellow))),
        Line::from("  ↑ / k          Select track up"),
        Line::from("  ↓ / j          Select track down"),
        Line::from("  m              Mute selected track"),
        Line::from("  s              Solo selected track"),
        Line::from("  a              Arm / disarm track"),
        Line::from("  + / -          Volume up / down"),
        Line::from("  [ / ]          Pan left / right"),
        Line::from(""),
        Line::from(Span::styled("  Timeline", Style::default().fg(Color::Yellow))),
        Line::from("  ← / h          Scroll left"),
        Line::from("  → / l          Scroll right"),
        Line::from("  i / o          Zoom in / out"),
        Line::from(""),
        Line::from("  Press any key to close"),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help ")
        .style(Style::default().bg(Color::Black));

    let para = Paragraph::new(help_text).block(block);
    f.render_widget(para, popup);
}
