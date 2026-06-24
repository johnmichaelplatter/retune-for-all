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
    widgets::{Block, Borders, Paragraph, Wrap, Tabs},
    Terminal,
};
use ratatui_textarea::TextArea;

use crate::midi::{MidiState, send_mpe_configuration};
use crate::tuning::{
    prompt_input, apply_grid_tuning, update_tuning, parse_scl, parse_kbm, 
    apply_custom_tuning, apply_equal_division, Kbm, Preset, PresetsConfig
};

#[derive(PartialEq, Clone, Copy)]
pub enum Focus {
    Input, Output, Mode, PitchBend, Channel(usize),
    Divisions, Interval,
    GridEdo, GridRefMidi, GridRefPitch, GridHoriz, GridCapo, GridOctave,
    GridUnequalToggle, GridUnequal(usize), GridOpen(usize),
    CommandInput, Notepad,
}

pub struct UiState {
    pub focus: Focus,
    pub is_editing_dropdown: bool,
    pub is_editing_pb: bool,
    pub clear_pb: bool,
    pub pb_input: String,

    pub is_editing_divisions: bool,
    pub clear_divisions: bool,
    pub divisions_input: String,

    pub is_editing_interval: bool,
    pub clear_interval: bool,
    pub interval_input: String,

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

    pub active_file_tab: usize,
    pub is_typing_in_notepad: bool,
    pub scl_textarea: ratatui_textarea::TextArea<'static>,
    pub kbm_textarea: ratatui_textarea::TextArea<'static>,

    // --- Preset Additions ---
    pub presets: PresetsConfig,
    pub pending_action: Option<char>, // 'P' for load, 'S' for save
    pub active_tuning_mode: String,
}

impl Default for UiState {
    fn default() -> Self {
        let mut scl = TextArea::default();
        scl.set_block(Block::default().borders(Borders::TOP));
        scl.insert_str("!\n12 EDO\n12\n!\n100.0\n200.0\n300.0\n400.0\n500.0\n600.0\n700.0\n800.0\n900.0\n1000.0\n1100.0\n2/1\n"); 
        
        let mut kbm = TextArea::default();
        kbm.set_block(Block::default().borders(Borders::TOP));
        kbm.insert_str("! Default KBM\n0\n0\n127\n69\n69\n440.0\n12\n");

        Self {
            focus: Focus::CommandInput,
            is_editing_dropdown: false,
            is_editing_pb: false,
            clear_pb: false,
            pb_input: String::new(),

            is_editing_divisions: false,
            clear_divisions: false,
            divisions_input: "12".to_string(),

            is_editing_interval: false,
            clear_interval: false,
            interval_input: "2/1".to_string(),

            grid_edo: "41".to_string(),
            grid_ref_midi: "48".to_string(),
            grid_ref_pitch: "260.89".to_string(),
            grid_horiz: "2".to_string(),
            grid_capo: "0".to_string(),
            grid_octave: "0".to_string(),
            grid_open: ["13".to_string(), "0".to_string(), "-17".to_string(), "-28".to_string(), "-41".to_string(), "-52".to_string(), "-65".to_string(), "-82".to_string()],
            grid_unequal: ["2".to_string(), "2".to_string(), "2".to_string(), "2".to_string(), "2".to_string(), "2".to_string(), "2".to_string(), "2".to_string(), "2".to_string()],
            grid_unequal_toggle: false,
            is_editing_grid: false,
            clear_grid: false,

            dropdown_index: 0,
            in_ports: vec![],
            out_ports: vec![],
            selected_in: 0,
            selected_out: 0,
            input: String::new(),
            logs: vec!["Welcome to Poly-Router!".into(), "Navigate to Settings with Arrow Keys to Configure.".into()],            
            active_file_tab: 0,
            is_typing_in_notepad: false,
            scl_textarea: scl,
            kbm_textarea: kbm,

            presets: crate::tuning::load_presets_from_file(),
            pending_action: None,
            active_tuning_mode: "equal_division".to_string(),
        }
    }
}

pub enum UiAction { None, Quit, ChangeInput(usize), ChangeOutput(usize) }

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
    
    let fmt_box = |ui_state: &UiState, focus: Focus, val: &str| -> Span {
        let is_focused = ui_state.focus == focus;
        let text = if ui_state.is_editing_grid && is_focused { format!("<{}_>", val) } else { format!("[{}]", val) };
        let style = if is_focused { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
        Span::styled(text, style)
    };

    loop {
        terminal.draw(|f| {
            let main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(7),
                    Constraint::Length(3),
                    Constraint::Min(16),
                    Constraint::Length(5),
                    Constraint::Length(3)
                ].as_ref())
                .split(f.size());

            let middle_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(58),
                    Constraint::Percentage(42),
                ].as_ref())
                .split(main_chunks[2]);

            let left_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(12),
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

            f.render_widget(Paragraph::new(vec![Line::raw(""), Line::from(top_row), Line::raw(""), Line::from(dots_row)]).block(Block::default().title(" Settings ").borders(Borders::ALL).border_style(Style::default().fg(Color::Green))).wrap(Wrap { trim: true }), main_chunks[0]);

            // --- PRESETS PANEL ---
            let presets_text = "  1   2   3   4   5   6   7   8   9   |  Shift+P, Num to Load  |  Shift+S, Num to Save";
            let p_color = if ui_state.pending_action.is_some() { Color::Yellow } else { Color::Green };
            f.render_widget(Paragraph::new(presets_text).block(Block::default().title(" Presets ").borders(Borders::ALL).border_style(Style::default().fg(p_color))), main_chunks[1]);

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
            
            f.render_widget(Paragraph::new(Line::from(ed_row)).block(Block::default().title(" Equal Division ").borders(Borders::ALL).border_style(Style::default().fg(Color::Green))), left_chunks[0]);

            // --- GUITAR GRID PANEL ---
            let grid_block = Block::default().title(" Guitar Grid ").borders(Borders::ALL).border_style(Style::default().fg(Color::Green));
            let inner_grid_area = grid_block.inner(left_chunks[1]);
            f.render_widget(grid_block, left_chunks[1]);

            let grid_splits = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Length(12), Constraint::Min(1)].as_ref()).split(inner_grid_area);

            // Left Side: 8 Open Strings
            let mut string_lines = vec![];
            for i in 0..8 {
                let string_idx = 7 - i; 
                let string_num = string_idx + 1; 
                let mut line = Vec::new();

                if i == 0 { line.push(Span::styled("S", Style::default().add_modifier(Modifier::UNDERLINED))); } else { line.push(Span::raw("S")); }

                line.push(Span::raw(format!("{}: ", string_num)));
                line.push(fmt_box(ui_state, Focus::GridOpen(string_idx), &ui_state.grid_open[string_idx]));
                string_lines.push(Line::from(line));
            }
            f.render_widget(Paragraph::new(string_lines), grid_splits[0]);
            
            // Right Side: Grid Parameters
            let mut g_row1 = vec![];
            g_row1.extend(render_labeled("EDO: ",0)); g_row1.push(fmt_box(ui_state, Focus::GridEdo, &ui_state.grid_edo));
            g_row1.extend(render_labeled("  Ref MIDI: ",