mod config;
mod emulators;
mod theme;

use clap::{Parser, Subcommand};
use emulators::{EmulatorEntry, EmulatorType};
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
      match emulators::find_emulator(&name) {
        Ok(EmulatorType::Android(emu_name)) => match emulators::open_android_emulator(&emu_name) {
          Ok(msg) => println!("{}", msg),
          Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
          }
        },
        Ok(EmulatorType::Ios(udid)) => match emulators::open_ios_simulator(&udid) {
          Ok(msg) => println!("{}", msg),
          Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
          }
        },
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
  entries: Vec<EmulatorEntry>,
  filtered_indices: Vec<usize>,
  list_state: ListState,
  filter: String,
  result_message: Option<String>,
  show_help: bool,
}

impl App {
  fn new(entries: Vec<EmulatorEntry>) -> Self {
    let filtered_indices: Vec<usize> = (0..entries.len()).collect();
    let mut list_state = ListState::default();
    // Select first non-header item
    let first_selectable = filtered_indices
      .iter()
      .position(|&i| !entries[i].is_header());
    list_state.select(first_selectable);

    App {
      entries,
      filtered_indices,
      list_state,
      filter: String::new(),
      result_message: None,
      show_help: false,
    }
  }

  fn apply_filter(&mut self) {
    let query = self.filter.to_lowercase();
    self.filtered_indices = (0..self.entries.len())
      .filter(|&i| {
        let entry = &self.entries[i];
        if entry.is_header() {
          // Keep headers if any child in their section matches
          return self.section_has_match(i, &query);
        }
        query.is_empty() || entry.display_name().to_lowercase().contains(&query)
      })
      .collect();

    // Select first non-header item
    let first_selectable = self
      .filtered_indices
      .iter()
      .position(|&i| !self.entries[i].is_header());
    self.list_state.select(first_selectable);
  }

  fn section_has_match(&self, header_idx: usize, query: &str) -> bool {
    if query.is_empty() {
      return true;
    }
    for i in (header_idx + 1)..self.entries.len() {
      if self.entries[i].is_header() {
        break;
      }
      if self.entries[i]
        .display_name()
        .to_lowercase()
        .contains(query)
      {
        return true;
      }
    }
    false
  }

  fn move_selection(&mut self, delta: i32) {
    let selectable: Vec<usize> = self
      .filtered_indices
      .iter()
      .enumerate()
      .filter(|(_, &i)| !self.entries[i].is_header())
      .map(|(pos, _)| pos)
      .collect();

    if selectable.is_empty() {
      self.list_state.select(None);
      return;
    }

    let current = self.list_state.selected().unwrap_or(0);
    let current_pos = selectable.iter().position(|&p| p == current).unwrap_or(0);
    let new_pos = if delta > 0 {
      (current_pos + 1).min(selectable.len() - 1)
    } else {
      current_pos.saturating_sub(1)
    };
    self.list_state.select(Some(selectable[new_pos]));
  }

  fn selected_entry(&self) -> Option<&EmulatorEntry> {
    let selected = self.list_state.selected()?;
    let &entry_idx = self.filtered_indices.get(selected)?;
    let entry = &self.entries[entry_idx];
    if entry.is_header() {
      None
    } else {
      Some(entry)
    }
  }

  /// Reload the emulator list in-place, preserving selection by name if possible.
  fn reload(&mut self) {
    let selected_name = self.selected_entry().map(|e| e.display_name().to_string());
    self.entries = emulators::collect_all_entries().entries;
    self.apply_filter();

    // Try to restore selection by matching the previously selected display name
    if let Some(name) = selected_name {
      if let Some(pos) = self
        .filtered_indices
        .iter()
        .enumerate()
        .find(|(_, &i)| self.entries[i].display_name() == name)
        .map(|(pos, _)| pos)
      {
        self.list_state.select(Some(pos));
      }
    }
  }
}

/// Build the detail panel lines for the selected emulator (returns owned data to avoid borrow conflicts).
struct DetailData {
  kind: String,
  fields: Vec<(String, String)>,
}

fn get_detail_data(app: &App) -> Option<DetailData> {
  let selected_pos = app.list_state.selected()?;
  let &entry_idx = app.filtered_indices.get(selected_pos)?;
  match &app.entries[entry_idx] {
    EmulatorEntry::Android(e) => Some(DetailData {
      kind: "Android Emulator".to_string(),
      fields: vec![
        ("Name".to_string(), e.name.clone()),
        ("ID".to_string(), e.id.clone()),
        ("State".to_string(), e.state.clone()),
        ("Type".to_string(), e.device_type.clone()),
      ],
    }),
    EmulatorEntry::Ios(s) => Some(DetailData {
      kind: "iOS Simulator".to_string(),
      fields: vec![
        ("Name".to_string(), s.name.clone()),
        ("UDID".to_string(), s.udid.clone()),
        ("State".to_string(), s.state.clone()),
        ("Runtime".to_string(), s.runtime.clone()),
      ],
    }),
    EmulatorEntry::SectionHeader(_) => None,
  }
}

fn state_color(state: &str, theme: &theme::ThemeColors) -> ratatui::style::Color {
  match state {
    emulators::STATE_BOOTED => theme.state_booted_fg,
    emulators::STATE_SHUTDOWN => theme.state_shutdown_fg,
    _ => theme.state_unknown_fg,
  }
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
  let emulators::Collected { entries, errors } = emulators::collect_all_entries();
  if entries.is_empty() {
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
  let mut app = App::new(entries);
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
      let visible_count = app
        .filtered_indices
        .iter()
        .filter(|&&i| !app.entries[i].is_header())
        .count();
      let total_count = app.entries.iter().filter(|e| !e.is_header()).count();
      let list_title = if app.filter.is_empty() {
        format!(" Emulators ({}) ", total_count)
      } else {
        format!(" Emulators ({}/{}) ", visible_count, total_count)
      };

      // Emulator list items
      let items: Vec<ListItem> = app
        .filtered_indices
        .iter()
        .map(|&i| {
          let entry = &app.entries[i];
          match entry {
            EmulatorEntry::SectionHeader(s) => ListItem::new(Line::from(Span::styled(
              format!(" {}", s),
              Style::default()
                .fg(theme.header_fg)
                .add_modifier(Modifier::BOLD),
            ))),
            EmulatorEntry::Android(e) => {
              let state_dot = if e.state == emulators::STATE_BOOTED {
                "● "
              } else {
                "○ "
              };
              ListItem::new(Line::from(vec![
                Span::raw("   "),
                Span::styled(&e.name, Style::default().fg(theme.name_fg)),
                Span::raw("  "),
                Span::styled(
                  format!("{}{}", state_dot, e.state),
                  Style::default().fg(state_color(&e.state, theme)),
                ),
                Span::styled(
                  format!("  ({})", e.device_type),
                  Style::default().fg(theme.meta_fg),
                ),
              ]))
            }
            EmulatorEntry::Ios(s) => {
              let state_dot = if s.state == emulators::STATE_BOOTED {
                "● "
              } else {
                "○ "
              };
              ListItem::new(Line::from(vec![
                Span::raw("   "),
                Span::styled(&s.name, Style::default().fg(theme.name_fg)),
                Span::raw("  "),
                Span::styled(
                  format!("{}{}", state_dot, s.state),
                  Style::default().fg(state_color(&s.state, theme)),
                ),
                Span::styled(
                  format!("  ({})", s.runtime),
                  Style::default().fg(theme.meta_fg),
                ),
              ]))
            }
          }
        })
        .collect();

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
        let detail_data = get_detail_data(app);
        let detail_lines: Vec<Line> = if let Some(data) = detail_data {
          let mut lines = vec![
            Line::from(Span::styled(
              format!(" {}", data.kind),
              Style::default()
                .fg(theme.header_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )),
            Line::from(""),
          ];
          for (label, value) in &data.fields {
            lines.push(Line::from(Span::styled(
              format!(" {}:", label),
              Style::default()
                .fg(theme.help_key_fg)
                .add_modifier(Modifier::BOLD),
            )));
            // State field gets a colored dot indicator
            if label == "State" {
              let dot = if value == emulators::STATE_BOOTED {
                "● "
              } else {
                "○ "
              };
              lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(
                  format!("{}{}", dot, value),
                  Style::default().fg(state_color(value, theme)),
                ),
              ]));
            } else {
              lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(value.clone(), Style::default().fg(theme.name_fg)),
              ]));
            }
            lines.push(Line::from(""));
          }
          lines
        } else {
          vec![
            Line::from(""),
            Line::from(Span::styled(
              " Select an emulator",
              Style::default().fg(theme.meta_fg),
            )),
            Line::from(Span::styled(
              " to see details",
              Style::default().fg(theme.meta_fg),
            )),
          ]
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
            if let Some(entry) = app.selected_entry() {
              match emulators::open_entry(entry) {
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
