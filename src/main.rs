mod config;
mod emulators;
mod theme;

use clap::{Parser, Subcommand};
use emulators::{Device, Platform, State};
use ratatui::{
  crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
  layout::{Constraint, Layout, Rect},
  style::{Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
  DefaultTerminal,
};
use std::io;

#[derive(Parser)]
#[command(version, about = "List and open Android/iOS emulators")]
struct Cli {
  #[command(subcommand)]
  command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
  /// Print a plain text list of all emulators
  List,
  /// Open an emulator by name
  Open {
    /// Name of the emulator to open
    name: Vec<String>,
  },
}

fn main() {
  let cli = Cli::parse();

  match cli.command {
    Some(Commands::List) => {
      print!("{}", emulators::format_emulator_list());
    }
    Some(Commands::Open { name }) => {
      let name = name.join(" ");
      match emulators::find_device(&name).and_then(|device| device.open()) {
        Ok(msg) => println!("{}", msg),
        Err(e) => {
          eprintln!("Error: {}", e);
          std::process::exit(1);
        }
      }
    }
    None => {
      if let Err(e) = run_tui() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
      }
    }
  }
}

struct App {
  devices: Vec<Device>,
  /// Indices into `devices` that match the filter, in display order.
  filtered: Vec<usize>,
  /// Position of the selected device within `filtered`.
  selected: usize,
  /// Row highlighted in the rendered list. Recomputed every frame because
  /// section headers are rows too.
  list_state: ListState,
  filter: String,
  result_message: Option<String>,
  show_help: bool,
}

impl App {
  fn new(devices: Vec<Device>) -> Self {
    let mut app = App {
      devices,
      filtered: Vec::new(),
      selected: 0,
      list_state: ListState::default(),
      filter: String::new(),
      result_message: None,
      show_help: false,
    };
    app.apply_filter();
    app
  }

  /// Recompute `filtered` from the filter text and select the first match.
  fn apply_filter(&mut self) {
    let query = self.filter.to_lowercase();
    self.filtered = self
      .devices
      .iter()
      .enumerate()
      .filter(|(_, d)| query.is_empty() || d.name.to_lowercase().contains(&query))
      .map(|(i, _)| i)
      .collect();
    self.selected = 0;
  }

  fn move_selection(&mut self, delta: isize) {
    if let Some(last) = self.filtered.len().checked_sub(1) {
      self.selected = self.selected.saturating_add_signed(delta).min(last);
    }
  }

  fn selected_device(&self) -> Option<&Device> {
    let &index = self.filtered.get(self.selected)?;
    self.devices.get(index)
  }

  /// Reload the device list in-place, keeping the same device selected if it
  /// is still listed.
  fn reload(&mut self) {
    let previous = self.selected_device().map(|d| (d.platform, d.id.clone()));
    self.devices = emulators::collect_devices().devices;
    self.apply_filter();

    if let Some((platform, id)) = previous {
      if let Some(pos) = self.filtered.iter().position(|&i| {
        let d = &self.devices[i];
        d.platform == platform && d.id == id
      }) {
        self.selected = pos;
      }
    }
  }
}

fn state_color(state: &State, theme: &theme::ThemeColors) -> ratatui::style::Color {
  match state {
    State::Booted => theme.state_booted_fg,
    State::Shutdown => theme.state_shutdown_fg,
    State::Other(_) => theme.state_unknown_fg,
  }
}

/// "● Booted" / "○ Shutdown", colored by state.
fn state_span(state: &State, theme: &theme::ThemeColors) -> Span<'static> {
  let dot = if state.is_booted() { "● " } else { "○ " };
  Span::styled(
    format!("{}{}", dot, state),
    Style::default().fg(state_color(state, theme)),
  )
}

fn header_row(platform: Platform, theme: &theme::ThemeColors) -> ListItem<'static> {
  ListItem::new(Line::from(Span::styled(
    format!(" {}", platform.section_title()),
    Style::default()
      .fg(theme.header_fg)
      .add_modifier(Modifier::BOLD),
  )))
}

fn device_row<'a>(device: &'a Device, theme: &theme::ThemeColors) -> ListItem<'a> {
  ListItem::new(Line::from(vec![
    Span::raw("   "),
    Span::styled(&device.name, Style::default().fg(theme.name_fg)),
    Span::raw("  "),
    state_span(&device.state, theme),
    Span::styled(
      format!("  ({})", device.meta),
      Style::default().fg(theme.meta_fg),
    ),
  ]))
}

/// The detail panel contents for one device.
fn detail_lines<'a>(device: &'a Device, theme: &theme::ThemeColors) -> Vec<Line<'a>> {
  let label = |text: &'a str| {
    Line::from(Span::styled(
      format!(" {}:", text),
      Style::default()
        .fg(theme.help_key_fg)
        .add_modifier(Modifier::BOLD),
    ))
  };
  let text = |value: &'a str| {
    Line::from(vec![
      Span::raw("   "),
      Span::styled(value, Style::default().fg(theme.name_fg)),
    ])
  };

  vec![
    Line::from(Span::styled(
      format!(" {}", device.platform.kind()),
      Style::default()
        .fg(theme.header_fg)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    )),
    Line::from(""),
    label("Name"),
    text(&device.name),
    Line::from(""),
    label(device.platform.id_label()),
    text(&device.id),
    Line::from(""),
    label("State"),
    Line::from(vec![Span::raw("   "), state_span(&device.state, theme)]),
    Line::from(""),
    label(device.platform.meta_label()),
    text(&device.meta),
    Line::from(""),
  ]
}

/// Return a centered Rect of fixed `width` x `height` within `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
  let x = area.x + area.width.saturating_sub(width) / 2;
  let y = area.y + area.height.saturating_sub(height) / 2;
  Rect {
    x,
    y,
    width: width.min(area.width),
    height: height.min(area.height),
  }
}

fn run_tui() -> io::Result<()> {
  let emulators::Collected { devices, errors } = emulators::collect_devices();
  if devices.is_empty() {
    if errors.is_empty() {
      println!("No emulators or simulators found.");
      return Ok(());
    }
    // Explain why nothing could be listed (e.g. adb / emulator not found)
    return Err(io::Error::other(errors.join("\n\n")));
  }

  let cfg = config::load_config();
  let theme = theme::resolve_theme(
    cfg.as_ref().and_then(|c| c.theme.as_deref()),
    cfg.as_ref().and_then(|c| c.theme_overrides.as_ref()),
  );

  // init() enters raw mode + the alternate screen and installs a panic hook
  // that restores the terminal; restore() always runs, even if run_app fails.
  let mut terminal = ratatui::init();
  let mut app = App::new(devices);
  let result = run_app(&mut terminal, &mut app, &theme);
  ratatui::restore();

  if let Some(msg) = app.result_message {
    println!("{}", msg);
  }

  result
}

fn run_app(
  terminal: &mut DefaultTerminal,
  app: &mut App,
  theme: &theme::ThemeColors,
) -> io::Result<()> {
  loop {
    terminal.draw(|frame| {
      let area = frame.area();

      let chunks = Layout::vertical([
        Constraint::Length(3), // filter input
        Constraint::Min(1),    // list (+ optional detail panel)
        Constraint::Length(1), // help bar
      ])
      .split(area);

      // Split the list area horizontally when terminal is wide enough for the detail panel
      let show_detail = area.width >= 90;
      let (list_area, detail_area_opt) = if show_detail {
        let split = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
          .split(chunks[1]);
        (split[0], Some(split[1]))
      } else {
        (chunks[1], None)
      };

      // Filter input
      let filter_text = if app.filter.is_empty() {
        "Type to filter...".to_string()
      } else {
        app.filter.clone()
      };
      let filter_style = if app.filter.is_empty() {
        Style::default().fg(theme.filter_placeholder_fg)
      } else {
        Style::default().fg(theme.filter_active_fg)
      };
      let filter = Paragraph::new(filter_text)
        .style(filter_style)
        .block(Block::default().borders(Borders::ALL).title(" Filter "));
      frame.render_widget(filter, chunks[0]);

      // List title with visible/total count
      let list_title = if app.filter.is_empty() {
        format!(" Emulators ({}) ", app.devices.len())
      } else {
        format!(" Emulators ({}/{}) ", app.filtered.len(), app.devices.len())
      };

      // Rows: a heading whenever the platform changes, then the device. The
      // devices are grouped by platform, so this yields one heading per group.
      let mut items: Vec<ListItem> = Vec::new();
      let mut selected_row = None;
      let mut group: Option<Platform> = None;
      for (pos, &index) in app.filtered.iter().enumerate() {
        let device = &app.devices[index];
        if group != Some(device.platform) {
          group = Some(device.platform);
          items.push(header_row(device.platform, theme));
        }
        if pos == app.selected {
          selected_row = Some(items.len());
        }
        items.push(device_row(device, theme));
      }
      app.list_state.select(selected_row);

      let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(list_title))
        .highlight_style(
          Style::default()
            .bg(theme.selection_bg)
            .add_modifier(Modifier::BOLD),
        );
      frame.render_stateful_widget(list, list_area, &mut app.list_state);

      // Detail panel — shown when terminal is wide enough
      if let Some(detail_area) = detail_area_opt {
        let detail_lines: Vec<Line> = match app.selected_device() {
          Some(device) => detail_lines(device, theme),
          None => vec![
            Line::from(""),
            Line::from(Span::styled(
              " Select an emulator",
              Style::default().fg(theme.meta_fg),
            )),
            Line::from(Span::styled(
              " to see details",
              Style::default().fg(theme.meta_fg),
            )),
          ],
        };

        let detail_widget = Paragraph::new(detail_lines)
          .block(Block::default().borders(Borders::ALL).title(" Details "))
          .wrap(Wrap { trim: false });
        frame.render_widget(detail_widget, detail_area);
      }

      // Help bar
      let help = Paragraph::new(Line::from(vec![
        Span::styled(" ↑/↓", Style::default().fg(theme.help_key_fg)),
        Span::styled(" navigate  ", Style::default().fg(theme.help_text_fg)),
        Span::styled("Enter", Style::default().fg(theme.help_key_fg)),
        Span::styled(" open  ", Style::default().fg(theme.help_text_fg)),
        Span::styled("^R", Style::default().fg(theme.help_key_fg)),
        Span::styled(" refresh  ", Style::default().fg(theme.help_text_fg)),
        Span::styled("?", Style::default().fg(theme.help_key_fg)),
        Span::styled(" help  ", Style::default().fg(theme.help_text_fg)),
        Span::styled("Esc", Style::default().fg(theme.help_key_fg)),
        Span::styled(" quit", Style::default().fg(theme.help_text_fg)),
      ]));
      frame.render_widget(help, chunks[2]);

      // Help overlay popup
      if app.show_help {
        let popup_w = 46u16;
        let popup_h = 13u16;
        let popup_area = centered_rect(popup_w, popup_h, area);
        frame.render_widget(Clear, popup_area);

        let key_style = Style::default()
          .fg(theme.help_key_fg)
          .add_modifier(Modifier::BOLD);
        let text_style = Style::default().fg(theme.help_text_fg);

        let help_lines = vec![
          Line::from(""),
          Line::from(vec![
            Span::styled("  ↑/↓ ^N/^P  ", key_style),
            Span::styled("Navigate list", text_style),
          ]),
          Line::from(vec![
            Span::styled("  Enter      ", key_style),
            Span::styled("Launch selected emulator", text_style),
          ]),
          Line::from(vec![
            Span::styled("  ^R / F5    ", key_style),
            Span::styled("Refresh emulator list", text_style),
          ]),
          Line::from(vec![
            Span::styled("  ^U         ", key_style),
            Span::styled("Clear filter", text_style),
          ]),
          Line::from(vec![
            Span::styled("  Type       ", key_style),
            Span::styled("Filter emulators", text_style),
          ]),
          Line::from(vec![
            Span::styled("  Backspace  ", key_style),
            Span::styled("Delete filter character", text_style),
          ]),
          Line::from(vec![
            Span::styled("  ? / F1     ", key_style),
            Span::styled("Toggle this help", text_style),
          ]),
          Line::from(vec![
            Span::styled("  Esc / ^C   ", key_style),
            Span::styled("Quit", text_style),
          ]),
          Line::from(""),
        ];

        let help_popup = Paragraph::new(help_lines).block(
          Block::default()
            .borders(Borders::ALL)
            .title(" Keyboard Shortcuts ")
            .title_style(
              Style::default()
                .fg(theme.header_fg)
                .add_modifier(Modifier::BOLD),
            ),
        );
        frame.render_widget(help_popup, popup_area);
      }
    })?;

    if event::poll(std::time::Duration::from_millis(100))? {
      if let Event::Key(key) = event::read()? {
        if key.kind != KeyEventKind::Press {
          continue;
        }

        // When the help overlay is open, only handle overlay-dismissal keys
        if app.show_help {
          match key.code {
            KeyCode::Esc | KeyCode::Char('q' | '?') | KeyCode::F(1) => {
              app.show_help = false;
            }
            _ => {}
          }
          continue;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
          KeyCode::Esc => break,
          KeyCode::Char('c') if ctrl => break,
          KeyCode::Char('?') if app.filter.is_empty() => app.show_help = true,
          KeyCode::F(1) => app.show_help = true,
          KeyCode::Char('r') if ctrl => app.reload(),
          KeyCode::F(5) => app.reload(),
          KeyCode::Char('u') if ctrl => {
            app.filter.clear();
            app.apply_filter();
          }
          KeyCode::Char('n') if ctrl => app.move_selection(1),
          KeyCode::Char('p') if ctrl => app.move_selection(-1),
          KeyCode::Down => app.move_selection(1),
          KeyCode::Up => app.move_selection(-1),
          KeyCode::Enter => {
            if let Some(device) = app.selected_device() {
              match device.open() {
                Ok(msg) => {
                  app.result_message = Some(msg);
                  break;
                }
                Err(e) => {
                  app.result_message = Some(format!("Error: {}", e));
                  break;
                }
              }
            }
          }
          KeyCode::Backspace => {
            app.filter.pop();
            app.apply_filter();
          }
          // Ignore other Ctrl/Alt chords so they don't end up in the filter
          KeyCode::Char(_)
            if key
              .modifiers
              .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {}
          KeyCode::Char(c) => {
            app.filter.push(c);
            app.apply_filter();
          }
          _ => {}
        }
      }
    }
  }

  Ok(())
}
