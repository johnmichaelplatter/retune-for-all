use std::io;
use std::sync::{Arc, Mutex};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};

use crate::midi::MidiState;
use crate::tuning::{prompt_input, setup_grid_tuning, update_tuning, parse_scl, parse_kbm, apply_custom_tuning, Kbm};

pub fn run_tui(state: Arc<Mutex<MidiState>>) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut input = String::new();
    let mut logs: Vec<String> = vec![
        "Welcome to Poly-Router TUI!".to_string(),
        "Press 1-9 for presets, type '0' for SCL, type 'grid' for Launchpad, 'q' to quit.".to_string(),
    ];

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints([Constraint::Min(1), Constraint::Length(3)].as_ref())
                .split(f.size());

            // Logs Panel
            let log_text = logs.iter().cloned().collect::<Vec<String>>().join("\n");
            let logs_block = Paragraph::new(log_text)
                .block(Block::default().title(" Status / Logs ").borders(Borders::ALL));
            f.render_widget(logs_block, chunks[0]);

            // Input Panel
            let input_block = Paragraph::new(format!("> {}", input))
                .style(Style::default().fg(Color::Yellow))
                .block(Block::default().title(" Command Input (Press Enter) ").borders(Borders::ALL));
            f.render_widget(input_block, chunks[1]);
        })?;

        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char(c) => { input.push(c); }
                    KeyCode::Backspace => { input.pop(); }
                    KeyCode::Enter => {
                        let cmd = input.trim().to_string();
                        input.clear();

                        if cmd == "q" {
                            break;
                        } else if cmd == "grid" || cmd == "0" {
                            // --- MAGIC TRICK: SUSPEND TUI FOR COMPLEX PROMPTS ---
                            disable_raw_mode()?;
                            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                            
                            if cmd == "grid" {
                                match setup_grid_tuning(state.clone()) {
                                    Ok(msg) => logs.push(msg),
                                    Err(e) => logs.push(format!("Grid Error: {}", e)),
                                }
                            } else if cmd == "0" {
                                let scl_path = prompt_input("Enter path to .scl file: ");
                                let scl_path = scl_path.trim_matches('"').trim_matches('\'');
                                match parse_scl(scl_path) {
                                    Ok(multipliers) => {
                                        let kbm_path = prompt_input("Enter path to .kbm file (or press Enter for standard linear mapping): ");
                                        let kbm_path = kbm_path.trim_matches('"').trim_matches('\'');
                                        let kbm = if kbm_path.is_empty() {
                                            Kbm { map_size: 0, first_note: 0, last_note: 127, middle_note: 69, ref_note: 69, ref_freq: 440.0, formal_octave: (multipliers.len() - 1) as i32, mapping: vec![] }
                                        } else {
                                            parse_kbm(kbm_path).unwrap_or(Kbm { map_size: 0, first_note: 0, last_note: 127, middle_note: 69, ref_note: 69, ref_freq: 440.0, formal_octave: 12, mapping: vec![] })
                                        };
                                        match apply_custom_tuning(state.clone(), &multipliers, &kbm) {
                                            Ok(_) => logs.push(format!("Successfully loaded SCL tuning!")),
                                            Err(e) => logs.push(format!("SCL Apply Error: {}", e)),
                                        }
                                    },
                                    Err(e) => logs.push(format!("SCL Parse Error: {}", e)),
                                }
                            }

                            // --- RESUME TUI ---
                            execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                            enable_raw_mode()?;
                            terminal.clear()?;
                        } else {
                            if update_tuning(state.clone(), &cmd) {
                                logs.push(format!("Preset {} loaded.", cmd));
                            } else {
                                logs.push(format!("Unknown command: {}", cmd));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Cleanup TUI on exit
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}