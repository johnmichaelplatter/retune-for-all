use std::time::Duration;
use std::sync::{Arc, Mutex};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style, Modifier},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};

use crate::midi::{MidiState, send_mpe_configuration};
use crate::tuning::{prompt_input, setup_grid_tuning, update_tuning, parse_scl, parse_kbm, apply_custom_tuning, apply_equal_division, Kbm};

#[derive(PartialEq, Clone, Copy)]
pub enum Focus {
    Input,
    Output,
    Mode,
    PitchBend,
    Channel(usize),
    Divisions,
    Interval,
    CommandInput,
}

pub struct UiState {
    pub focus: Focus,
    pub is_editing_dropdown: bool,
    pub is_editing_pb: bool,
    pub pb_input: String,

    pub is_editing_divisions: bool,
    pub clear_divisions: bool,
    pub divisions_input: String,

    pub is_editing_interval: bool,
    pub clear_interval: bool,
    pub interval_input: String,

    pub dropdown_index: usize,
    pub in_ports: Vec<String>,
    pub out_ports: Vec<String>,
    pub selected_in: usize,
    pub selected_out: usize,
    pub input: String,
    pub logs: Vec<String>,
}

pub enum UiAction {
    None,
    Quit,
    ChangeInput(usize),
    ChangeOutput(usize),
}

pub fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ui_state: &mut UiState,
    state_mutex: Arc<Mutex<MidiState>>
) -> Result<UiAction, Box<dyn std::error::Error>> {
    
    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(7), // Settings
                    Constraint::Length(3), // Equal Division
                    Constraint::Min(1),    // Logs
                    Constraint::Length(3)  // Command Input
                ].as_ref())
                .split(f.size());

            let mut midi_state = state_mutex.lock().unwrap();

            // --- BUILD SETTINGS TOP ROW ---
            let mut top_row = vec![];
            
            let in_str = if ui_state.is_editing_dropdown && ui_state.focus == Focus::Input { format!("< {} >", ui_state.in_ports.get(ui_state.dropdown_index).unwrap_or(&"None".to_string())) } else { format!("[ {} ]", ui_state.in_ports.get(ui_state.selected_in).unwrap_or(&"None".to_string())) };
            let in_style = if ui_state.focus == Focus::Input { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            top_row.push(Span::raw("Input Device: "));
            top_row.push(Span::styled(in_str, in_style));
            top_row.push(Span::raw("   "));

            let out_str = if ui_state.is_editing_dropdown && ui_state.focus == Focus::Output { format!("< {} >", ui_state.out_ports.get(ui_state.dropdown_index).unwrap_or(&"None".to_string())) } else { format!("[ {} ]", ui_state.out_ports.get(ui_state.selected_out).unwrap_or(&"None".to_string())) };
            let out_style = if ui_state.focus == Focus::Output { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            top_row.push(Span::raw("Output Device: "));
            top_row.push(Span::styled(out_str, out_style));
            top_row.push(Span::raw("   "));

            let mode_str = if midi_state.is_mpe { "< MPE >" } else { "< Multi-timbral >" };
            let mode_style = if ui_state.focus == Focus::Mode { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            top_row.push(Span::raw("Output Type: "));
            top_row.push(Span::styled(mode_str, mode_style));
            top_row.push(Span::raw("   "));

            let pb_str = if ui_state.is_editing_pb { format!("< {}_ >", ui_state.pb_input) } else { format!("[ {} ]", midi_state.pitch_bend_range) };
            let pb_style = if ui_state.focus == Focus::PitchBend { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            top_row.push(Span::raw("PB Range: "));
            top_row.push(Span::styled(pb_str, pb_style));

            // --- BUILD DOTS ROW ---
            let mut dots_row = vec![];
            dots_row.push(Span::raw("MIDI In: "));
            let in_dot_color = if midi_state.input_flash > 0 { Color::Cyan } else { Color::DarkGray };
            dots_row.push(Span::styled("•", Style::default().fg(in_dot_color)));
            dots_row.push(Span::raw("    Channels Out: "));

            for i in 0..16 {
                let is_focused = ui_state.focus == Focus::Channel(i);
                let mut dot_style = Style::default();
                if midi_state.output_flash[i] > 0 { dot_style = dot_style.fg(Color::White).add_modifier(Modifier::BOLD); } 
                else if midi_state.channel_enabled[i] { dot_style = dot_style.fg(Color::Gray); } 
                else { dot_style = dot_style.fg(Color::DarkGray); }
                if is_focused { dot_style = dot_style.bg(Color::DarkGray); }
                dots_row.push(Span::styled("• ", dot_style));
            }

            let settings_para = Paragraph::new(vec![Line::raw(""), Line::from(top_row), Line::raw(""), Line::from(dots_row)])
                .block(Block::default().title(" Settings ").borders(Borders::ALL))
                .wrap(Wrap { trim: true }); 
            f.render_widget(settings_para, chunks[0]);

            // --- BUILD EQUAL DIVISION PANEL ---
            let mut ed_row = vec![];
            
            let div_str = if ui_state.is_editing_divisions { format!("< {}_ >", ui_state.divisions_input) } else { format!("[ {} ]", ui_state.divisions_input) };
            let div_style = if ui_state.focus == Focus::Divisions { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            ed_row.push(Span::styled("D", Style::default().add_modifier(Modifier::UNDERLINED)));
            ed_row.push(Span::raw("ivisions: "));
            ed_row.push(Span::styled(div_str, div_style));
            ed_row.push(Span::raw("      "));

            let int_str = if ui_state.is_editing_interval { format!("< {}_ >", ui_state.interval_input) } else { format!("[ {} ]", ui_state.interval_input) };
            let int_style = if ui_state.focus == Focus::Interval { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            ed_row.push(Span::styled("I", Style::default().add_modifier(Modifier::UNDERLINED)));
            ed_row.push(Span::raw("nterval to Divide: "));
            ed_row.push(Span::styled(int_str, int_style));

            let ed_para = Paragraph::new(Line::from(ed_row))
                .block(Block::default().title(" Equal Division ").borders(Borders::ALL));
            f.render_widget(ed_para, chunks[1]);

            // --- BUILD LOGS PANEL ---
            let log_text = ui_state.logs.iter().cloned().collect::<Vec<String>>().join("\n");
            let logs_block = Paragraph::new(log_text).block(Block::default().title(" Logs ").borders(Borders::ALL));
            f.render_widget(logs_block, chunks[2]);

            // --- BUILD COMMAND INPUT ---
            let input_style = if ui_state.focus == Focus::CommandInput { Style::default().fg(Color::Yellow) } else { Style::default() };
            let input_block = Paragraph::new(format!("> {}", ui_state.input))
                .style(input_style)
                .block(Block::default().title(" Command Input (Presets 1-9, '0', 'grid', 'q') ").borders(Borders::ALL));
            f.render_widget(input_block, chunks[3]);
        })?;

        if event::poll(Duration::from_millis(30))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    
                    if ui_state.is_editing_pb {
                        match key.code {
                            KeyCode::Char(c) if c.is_ascii_digit() => { if ui_state.pb_input.len() < 3 { ui_state.pb_input.push(c); } }
                            KeyCode::Backspace => { ui_state.pb_input.pop(); }
                            KeyCode::Enter => {
                                if let Ok(val) = ui_state.pb_input.parse::<u8>() {
                                    let clamped = val.clamp(1, 96); 
                                    state_mutex.lock().unwrap().pitch_bend_range = clamped;
                                    ui_state.logs.push(format!("Pitch bend range updated to {}", clamped));
                                }
                                ui_state.is_editing_pb = false;
                            }
                            KeyCode::Esc => { ui_state.is_editing_pb = false; }
                            _ => {}
                        }
                    } else if ui_state.is_editing_divisions {
                        match key.code {
                            KeyCode::Char(c) => {
                                if ui_state.clear_divisions { ui_state.divisions_input.clear(); ui_state.clear_divisions = false; }
                                ui_state.divisions_input.push(c);
                            }
                            KeyCode::Backspace => { ui_state.divisions_input.pop(); }
                            KeyCode::Enter => {
                                ui_state.is_editing_divisions = false;
                                match apply_equal_division(state_mutex.clone(), &ui_state.divisions_input, &ui_state.interval_input) {
                                    Ok(msg) => ui_state.logs.push(msg),
                                    Err(e) => ui_state.logs.push(format!("ED Error: {}", e)),
                                }
                            }
                            KeyCode::Esc => { ui_state.is_editing_divisions = false; }
                            _ => {}
                        }
                    } else if ui_state.is_editing_interval {
                        match key.code {
                            KeyCode::Char(c) => {
                                if ui_state.clear_interval { ui_state.interval_input.clear(); ui_state.clear_interval = false; }
                                ui_state.interval_input.push(c);
                            }
                            KeyCode::Backspace => { ui_state.interval_input.pop(); }
                            KeyCode::Enter => {
                                ui_state.is_editing_interval = false;
                                match apply_equal_division(state_mutex.clone(), &ui_state.divisions_input, &ui_state.interval_input) {
                                    Ok(msg) => ui_state.logs.push(msg),
                                    Err(e) => ui_state.logs.push(format!("ED Error: {}", e)),
                                }
                            }
                            KeyCode::Esc => { ui_state.is_editing_interval = false; }
                            _ => {}
                        }
                    } else if ui_state.is_editing_dropdown {
                        match key.code {
                            KeyCode::Up => {
                                let max = if ui_state.focus == Focus::Input { ui_state.in_ports.len() } else { ui_state.out_ports.len() };
                                if max > 0 { ui_state.dropdown_index = ui_state.dropdown_index.saturating_sub(1); }
                            }
                            KeyCode::Down => {
                                let max = if ui_state.focus == Focus::Input { ui_state.in_ports.len() } else { ui_state.out_ports.len() };
                                if max > 0 && ui_state.dropdown_index < max - 1 { ui_state.dropdown_index += 1; }
                            }
                            KeyCode::Enter => {
                                ui_state.is_editing_dropdown = false;
                                if ui_state.focus == Focus::Input { return Ok(UiAction::ChangeInput(ui_state.dropdown_index)); } 
                                else if ui_state.focus == Focus::Output { return Ok(UiAction::ChangeOutput(ui_state.dropdown_index)); }
                            }
                            KeyCode::Esc => { ui_state.is_editing_dropdown = false; }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Left => {
                                ui_state.focus = match ui_state.focus {
                                    Focus::Input => Focus::Input,
                                    Focus::Output => Focus::Input,
                                    Focus::Mode => Focus::Output,
                                    Focus::PitchBend => Focus::Mode,
                                    Focus::Channel(0) => Focus::PitchBend,
                                    Focus::Channel(i) => Focus::Channel(i - 1),
                                    Focus::Divisions => Focus::Divisions,
                                    Focus::Interval => Focus::Divisions,
                                    Focus::CommandInput => Focus::Interval,
                                };
                            }
                            KeyCode::Right => {
                                ui_state.focus = match ui_state.focus {
                                    Focus::Input => Focus::Output,
                                    Focus::Output => Focus::Mode,
                                    Focus::Mode => Focus::PitchBend,
                                    Focus::PitchBend => Focus::Channel(0),
                                    Focus::Channel(15) => Focus::Divisions,
                                    Focus::Channel(i) => Focus::Channel(i + 1),
                                    Focus::Divisions => Focus::Interval,
                                    Focus::Interval => Focus::CommandInput,
                                    Focus::CommandInput => Focus::CommandInput,
                                };
                            }
                            KeyCode::Up => { 
                                ui_state.focus = match ui_state.focus {
                                    Focus::CommandInput => Focus::Divisions,
                                    Focus::Divisions | Focus::Interval => Focus::Channel(0),
                                    _ => ui_state.focus
                                };
                            }
                            KeyCode::Down => { 
                                ui_state.focus = match ui_state.focus {
                                    Focus::Input | Focus::Output | Focus::Mode | Focus::PitchBend | Focus::Channel(_) => Focus::Divisions,
                                    Focus::Divisions | Focus::Interval => Focus::CommandInput,
                                    _ => ui_state.focus
                                };
                            }
                            KeyCode::Enter => {
                                match ui_state.focus {
                                    Focus::Input => { ui_state.is_editing_dropdown = true; ui_state.dropdown_index = ui_state.selected_in; }
                                    Focus::Output => { ui_state.is_editing_dropdown = true; ui_state.dropdown_index = ui_state.selected_out; }
                                    Focus::PitchBend => { ui_state.pb_input = state_mutex.lock().unwrap().pitch_bend_range.to_string(); ui_state.is_editing_pb = true; }
                                    Focus::Divisions => { ui_state.is_editing_divisions = true; ui_state.clear_divisions = true; }
                                    Focus::Interval => { ui_state.is_editing_interval = true; ui_state.clear_interval = true; }
                                    Focus::Mode => {
                                        let mut s = state_mutex.lock().unwrap();
                                        s.is_mpe = !s.is_mpe;
                                        if s.is_mpe {
                                            s.pitch_bend_range = 48;
                                            if let Some(conn) = &mut s.out_conn { send_mpe_configuration(conn, 15); }
                                            ui_state.logs.push("Switched to MPE. Pitch bend range locked to 48.".to_string());
                                        } else {
                                            s.pitch_bend_range = 12; 
                                            ui_state.logs.push("Switched to Multi-timbral. Pitch bend range reset to 12.".to_string());
                                        }
                                    }
                                    Focus::Channel(i) => {
                                        let mut s = state_mutex.lock().unwrap();
                                        if s.is_mpe && i == 0 { ui_state.logs.push("Channel 1 is MPE Master (cannot disable).".to_string()); } 
                                        else { s.channel_enabled[i] = !s.channel_enabled[i]; }
                                    }
                                    Focus::CommandInput => {
                                        let cmd = ui_state.input.trim().to_string();
                                        ui_state.input.clear();
                                        if cmd == "q" { return Ok(UiAction::Quit); }
                                        
                                        if cmd == "grid" || cmd == "0" {
                                            disable_raw_mode()?; execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                                            if cmd == "grid" {
                                                match setup_grid_tuning(state_mutex.clone()) {
                                                    Ok(msg) => ui_state.logs.push(msg),
                                                    Err(e) => ui_state.logs.push(format!("Grid Error: {}", e)),
                                                }
                                            } else if cmd == "0" {
                                                let scl_path = prompt_input("Enter path to .scl file: ");
                                                let scl_path = scl_path.trim_matches('"').trim_matches('\'');
                                                match parse_scl(scl_path) {
                                                    Ok(multipliers) => {
                                                        let kbm_path = prompt_input("Enter path to .kbm file: ");
                                                        let kbm_path = kbm_path.trim_matches('"').trim_matches('\'');
                                                        let kbm = if kbm_path.is_empty() { Kbm { map_size: 0, first_note: 0, last_note: 127, middle_note: 69, ref_note: 69, ref_freq: 440.0, formal_octave: (multipliers.len() - 1) as i32, mapping: vec![] } } 
                                                                  else { parse_kbm(kbm_path).unwrap_or(Kbm { map_size: 0, first_note: 0, last_note: 127, middle_note: 69, ref_note: 69, ref_freq: 440.0, formal_octave: 12, mapping: vec![] }) };
                                                        match apply_custom_tuning(state_mutex.clone(), &multipliers, &kbm) { Ok(_) => ui_state.logs.push(format!("Successfully loaded SCL tuning!")), Err(e) => ui_state.logs.push(format!("SCL Apply Error: {}", e)) }
                                                    }, Err(e) => ui_state.logs.push(format!("SCL Parse Error: {}", e))
                                                }
                                            }
                                            execute!(terminal.backend_mut(), EnterAlternateScreen)?; enable_raw_mode()?; terminal.clear()?;
                                        } else {
                                            if update_tuning(state_mutex.clone(), &cmd) { ui_state.logs.push(format!("Preset {} loaded.", cmd)); } 
                                            else { ui_state.logs.push(format!("Unknown command: {}", cmd)); }
                                        }
                                    }
                                }
                            }
                            KeyCode::Char(c) => { if ui_state.focus == Focus::CommandInput { ui_state.input.push(c); } }
                            KeyCode::Backspace => { if ui_state.focus == Focus::CommandInput { ui_state.input.pop(); } }
                            _ => {}
                        }
                    }
                }
            }
        } else {
            let mut s = state_mutex.lock().unwrap();
            if s.input_flash > 0 { s.input_flash -= 1; }
            for f in &mut s.output_flash { if *f > 0 { *f -= 1; } }
        }
    }
}