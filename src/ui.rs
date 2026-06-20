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
use crate::tuning::{prompt_input, apply_grid_tuning, update_tuning, parse_scl, parse_kbm, apply_custom_tuning, apply_equal_division, Kbm};

#[derive(PartialEq, Clone, Copy)]
pub enum Focus {
    Input, Output, Mode, PitchBend, Channel(usize),
    Divisions, Interval,
    
    // New Grid Focus States
    GridEdo, GridRefMidi, GridRefPitch, GridHoriz, GridCapo, GridOctave,
    GridUnequalToggle, GridUnequal(usize), GridOpen(usize),
    
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

    // New Grid Buffers
    pub grid_edo: String,
    pub grid_ref_midi: String,
    pub grid_ref_pitch: String,
    pub grid_horiz: String,
    pub grid_capo: String,
    pub grid_octave: String,
    pub grid_open: [String; 8],
    pub grid_unequal: [String; 9],
    pub grid_unequal_toggle: bool,
    pub is_editing_grid: bool,
    pub clear_grid: bool,

    pub dropdown_index: usize,
    pub in_ports: Vec<String>,
    pub out_ports: Vec<String>,
    pub selected_in: usize,
    pub selected_out: usize,
    pub input: String,
    pub logs: Vec<String>,
}

pub enum UiAction { None, Quit, ChangeInput(usize), ChangeOutput(usize) }

// --- Helper function to render underlined labels ---
pub fn render_labeled(text: &str, hotkey_idx: usize) -> Vec<Span> {
    let (first, rest) = text.split_at(hotkey_idx);
    let (hotkey, last) = rest.split_at(1);
    vec![
        Span::raw(first),
        Span::styled(hotkey, Style::default().add_modifier(Modifier::UNDERLINED)),
        Span::raw(last),
    ]
}

pub fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ui_state: &mut UiState,
    state_mutex: Arc<Mutex<MidiState>>
) -> Result<UiAction, Box<dyn std::error::Error>> {
    
    // Helper to format edit boxes dynamically
    let fmt_box = |ui_state: &UiState, focus: Focus, val: &str| -> Span {
        let is_focused = ui_state.focus == focus;
        let text = if ui_state.is_editing_grid && is_focused { format!("<{}_>", val) } else { format!("[{}]", val) };
        let style = if is_focused { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
        Span::styled(text, style)
    };

    loop {
        terminal.draw(|f| {
            // --- 1. MAIN VERTICAL LAYOUT ---
            let main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(7),  // [0] Settings
                    Constraint::Length(3),  // [1] Presets (New)
                    Constraint::Min(13),    // [2] Middle Content (ED/Grid + File)
                    Constraint::Length(6),  // [3] Logs (Short, near bottom)
                    Constraint::Length(3)   // [4] Command Input
                ].as_ref())
                .split(f.size());

            // --- 2. MIDDLE HORIZONTAL SPLIT ---
            let middle_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(50), // [0] Left Half (ED + Grid)
                    Constraint::Percentage(50), // [1] Right Half (File)
                ].as_ref())
                .split(main_chunks[2]);

            // --- 3. LEFT VERTICAL SPLIT ---
            let left_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // [0] Equal Division
                    Constraint::Min(10),   // [1] Guitar Grid
                ].as_ref())
                .split(middle_chunks[0]);

            let mut midi_state = state_mutex.lock().unwrap();

            // --- SETTINGS PANEL ---
            let mut top_row = vec![];

            top_row.extend(render_labeled("Input Device: ", 0));
            let in_str = if ui_state.is_editing_dropdown && ui_state.focus == Focus::Input { format!("< {} >", ui_state.in_ports.get(ui_state.dropdown_index).unwrap_or(&"None".to_string())) } else { format!("[ {} ]", ui_state.in_ports.get(ui_state.selected_in).unwrap_or(&"None".to_string())) };
            let in_style = if ui_state.focus == Focus::Input { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            top_row.push(Span::styled(in_str, in_style)); top_row.push(Span::raw("   "));

            top_row.extend(render_labeled("Output Device: ", 0));
            let out_str = if ui_state.is_editing_dropdown && ui_state.focus == Focus::Output { format!("< {} >", ui_state.out_ports.get(ui_state.dropdown_index).unwrap_or(&"None".to_string())) } else { format!("[ {} ]", ui_state.out_ports.get(ui_state.selected_out).unwrap_or(&"None".to_string())) };
            let out_style = if ui_state.focus == Focus::Output { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            top_row.push(Span::styled(out_str, out_style)); top_row.push(Span::raw("   "));

            top_row.extend(render_labeled("Output Type: ", 7));
            let mode_str = if midi_state.is_mpe { "< MPE >" } else { "< Multi-timbral >" };
            let mode_style = if ui_state.focus == Focus::Mode { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            top_row.push(Span::styled(mode_str, mode_style)); top_row.push(Span::raw("   "));

            top_row.extend(render_labeled("PB Range: ", 0));
            let pb_str = if ui_state.is_editing_pb { format!("< {}_ >", ui_state.pb_input) } else { format!("[ {} ]", midi_state.pitch_bend_range) };
            let pb_style = if ui_state.focus == Focus::PitchBend { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            top_row.push(Span::styled(pb_str, pb_style));

            let mut dots_row = vec![Span::raw("MIDI In: ")];
            let in_dot_color = if midi_state.input_flash > 0 { Color::Cyan } else { Color::DarkGray };
            dots_row.push(Span::styled("•", Style::default().fg(in_dot_color))); 
            dots_row.extend(render_labeled("   Channels Out: ",4));
            for i in 0..16 {
                let mut dot_style = Style::default();
                if midi_state.output_flash[i] > 0 { dot_style = dot_style.fg(Color::White).add_modifier(Modifier::BOLD); } 
                else if midi_state.channel_enabled[i] { dot_style = dot_style.fg(Color::Gray); } else { dot_style = dot_style.fg(Color::DarkGray); }
                if ui_state.focus == Focus::Channel(i) { dot_style = dot_style.bg(Color::DarkGray); }
                dots_row.push(Span::styled("• ", dot_style));
            }

            // Render to main_chunks[0]
            f.render_widget(Paragraph::new(vec![Line::raw(""), Line::from(top_row), Line::raw(""), Line::from(dots_row)]).block(Block::default().title(" Settings ").borders(Borders::ALL)).wrap(Wrap { trim: true }), main_chunks[0]);

            // --- PRESETS PANEL (NEW) ---
            let presets_text = "  1     2     3     4     5     6     7     8     9  ";
            f.render_widget(Paragraph::new(presets_text).block(Block::default().title(" Presets ").borders(Borders::ALL)), main_chunks[1]);

            // --- EQUAL DIVISION PANEL ---
            let mut ed_row = vec![];

            ed_row.extend(render_labeled("Divisions: ", 0));
            let div_str = if ui_state.is_editing_divisions { format!("< {}_ >", ui_state.divisions_input) } else { format!("[ {} ]", ui_state.divisions_input) };
            let div_style = if ui_state.focus == Focus::Divisions { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            ed_row.push(Span::styled(div_str, div_style)); ed_row.push(Span::raw("      "));

            ed_row.extend(render_labeled("Interval to Divide: ", 1));
            let int_str = if ui_state.is_editing_interval { format!("< {}_ >", ui_state.interval_input) } else { format!("[ {} ]", ui_state.interval_input) };
            let int_style = if ui_state.focus == Focus::Interval { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
            ed_row.push(Span::styled(int_str, int_style));
            
            // Render to left_chunks[0]
            f.render_widget(Paragraph::new(Line::from(ed_row)).block(Block::default().title(" Equal Division ").borders(Borders::ALL)), left_chunks[0]);

            // --- GUITAR GRID PANEL ---
            let grid_block = Block::default().title(" Guitar Grid ").borders(Borders::ALL);
            // Render to left_chunks[1]
            let inner_grid_area = grid_block.inner(left_chunks[1]);
            f.render_widget(grid_block, left_chunks[1]);

            // Split Grid into Left (Strings) and Right (Math parameters)
            let grid_splits = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Length(16), Constraint::Min(1)].as_ref()).split(inner_grid_area);

            // Left Side: 8 Open Strings
            let mut string_lines = vec![];
            for i in 0..8 {
                let string_num = 8 - i;
                let mut line = Vec::new();

                if i == 0 {
                    line.push(Span::styled("S", Style::default().add_modifier(Modifier::UNDERLINED)));
                } else {
                    line.push(Span::raw("S"));
                }

                line.push(Span::raw(format!("{}: ", string_num)));
                line.push(fmt_box(ui_state, Focus::GridOpen(i), &ui_state.grid_open[i]));
                string_lines.push(Line::from(line));
            }
            f.render_widget(Paragraph::new(string_lines), grid_splits[0]);
            
            // Right Side: Grid Parameters
            let mut g_row1 = vec![];
            g_row1.extend(render_labeled("EDO: ",0)); g_row1.push(fmt_box(ui_state, Focus::GridEdo, &ui_state.grid_edo));
            g_row1.extend(render_labeled("  Ref MIDI: ", 0)); g_row1.push(fmt_box(ui_state, Focus::GridRefMidi, &ui_state.grid_ref_midi));
            g_row1.extend(render_labeled("  Ref Hz: ", 7)); g_row1.push(fmt_box(ui_state, Focus::GridRefPitch, &ui_state.grid_ref_pitch));
            
            let mut g_row2 = vec![];
            g_row2.extend(render_labeled("Horiz Step: ", 0));
            let mut h_step_span = fmt_box(ui_state, Focus::GridHoriz, &ui_state.grid_horiz);
            if ui_state.grid_unequal_toggle { h_step_span = Span::styled(format!("[ {} ]", ui_state.grid_horiz), Style::default().fg(Color::DarkGray)); }
            g_row2.push(h_step_span);
            g_row2.extend(render_labeled("  Capo: ", 3)); g_row2.push(fmt_box(ui_state, Focus::GridCapo, &ui_state.grid_capo));
            g_row2.extend(render_labeled("  Octave: ", 6)); g_row2.push(fmt_box(ui_state, Focus::GridOctave, &ui_state.grid_octave));

            let checkbox = if ui_state.grid_unequal_toggle { "[x] " } else { "[ ] " };

            let focus_style = if ui_state.focus == Focus::GridUnequalToggle { 
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) 
            } else { 
                Style::default() 
            };

            let mut g_row3 = vec![Span::styled(checkbox, focus_style)];
            g_row3.extend(render_labeled("Unequal Frets", 0));

            for span in &mut g_row3[1..] {
                span.style = span.style.patch(focus_style);
            }
            let mut g_row4 = vec![Span::raw("Steps: ")];
            if ui_state.grid_unequal_toggle {
                for i in 0..9 {
                    g_row4.push(fmt_box(ui_state, Focus::GridUnequal(i), &ui_state.grid_unequal[i]));
                    g_row4.push(Span::raw(" "));
                }
            }

            f.render_widget(Paragraph::new(vec![Line::from(g_row1), Line::from(g_row2), Line::raw(""), Line::from(g_row3), Line::from(g_row4)]), grid_splits[1]);

            // --- FILE PANEL (NEW) ---
            // Render to middle_chunks[1]
            f.render_widget(Paragraph::new("Placeholder for File I/O").block(Block::default().title(" File ").borders(Borders::ALL)), middle_chunks[1]);

            // --- LOGS PANEL ---
            // Render to main_chunks[3]
            let log_text = ui_state.logs.iter().cloned().collect::<Vec<String>>().join("\n");
            f.render_widget(Paragraph::new(log_text).block(Block::default().title(" Logs ").borders(Borders::ALL)), main_chunks[3]);

            // --- COMMAND INPUT ---
            // Render to main_chunks[4]
            let input_style = if ui_state.focus == Focus::CommandInput { Style::default().fg(Color::Yellow) } else { Style::default() };
            f.render_widget(Paragraph::new(format!("> {}", ui_state.input)).style(input_style).block(Block::default().title(" Command Input (Presets 1-9, '0', 'q') ").borders(Borders::ALL)), main_chunks[4]);
        })?;

        if event::poll(Duration::from_millis(30))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if let KeyCode::Char(c) = key.code {
                        // Only jump if we aren't currently editing a specific text box
                        if !ui_state.is_editing_grid && !ui_state.is_editing_divisions && 
                           !ui_state.is_editing_interval && !ui_state.is_editing_dropdown && 
                           !ui_state.is_editing_pb {
                            
                            let new_focus = match c {
                                'i' => Some(Focus::Input),
                                'o' => Some(Focus::Output),
                                't' => Some(Focus::Mode),
                                'p' => Some(Focus::PitchBend),
                                'c' => Some(Focus::Channel(0)),
                                'd' => Some(Focus::Divisions),
                                'n' => Some(Focus::Interval),
                                'e' => Some(Focus::GridEdo),
                                'r' => Some(Focus::GridRefMidi),
                                'z' => Some(Focus::GridRefPitch),
                                'h' => Some(Focus::GridHoriz),
                                'a' => Some(Focus::GridCapo),
                                'v' => Some(Focus::GridOctave),
                                'u' => Some(Focus::GridUnequalToggle),
                                's' => Some(Focus::GridOpen(0)),
                                _ => None,
                            };

                            if let Some(f) = new_focus {
                                ui_state.focus = f;
                                continue; // Jumped, skip further key processing
                            }
                        }
                    }                    
                    if ui_state.is_editing_pb {
                        match key.code {
                            KeyCode::Char(c) if c.is_ascii_digit() => { if ui_state.pb_input.len() < 3 { ui_state.pb_input.push(c); } }
                            KeyCode::Backspace => { ui_state.pb_input.pop(); }
                            KeyCode::Enter => {
                                if let Ok(val) = ui_state.pb_input.parse::<u8>() {
                                    state_mutex.lock().unwrap().pitch_bend_range = val.clamp(1, 96); 
                                    ui_state.logs.push(format!("Pitch bend range updated to {}", val.clamp(1, 96)));
                                }
                                ui_state.is_editing_pb = false;
                            }
                            KeyCode::Esc => { ui_state.is_editing_pb = false; }
                            _ => {}
                        }
                    } else if ui_state.is_editing_divisions || ui_state.is_editing_interval {
                        let (buf, flag) = if ui_state.is_editing_divisions { (&mut ui_state.divisions_input, &mut ui_state.clear_divisions) } else { (&mut ui_state.interval_input, &mut ui_state.clear_interval) };
                        match key.code {
                            KeyCode::Char(c) => { if *flag { buf.clear(); *flag = false; } buf.push(c); }
                            KeyCode::Backspace => { buf.pop(); }
                            KeyCode::Enter => {
                                ui_state.is_editing_divisions = false; ui_state.is_editing_interval = false;
                                match apply_equal_division(state_mutex.clone(), &ui_state.divisions_input, &ui_state.interval_input) {
                                    Ok(msg) => ui_state.logs.push(msg), Err(e) => ui_state.logs.push(format!("ED Error: {}", e)),
                                }
                            }
                            KeyCode::Esc => { ui_state.is_editing_divisions = false; ui_state.is_editing_interval = false; }
                            _ => {}
                        }
                    } else if ui_state.is_editing_grid {
                        match key.code {
                            KeyCode::Char(c) => {
                                if ui_state.clear_grid {
                                    match ui_state.focus {
                                        Focus::GridEdo => ui_state.grid_edo.clear(), Focus::GridRefMidi => ui_state.grid_ref_midi.clear(),
                                        Focus::GridRefPitch => ui_state.grid_ref_pitch.clear(), Focus::GridHoriz => ui_state.grid_horiz.clear(),
                                        Focus::GridCapo => ui_state.grid_capo.clear(), Focus::GridOctave => ui_state.grid_octave.clear(),
                                        Focus::GridOpen(i) => ui_state.grid_open[i].clear(), Focus::GridUnequal(i) => ui_state.grid_unequal[i].clear(),
                                        _ => {}
                                    }
                                    ui_state.clear_grid = false;
                                }
                                match ui_state.focus {
                                    Focus::GridEdo => ui_state.grid_edo.push(c), Focus::GridRefMidi => ui_state.grid_ref_midi.push(c),
                                    Focus::GridRefPitch => ui_state.grid_ref_pitch.push(c), Focus::GridHoriz => ui_state.grid_horiz.push(c),
                                    Focus::GridCapo => ui_state.grid_capo.push(c), Focus::GridOctave => ui_state.grid_octave.push(c),
                                    Focus::GridOpen(i) => ui_state.grid_open[i].push(c), Focus::GridUnequal(i) => ui_state.grid_unequal[i].push(c),
                                    _ => {}
                                }
                            }
                            KeyCode::Backspace => {
                                match ui_state.focus {
                                    Focus::GridEdo => { ui_state.grid_edo.pop(); }, Focus::GridRefMidi => { ui_state.grid_ref_midi.pop(); },
                                    Focus::GridRefPitch => { ui_state.grid_ref_pitch.pop(); }, Focus::GridHoriz => { ui_state.grid_horiz.pop(); },
                                    Focus::GridCapo => { ui_state.grid_capo.pop(); }, Focus::GridOctave => { ui_state.grid_octave.pop(); },
                                    Focus::GridOpen(i) => { ui_state.grid_open[i].pop(); }, Focus::GridUnequal(i) => { ui_state.grid_unequal[i].pop(); },
                                    _ => {}
                                }
                            }
                            KeyCode::Enter => {
                                ui_state.is_editing_grid = false;
                                let horiz = if ui_state.grid_unequal_toggle { ui_state.grid_unequal.to_vec() } else { vec![ui_state.grid_horiz.clone()] };
                                match apply_grid_tuning(state_mutex.clone(), &ui_state.grid_edo, &ui_state.grid_ref_midi, &ui_state.grid_ref_pitch, &ui_state.grid_open, &horiz, &ui_state.grid_capo, &ui_state.grid_octave) {
                                    Ok(msg) => ui_state.logs.push(msg), Err(e) => ui_state.logs.push(format!("Grid Error: {}", e)),
                                }
                            }
                            KeyCode::Esc => { ui_state.is_editing_grid = false; }
                            _ => {}
                        }
                    } else if ui_state.is_editing_dropdown {
                        match key.code {
                            KeyCode::Up => { let max = if ui_state.focus == Focus::Input { ui_state.in_ports.len() } else { ui_state.out_ports.len() }; if max > 0 { ui_state.dropdown_index = ui_state.dropdown_index.saturating_sub(1); } }
                            KeyCode::Down => { let max = if ui_state.focus == Focus::Input { ui_state.in_ports.len() } else { ui_state.out_ports.len() }; if max > 0 && ui_state.dropdown_index < max - 1 { ui_state.dropdown_index += 1; } }
                            KeyCode::Enter => {
                                ui_state.is_editing_dropdown = false;
                                if ui_state.focus == Focus::Input { return Ok(UiAction::ChangeInput(ui_state.dropdown_index)); } else if ui_state.focus == Focus::Output { return Ok(UiAction::ChangeOutput(ui_state.dropdown_index)); }
                            }
                            KeyCode::Esc => { ui_state.is_editing_dropdown = false; }
                            _ => {}
                        }
                    } else {
                        // Global Navigation map
                        match key.code {
                            KeyCode::Left => {
                                ui_state.focus = match ui_state.focus {
                                    Focus::Input => Focus::Input, Focus::Output => Focus::Input, Focus::Mode => Focus::Output, Focus::PitchBend => Focus::Mode, Focus::Channel(0) => Focus::PitchBend, Focus::Channel(i) => Focus::Channel(i - 1),
                                    Focus::Divisions => Focus::Channel(15), Focus::Interval => Focus::Divisions,
                                    Focus::GridEdo => Focus::Interval, Focus::GridRefMidi => Focus::GridEdo, Focus::GridRefPitch => Focus::GridRefMidi, Focus::GridHoriz => Focus::GridRefPitch, Focus::GridCapo => if ui_state.grid_unequal_toggle { Focus::GridRefPitch } else { Focus::GridHoriz }, Focus::GridOctave => Focus::GridCapo, Focus::GridUnequalToggle => Focus::GridOctave, Focus::GridUnequal(0) => Focus::GridUnequalToggle, Focus::GridUnequal(i) => Focus::GridUnequal(i-1), Focus::GridOpen(0) => if ui_state.grid_unequal_toggle { Focus::GridUnequal(8) } else { Focus::GridUnequalToggle }, Focus::GridOpen(i) => Focus::GridOpen(i-1),
                                    Focus::CommandInput => Focus::GridOpen(7),
                                };
                            }
                            KeyCode::Right => {
                                ui_state.focus = match ui_state.focus {
                                    Focus::Input => Focus::Output, Focus::Output => Focus::Mode, Focus::Mode => Focus::PitchBend, Focus::PitchBend => Focus::Channel(0), Focus::Channel(15) => Focus::Divisions, Focus::Channel(i) => Focus::Channel(i + 1),
                                    Focus::Divisions => Focus::Interval, Focus::Interval => Focus::GridEdo,
                                    Focus::GridEdo => Focus::GridRefMidi, Focus::GridRefMidi => Focus::GridRefPitch, Focus::GridRefPitch => if ui_state.grid_unequal_toggle { Focus::GridCapo } else { Focus::GridHoriz }, Focus::GridHoriz => Focus::GridCapo, Focus::GridCapo => Focus::GridOctave, Focus::GridOctave => Focus::GridUnequalToggle, Focus::GridUnequalToggle => if ui_state.grid_unequal_toggle { Focus::GridUnequal(0) } else { Focus::GridOpen(0) }, Focus::GridUnequal(8) => Focus::GridOpen(0), Focus::GridUnequal(i) => Focus::GridUnequal(i+1), Focus::GridOpen(7) => Focus::CommandInput, Focus::GridOpen(i) => Focus::GridOpen(i+1),
                                    Focus::CommandInput => Focus::CommandInput,
                                };
                            }
                            KeyCode::Up => { 
                                ui_state.focus = match ui_state.focus {
                                    Focus::CommandInput => Focus::GridOpen(7),
                                    Focus::GridOpen(_) | Focus::GridEdo | Focus::GridRefMidi | Focus::GridRefPitch | Focus::GridHoriz | Focus::GridCapo | Focus::GridOctave | Focus::GridUnequalToggle | Focus::GridUnequal(_) => Focus::Divisions,
                                    Focus::Divisions | Focus::Interval => Focus::Channel(0),
                                    _ => ui_state.focus
                                };
                            }
                            KeyCode::Down => { 
                                ui_state.focus = match ui_state.focus {
                                    Focus::Input | Focus::Output | Focus::Mode | Focus::PitchBend | Focus::Channel(_) => Focus::Divisions,
                                    Focus::Divisions | Focus::Interval => Focus::GridEdo,
                                    Focus::GridEdo | Focus::GridRefMidi | Focus::GridRefPitch | Focus::GridHoriz | Focus::GridCapo | Focus::GridOctave | Focus::GridUnequalToggle | Focus::GridUnequal(_) => Focus::GridOpen(0),
                                    Focus::GridOpen(_) => Focus::CommandInput,
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
                                    Focus::GridEdo | Focus::GridRefMidi | Focus::GridRefPitch | Focus::GridHoriz | Focus::GridCapo | Focus::GridOctave | Focus::GridOpen(_) | Focus::GridUnequal(_) => {
                                        ui_state.is_editing_grid = true; ui_state.clear_grid = true;
                                    }
                                    Focus::GridUnequalToggle => {
                                        ui_state.grid_unequal_toggle = !ui_state.grid_unequal_toggle;
                                        let horiz = if ui_state.grid_unequal_toggle { ui_state.grid_unequal.to_vec() } else { vec![ui_state.grid_horiz.clone()] };
                                        match apply_grid_tuning(state_mutex.clone(), &ui_state.grid_edo, &ui_state.grid_ref_midi, &ui_state.grid_ref_pitch, &ui_state.grid_open, &horiz, &ui_state.grid_capo, &ui_state.grid_octave) {
                                            Ok(msg) => ui_state.logs.push(msg), Err(e) => ui_state.logs.push(format!("Grid Error: {}", e)),
                                        }
                                    }
                                    Focus::Mode => {
                                        let mut s = state_mutex.lock().unwrap(); s.is_mpe = !s.is_mpe;
                                        if s.is_mpe { s.pitch_bend_range = 48; if let Some(conn) = &mut s.out_conn { send_mpe_configuration(conn, 15); } ui_state.logs.push("Switched to MPE. Pitch bend range locked to 48.".to_string()); } 
                                        else { s.pitch_bend_range = 12; ui_state.logs.push("Switched to Multi-timbral. Pitch bend range reset to 12.".to_string()); }
                                    }
                                    Focus::Channel(i) => {
                                        let mut s = state_mutex.lock().unwrap();
                                        if s.is_mpe && i == 0 { ui_state.logs.push("Channel 1 is MPE Master (cannot disable).".to_string()); } 
                                        else { s.channel_enabled[i] = !s.channel_enabled[i]; }
                                    }
                                    Focus::CommandInput => {
                                        let cmd = ui_state.input.trim().to_string(); ui_state.input.clear();
                                        if cmd == "q" { return Ok(UiAction::Quit); }
                                        if cmd == "0" {
                                            disable_raw_mode()?; execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                                            let scl_path = prompt_input("Enter path to .scl file: "); let scl_path = scl_path.trim_matches('"').trim_matches('\'');
                                            match parse_scl(scl_path) {
                                                Ok(multipliers) => {
                                                    let kbm_path = prompt_input("Enter path to .kbm file: "); let kbm_path = kbm_path.trim_matches('"').trim_matches('\'');
                                                    let kbm = if kbm_path.is_empty() { Kbm { map_size: 0, first_note: 0, last_note: 127, middle_note: 69, ref_note: 69, ref_freq: 440.0, formal_octave: (multipliers.len() - 1) as i32, mapping: vec![] } } 
                                                              else { parse_kbm(kbm_path).unwrap_or(Kbm { map_size: 0, first_note: 0, last_note: 127, middle_note: 69, ref_note: 69, ref_freq: 440.0, formal_octave: 12, mapping: vec![] }) };
                                                    match apply_custom_tuning(state_mutex.clone(), &multipliers, &kbm) { Ok(_) => ui_state.logs.push(format!("Successfully loaded SCL tuning!")), Err(e) => ui_state.logs.push(format!("SCL Apply Error: {}", e)) }
                                                }, Err(e) => ui_state.logs.push(format!("SCL Parse Error: {}", e))
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