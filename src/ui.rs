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
    }
}