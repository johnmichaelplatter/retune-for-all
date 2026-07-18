use std::time::Duration;
use std::sync::{Arc, Mutex};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Modifier},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap, Tabs},
    Terminal, Frame,
};
use ratatui_textarea::TextArea;
use tui_menu::{Menu, MenuItem, MenuState, MenuEvent};

use crate::midi::{MidiState, send_mpe_configuration};
use crate::tuning::{
    prompt_input, apply_grid_tuning, update_tuning, 
    apply_equal_division, Preset, PresetsConfig
};

#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    SelectInput(usize),
    SelectOutput(usize),
}

#[derive(PartialEq, Clone, Copy)]
pub enum Focus {
    Input, Output, Mode, PitchBend, Channel(usize),
    Divisions, Interval,
    GridModeToggle, GridEdo, GridRefMidi, GridRefPitch, GridHoriz, GridCapo, GridOctave,
    GridUnequalToggle, GridUnequal(usize), GridOpen(usize),
    CommandInput, Notepad,
}

pub struct UiState {
    pub focus: Focus,
    pub is_menu_active: bool,
    pub menu_state: MenuState<MenuAction>,
    pub is_editing_pb: bool,
    pub clear_pb: bool,
    pub pb_input: String,

    pub is_editing_divisions: bool,
    pub clear_divisions: bool,
    pub divisions_input: String,

    pub is_editing_interval: bool,
    pub clear_interval: bool,
    pub interval_input: String,

    pub grid_is_ji: bool, // NEW: Tracks EDO vs JI Mode
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

    pub presets: PresetsConfig,
    pub pending_action: Option<char>, 
    pub active_tuning_mode: String,
}

impl Default for UiState {
    fn default() -> Self {
        let mut scl = TextArea::default();
        scl.set_block(Block::default().borders(Borders::TOP));
        scl.insert_str("!\n12 EDO\n12\n!\n100.0\n200.0\n300.0\n400.0\n500.0\n600.0\n700.0\n800.0\n900.0\n1000.0\n1100.0\n2/1\n"); 
        
        let mut kbm = TextArea::default();
        kbm.set_block(Block::default().borders(Borders::TOP));
        kbm.insert_str("! Default KBM\n!Number of Notes\n0\n!Bottom Note\n0\n!Top Note\n127 \n!MIDI note for mapping\n69 \n!MIDI note for Tuning\n69 \n!Tuning Frequency\n440.0 \n!Formal Octave:\n12 \n!Mapping list:");
        


        Self {
            focus: Focus::CommandInput,
            
            is_menu_active: false,
            menu_state: MenuState::new(vec![]),

            is_editing_pb: false,
            clear_pb: false,
            pb_input: String::new(),

            is_editing_divisions: false,
            clear_divisions: false,
            divisions_input: "12".to_string(),

            is_editing_interval: false,
            clear_interval: false,
            interval_input: "2/1".to_string(),

            grid_is_ji: false,
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

            in_ports: vec![],
            out_ports: vec![],
            selected_in: 0,
            selected_out: 0,
            input: String::new(),
            logs: vec!["Welcome to Retune for All!".into(), "Navigate to Settings with Arrow Keys to Configure.".into()],            
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

impl UiState {
    pub fn load_preset(&mut self, state_mutex: Arc<Mutex<MidiState>>, preset_key: &str) -> Option<UiAction> {
        if let Some(preset) = self.presets.get(preset_key).cloned() {
            self.pb_input = preset.pb_range.to_string();
            self.divisions_input = preset.equal_division.divisions.clone();
            self.interval_input = preset.equal_division.interval_to_divide.clone();
            
            self.grid_is_ji = preset.guitar_grid.is_ji;
            self.grid_edo = preset.guitar_grid.edo.clone();
            self.grid_ref_midi = preset.guitar_grid.ref_midi.clone();
            self.grid_ref_pitch = preset.guitar_grid.ref_hz.clone();
            self.grid_horiz = preset.guitar_grid.horiz_step.clone();
            self.grid_capo = preset.guitar_grid.capo.clone();
            self.grid_octave = preset.guitar_grid.octave.clone();
            self.grid_unequal_toggle = preset.guitar_grid.unequal_frets_on;
            
            for (i, val) in preset.guitar_grid.unequal_frets.iter().enumerate().take(9) { self.grid_unequal[i] = val.clone(); }
            for (i, val) in preset.guitar_grid.open_strings.iter().enumerate().take(8) { self.grid_open[i] = val.clone(); }
            
            self.scl_textarea = TextArea::new(preset.file.scl.lines().map(|s| s.to_string()).collect());
            self.kbm_textarea = TextArea::new(preset.file.kbm.lines().map(|s| s.to_string()).collect());
            
            let mut in_changed = false;
            let mut out_changed = false;

            if let Some(idx) = self.in_ports.iter().position(|p| p.contains(&preset.input_device)) { 
                if self.selected_in != idx {
                    self.selected_in = idx; 
                    in_changed = true;
                }
            }
            if let Some(idx) = self.out_ports.iter().position(|p| p.contains(&preset.output_device)) { 
                if self.selected_out != idx {
                    self.selected_out = idx; 
                    out_changed = true;
                }
            }

            let mut s = state_mutex.lock().unwrap();
            s.pitch_bend_range = preset.pb_range;
            s.is_mpe = preset.output_type.to_lowercase().contains("mpe");
            for i in 0..16 { s.channel_enabled[i] = preset.channels_out.contains(&(i + 1)); }
            drop(s);
            
            self.active_tuning_mode = preset.active.clone();
            match preset.active.as_str() {
                "equal_division" => { let _ = apply_equal_division(state_mutex.clone(), &self.divisions_input, &self.interval_input); }
                "guitar_grid" => {
                    let horiz = if self.grid_unequal_toggle { self.grid_unequal.to_vec() } else { vec![self.grid_horiz.clone()] };
                    let _ = apply_grid_tuning(state_mutex.clone(), self.grid_is_ji, self.grid_unequal_toggle, &self.grid_edo, &self.grid_ref_midi, &self.grid_ref_pitch, &self.grid_open, &horiz, &self.grid_capo, &self.grid_octave);
                }
                "file" => { let _ = crate::tuning::sync_notepad_tuning(state_mutex.clone(), self.scl_textarea.lines(), self.kbm_textarea.lines()); }
                _ => {}
            }
            self.logs.push(format!("Loaded {}", preset_key));
            
            if in_changed || out_changed {
                return Some(UiAction::ChangeBoth(self.selected_in, self.selected_out));
            }
        } else {
            self.logs.push(format!("{} not found.", preset_key));
        }
        None
    }

    pub fn save_preset(&mut self, state_mutex: Arc<Mutex<MidiState>>, preset_key: &str) {
        let s = state_mutex.lock().unwrap();
        let mut channels_out = Vec::new();
        for i in 0..16 { if s.channel_enabled[i] { channels_out.push(i + 1); } }
        
        let new_preset = Preset {
            input_device: self.in_ports.get(self.selected_in).cloned().unwrap_or_default(),
            output_device: self.out_ports.get(self.selected_out).cloned().unwrap_or_default(),
            output_type: if s.is_mpe { "MPE".to_string() } else { "Multi-timbral".to_string() },
            pb_range: s.pitch_bend_range,
            channels_out,
            active: self.active_tuning_mode.clone(),
            equal_division: crate::tuning::PresetEqualDivision { divisions: self.divisions_input.clone(), interval_to_divide: self.interval_input.clone() },
            guitar_grid: crate::tuning::PresetGuitarGrid {
                is_ji: self.grid_is_ji,
                edo: self.grid_edo.clone(), ref_midi: self.grid_ref_midi.clone(), ref_hz: self.grid_ref_pitch.clone(), horiz_step: self.grid_horiz.clone(),
                capo: self.grid_capo.clone(), octave: self.grid_octave.clone(), unequal_frets_on: self.grid_unequal_toggle,
                unequal_frets: self.grid_unequal.to_vec(), open_strings: self.grid_open.to_vec(),
            },
            file: crate::tuning::PresetFile { scl: self.scl_textarea.lines().join("\n"), kbm: self.kbm_textarea.lines().join("\n") }
        };
        
        self.presets.insert(preset_key.to_string(), new_preset);
        match crate::tuning::save_presets_to_file(&self.presets) {
            Ok(_) => self.logs.push(format!("Saved {}", preset_key)),
            Err(e) => self.logs.push(format!("Save failed: {}", e)),
        }
    }
}

pub enum UiAction { Quit, ChangeInput(usize), ChangeOutput(usize), ChangeBoth(usize, usize) }

pub fn render_labeled(text: &str, hotkey_idx: usize) -> Vec<Span<'_>> {
    let (first, rest) = text.split_at(hotkey_idx);
    let (hotkey, last) = rest.split_at(1);
    vec![
        Span::raw(first),
        Span::styled(hotkey, Style::default().add_modifier(Modifier::UNDERLINED)),
        Span::raw(last),
    ]
}

fn fmt_box<'a>(ui_state: &'a UiState, focus: Focus, val: &'a str) -> Span<'a> {
    let is_focused = ui_state.focus == focus;
    let text = if ui_state.is_editing_grid && is_focused { format!("<{}_>", val) } else { format!("[{}]", val) };
    let style = if is_focused { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
    Span::styled(text, style)
}

fn move_focus(current: Focus, direction: &str, ui_state: &UiState) -> Focus {
    match direction {
        "Right" => match current {
            Focus::Input => Focus::Output,
            Focus::Output => Focus::Mode,
            Focus::Mode => Focus::PitchBend,
            Focus::PitchBend => Focus::Channel(0),
            Focus::Channel(i) if i < 15 => Focus::Channel(i + 1),
            Focus::GridUnequal(8) | Focus::GridOctave | Focus::GridRefPitch | Focus::GridUnequalToggle | Focus::Interval => Focus::Notepad,
            Focus::Divisions => Focus::Interval,
            Focus::GridOpen(0) => Focus::GridModeToggle,
            Focus::GridModeToggle => if ui_state.grid_is_ji { Focus::GridRefMidi } else { Focus::GridEdo },
            Focus::GridEdo => Focus::GridRefMidi,
            Focus::GridRefMidi => Focus::GridRefPitch,
            Focus::GridHoriz => Focus::GridCapo,
            Focus::GridCapo => Focus::GridOctave,
            Focus::GridUnequal(i) if i < 8 => Focus::GridUnequal(i + 1),
            Focus::GridOpen(1) => if ui_state.grid_unequal_toggle {Focus::GridCapo} else {Focus::GridHoriz},
            Focus::GridOpen(3) => Focus::GridUnequalToggle,
            Focus::GridOpen(4) if ui_state.grid_unequal_toggle => if ui_state.grid_is_ji { Focus::GridModeToggle } else { Focus::GridEdo },
            Focus::GridOpen(i) if i > 3 => Focus::GridUnequalToggle,
            _ => current,
        },
        "Left" => match current {
            Focus::Output => Focus::Input,
            Focus::Mode => Focus::Output,
            Focus::PitchBend => Focus::Mode,
            Focus::Channel(0) => Focus::PitchBend,
            Focus::Channel(i) if i > 0 => Focus::Channel(i - 1),
            Focus::Notepad => Focus::Divisions, 
            Focus::Interval => Focus::Divisions,
            Focus::GridRefMidi => if ui_state.grid_is_ji { Focus::GridModeToggle } else { Focus::GridEdo },
            Focus::GridEdo => Focus::GridModeToggle,
            Focus::GridModeToggle => Focus::GridOpen(0),
            Focus::GridRefPitch => Focus::GridRefMidi,
            Focus::GridHoriz => Focus::GridOpen(1),
            Focus::GridCapo => if ui_state.grid_unequal_toggle {Focus::GridOpen(1)} else {Focus::GridHoriz},
            Focus::GridOctave => Focus::GridCapo,
            Focus::GridUnequal(0) => Focus::GridUnequalToggle,
            Focus::GridUnequal(i) if i > 0 => Focus::GridUnequal(i - 1),
            Focus::GridUnequalToggle => Focus::GridOpen(3),
            _ => current,
        },
        "Up" => match current {
            Focus::Divisions | Focus::Interval => Focus::Channel(0),
            Focus::GridModeToggle => Focus::Divisions,
            Focus::GridEdo => Focus::Divisions,
            Focus::GridRefMidi => Focus::Interval,
            Focus::GridRefPitch => Focus::Interval,
            Focus::GridCapo => if ui_state.grid_unequal_toggle {Focus::GridOpen(1)} else {Focus::GridRefMidi},
            Focus::GridOctave => Focus::GridRefPitch,
            Focus::Channel(_) => Focus::Input,
            Focus::CommandInput => Focus::GridOpen(7),
            Focus::GridOpen(i) if i > 0 => Focus::GridOpen(i - 1),
            Focus::GridOpen(0) => Focus::Divisions,
            Focus::GridUnequal(_) => Focus::GridUnequalToggle,
            Focus::GridUnequalToggle => if ui_state.grid_unequal_toggle { if ui_state.grid_is_ji { Focus::GridModeToggle } else { Focus::GridEdo } } else {Focus::GridHoriz},
            Focus::Notepad => Focus::Channel(0), 
            _ => current,
        },
        "Down" => match current {
            Focus::Input | Focus::Output | Focus::Mode | Focus::PitchBend | Focus::Channel(_) => Focus::Divisions,
            Focus::Notepad => Focus::CommandInput,
            Focus::GridModeToggle => if ui_state.grid_unequal_toggle {Focus::GridUnequalToggle} else {Focus::GridHoriz},
            Focus::GridEdo => if ui_state.grid_unequal_toggle {Focus::GridUnequalToggle} else {Focus::GridHoriz},
            Focus::GridOpen(i) if i < 7 => Focus::GridOpen(i + 1),
            Focus::GridOpen(7) => Focus::CommandInput,
            Focus::GridHoriz => Focus::GridUnequalToggle,
            Focus::GridCapo => Focus::GridUnequalToggle,
            Focus::GridOctave => Focus::GridUnequalToggle,
            Focus::Divisions => Focus::GridOpen(0),
            Focus::Interval => Focus::GridRefMidi,
            Focus::GridRefMidi => Focus::GridCapo,
            Focus::GridRefPitch => Focus::GridOctave,
            Focus::GridUnequalToggle => if ui_state.grid_unequal_toggle {Focus::GridUnequal(0)} else {Focus::CommandInput},
            Focus::GridUnequal(_) => Focus::CommandInput,
            _ => current,
        },
        _ => current,
    }
}

// --- UI COMPONENT RENDERING ---

fn render_settings_panel(f: &mut Frame, ui_state: &UiState, midi_state: &MidiState, area: Rect) {
    let mut top_row = vec![];

    top_row.extend(render_labeled("Input Device: ", 0));
    let in_str = format!("[ {} ]", ui_state.in_ports.get(ui_state.selected_in).unwrap_or(&"None".to_string()));
    let in_style = if ui_state.focus == Focus::Input { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
    top_row.push(Span::styled(in_str, in_style)); top_row.push(Span::raw("   "));

    top_row.extend(render_labeled("Output Device: ", 0));
    let out_str = format!("[ {} ]", ui_state.out_ports.get(ui_state.selected_out).unwrap_or(&"None".to_string()));
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
    dots_row.extend(render_labeled("   Channels Out: ",3));
    for i in 0..16 {
        let mut dot_style = Style::default();
        if midi_state.output_flash[i] > 0 { dot_style = dot_style.fg(Color::White).add_modifier(Modifier::BOLD); } 
        else if midi_state.channel_enabled[i] { dot_style = dot_style.fg(Color::Gray); } else { dot_style = dot_style.fg(Color::DarkGray); }
        if ui_state.focus == Focus::Channel(i) { dot_style = dot_style.bg(Color::DarkGray); }
        dots_row.push(Span::styled("• ", dot_style));
    }

    f.render_widget(Paragraph::new(vec![Line::raw(""), Line::from(top_row), Line::raw(""), Line::from(dots_row)]).block(Block::default().title(" Settings ").borders(Borders::ALL).border_style(Style::default().fg(Color::Green))).wrap(Wrap { trim: true }), area);
}

fn render_presets_panel(f: &mut Frame, ui_state: &UiState, area: Rect) {
    let presets_text = "  1   2   3   4   5   6   7   8   9   |  Shift+P, Num to Load  |  Shift+S, Num to Save";
    let p_color = if ui_state.pending_action.is_some() { Color::Yellow } else { Color::Green };
    f.render_widget(Paragraph::new(presets_text).block(Block::default().title(" Presets ").borders(Borders::ALL).border_style(Style::default().fg(p_color))), area);
}

fn render_equal_division_panel(f: &mut Frame, ui_state: &UiState, area: Rect) {
    let mut ed_row = vec![];
    ed_row.extend(render_labeled("Divisions: ", 0));
    let div_str = if ui_state.is_editing_divisions { format!("< {}_ >", ui_state.divisions_input) } else { format!("[ {} ]", ui_state.divisions_input) };
    let div_style = if ui_state.focus == Focus::Divisions { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
    ed_row.push(Span::styled(div_str, div_style)); ed_row.push(Span::raw("      "));

    ed_row.extend(render_labeled("Interval to Divide: ", 1));
    let int_str = if ui_state.is_editing_interval { format!("< {}_ >", ui_state.interval_input) } else { format!("[ {} ]", ui_state.interval_input) };
    let int_style = if ui_state.focus == Focus::Interval { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
    ed_row.push(Span::styled(int_str, int_style));
    
    f.render_widget(Paragraph::new(Line::from(ed_row)).block(Block::default().title(" Equal Division ").borders(Borders::ALL).border_style(Style::default().fg(Color::Green))), area);
}

fn render_grid_panel(f: &mut Frame, ui_state: &UiState, area: Rect) {
    let grid_block = Block::default().title(" Guitar Grid ").borders(Borders::ALL).border_style(Style::default().fg(Color::Green));
    let inner_grid_area = grid_block.inner(area);
    f.render_widget(grid_block, area);

    let grid_splits = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Length(12), Constraint::Min(1)].as_ref()).split(inner_grid_area);

    let mut string_lines = vec![];
    for i in 0..8 {
        let string_idx = i; 
        let string_num = string_idx + 1; 
        let mut line = Vec::new();

        if i == 0 { line.push(Span::styled("S", Style::default().add_modifier(Modifier::UNDERLINED))); } else { line.push(Span::raw("S")); }

        line.push(Span::raw(format!("{}: ", string_num)));
        line.push(fmt_box(ui_state, Focus::GridOpen(string_idx), &ui_state.grid_open[string_idx]));
        string_lines.push(Line::from(line));
    }
    f.render_widget(Paragraph::new(string_lines), grid_splits[0]);
    
    let mut g_row1 = vec![];
    
    let mode_str = if ui_state.grid_is_ji { "[JI ]" } else { "[EDO]" };
    let mode_style = if ui_state.focus == Focus::GridModeToggle { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
    g_row1.extend(render_labeled("Mode: ", 0));
    g_row1.push(Span::styled(mode_str, mode_style));

    if !ui_state.grid_is_ji {
        g_row1.extend(render_labeled("  EDO: ", 2)); 
        g_row1.push(fmt_box(ui_state, Focus::GridEdo, &ui_state.grid_edo));
    } else {
        g_row1.push(Span::raw("             ")); // Pad width to align Ref MIDI
    }

    g_row1.extend(render_labeled("  Ref MIDI: ", 2)); g_row1.push(fmt_box(ui_state, Focus::GridRefMidi, &ui_state.grid_ref_midi));
    g_row1.extend(render_labeled("  Ref Hz: ", 7)); g_row1.push(fmt_box(ui_state, Focus::GridRefPitch, &ui_state.grid_ref_pitch));
    
    let mut g_row2 = vec![];
    let horiz_label = if ui_state.grid_is_ji { "Horiz Int: " } else { "Horiz Step: " };
    g_row2.extend(render_labeled(horiz_label, 0));
    
    let mut h_step_span = fmt_box(ui_state, Focus::GridHoriz, &ui_state.grid_horiz);
    if ui_state.grid_unequal_toggle { h_step_span = Span::styled(format!("[ {} ]", ui_state.grid_horiz), Style::default().fg(Color::DarkGray)); }
    g_row2.push(h_step_span);
    g_row2.extend(render_labeled("  Capo: ", 3)); g_row2.push(fmt_box(ui_state, Focus::GridCapo, &ui_state.grid_capo));
    g_row2.extend(render_labeled("  Octave: ", 6)); g_row2.push(fmt_box(ui_state, Focus::GridOctave, &ui_state.grid_octave));

    let checkbox = if ui_state.grid_unequal_toggle { "[x] " } else { "[ ] " };
    let focus_style = if ui_state.focus == Focus::GridUnequalToggle { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };

    let mut g_row3 = vec![Span::styled(checkbox, focus_style)];
    g_row3.extend(render_labeled("Unequal Frets", 0));

    for span in &mut g_row3[1..] { span.style = span.style.patch(focus_style); }
    
    let step_label = if ui_state.grid_is_ji { "Intervals: " } else { "Steps: " };
    let mut g_row4 = vec![Span::raw(step_label)];
    if ui_state.grid_unequal_toggle {
        for i in 0..8 {
            g_row4.push(fmt_box(ui_state, Focus::GridUnequal(i), &ui_state.grid_unequal[i]));
            g_row4.push(Span::raw(" "));
        }
    }

    f.render_widget(Paragraph::new(vec![Line::from(g_row1), Line::from(g_row2), Line::raw(""), Line::from(g_row3), Line::from(g_row4)]), grid_splits[1]);
}

fn render_file_panel(f: &mut Frame, ui_state: &UiState, area: Rect) {
    let file_border_color = if ui_state.focus == Focus::Notepad { if ui_state.is_typing_in_notepad { Color::White } else { Color::Yellow }} else { Color::Green };
    let file_block = Block::default().title(" File ").borders(Borders::ALL).border_style(Style::default().fg(file_border_color));
    let file_area = file_block.inner(area);
    f.render_widget(file_block, area);

    let file_splits = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Length(1), Constraint::Min(5)].as_ref())
        .split(file_area);

    let scl_tab = Line::from(vec![Span::raw(" SCL ")]);
    let kbm_tab = Line::from(vec![Span::raw(" KBM ")]);

    let tabs = Tabs::new(vec![scl_tab, kbm_tab])
        .select(ui_state.active_file_tab)
        .highlight_style(Style::default().add_modifier(Modifier::UNDERLINED)) 
        .divider("|");
    f.render_widget(tabs, file_splits[0]);

    let btn_text = " Load .scl/.kbm (Shift+L)  Clear .scl (Shift+C)  Clear .kbm (Shift+K) ";
    f.render_widget(Paragraph::new(btn_text), file_splits[1]);

    if ui_state.active_file_tab == 0 { f.render_widget(&ui_state.scl_textarea, file_splits[2]); } 
    else { f.render_widget(&ui_state.kbm_textarea, file_splits[2]); }
}

fn render_logs_panel(f: &mut Frame, ui_state: &UiState, area: Rect) {
    let log_block = Block::default().title(" Logs ").borders(Borders::ALL).border_style(Style::default().fg(Color::Green));
    let log_area = log_block.inner(area);
    
    let visible_lines = log_area.height as usize;
    let start_idx = ui_state.logs.len().saturating_sub(visible_lines);
    
    let log_text = ui_state.logs[start_idx..].join("\n");
    f.render_widget(Paragraph::new(log_text).block(log_block), area);
}

fn render_command_panel(f: &mut Frame, ui_state: &UiState, area: Rect) {
    let input_style = if ui_state.focus == Focus::CommandInput { Style::default().fg(Color::Yellow) } else { Style::default() };
    f.render_widget(Paragraph::new(format!("> {}", ui_state.input)).style(input_style).block(Block::default().title(" Command Input ").borders(Borders::ALL).border_style(input_style)), area);
}

// --- MAIN LOOP ---

pub fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ui_state: &mut UiState,
    state_mutex: Arc<Mutex<MidiState>>
) -> Result<UiAction, Box<dyn std::error::Error>> {

    loop {
        let active_color = if ui_state.focus == Focus::Notepad { if ui_state.is_typing_in_notepad { Color::White } else { Color::Yellow }} else { Color::Green };
        ui_state.scl_textarea.set_block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(if ui_state.active_file_tab == 0 { active_color } else { Color::DarkGray })));
        ui_state.kbm_textarea.set_block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(if ui_state.active_file_tab == 1 { active_color } else { Color::DarkGray })));

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
                .split(f.area());

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

            let midi_state = state_mutex.lock().unwrap();

            render_settings_panel(f, ui_state, &midi_state, main_chunks[0]);
            render_presets_panel(f, ui_state, main_chunks[1]);
            render_equal_division_panel(f, ui_state, left_chunks[0]);
            render_grid_panel(f, ui_state, left_chunks[1]);
            render_file_panel(f, ui_state, middle_chunks[1]);
            render_logs_panel(f, ui_state, main_chunks[3]);
            render_command_panel(f, ui_state, main_chunks[4]);

            // Draw the Menu on top of everything if active
            if ui_state.is_menu_active {
                let term_area = f.area();
                
                let x_pos = match ui_state.focus {
                    Focus::Input => 16,
                    Focus::Output => 46, // "Input Device" + Value + padding
                    _ => 16,
                };
                
                let width = 40.min(term_area.width.saturating_sub(x_pos));
                let height = 12.min(term_area.height.saturating_sub(3));
                
                let popup_area = Rect::new(x_pos, 3, width, height);
                
                let menu = Menu::new();
                f.render_stateful_widget(menu, popup_area, &mut ui_state.menu_state);
            }
        })?;
        if event::poll(Duration::from_millis(30))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if let KeyCode::Char(c) = key.code {
                        if let Some(action) = ui_state.pending_action {
                            if c.is_ascii_digit() && c != '0' {
                                let preset_key = format!("preset{}", c);
                                
                                if action == 'P' {
                                    if let Some(ui_action) = ui_state.load_preset(state_mutex.clone(), &preset_key) {
                                        ui_state.pending_action = None;
                                        return Ok(ui_action);
                                    }
                                } else if action == 'S' {
                                    ui_state.save_preset(state_mutex.clone(), &preset_key);
                                }
                            } else { ui_state.logs.push("Cancelled.".to_string()); }
                            ui_state.pending_action = None;
                            continue;
                        }

                        if !ui_state.is_editing_grid && !ui_state.is_editing_divisions && !ui_state.is_editing_interval && !ui_state.is_menu_active && !ui_state.is_editing_pb && !ui_state.is_typing_in_notepad {
                            match c {
                                'P' => { ui_state.pending_action = Some('P'); ui_state.logs.push("Press 1-9 to LOAD, or any other key to cancel.".to_string()); continue; }
                                'S' => { ui_state.pending_action = Some('S'); ui_state.logs.push("Press 1-9 to SAVE, or any other key to cancel.".to_string()); continue; }
                                'L' => {
                                    disable_raw_mode()?; execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                                    let scl_path = prompt_input("Enter path to .scl file (or Drag & Drop): ");
                                    let kbm_path = prompt_input("Enter path to .kbm file (or Drag & Drop, press Enter for default): ");
                                    execute!(terminal.backend_mut(), EnterAlternateScreen)?; enable_raw_mode()?; terminal.clear()?;
                                    
                                    let p_scl = scl_path.trim_matches('"').trim_matches('\'').trim();
                                    if let Ok(content) = std::fs::read_to_string(p_scl) {
                                        let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                                        ui_state.scl_textarea = ratatui_textarea::TextArea::new(lines);
                                        ui_state.active_file_tab = 0; 
                                    } else if !p_scl.is_empty() {
                                        ui_state.logs.push(format!("Failed to read SCL file: {}", p_scl));
                                    }

                                    let p_kbm = kbm_path.trim_matches('"').trim_matches('\'').trim();
                                    if let Ok(content) = std::fs::read_to_string(p_kbm) {
                                        let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                                        ui_state.kbm_textarea = ratatui_textarea::TextArea::new(lines);
                                    } else if p_kbm.is_empty() {
                                        let lines: Vec<String> = vec!["! Default KBM".into() , "!Number of Mapped Notes".into(), "0".into(), "!Bottom Note".into(), "0".into(), "!Top Note".into(), "127 ".into(), "!MIDI note for mapping".into(), "69 ".into(), "!MIDI note for Tuning".into(), "69 ".into(), "!Tuning Frequency".into(), "440.0 ".into(), "!Formal Octave".into(), "12 ".into(), "!Mapping list:".into()];
                                        ui_state.kbm_textarea = ratatui_textarea::TextArea::new(lines);
                                    } else {
                                        ui_state.logs.push(format!("Failed to read KBM file: {}", p_kbm));
                                    }
                                    ui_state.active_tuning_mode = "file".to_string();
                                    match crate::tuning::sync_notepad_tuning(state_mutex.clone(), ui_state.scl_textarea.lines(), ui_state.kbm_textarea.lines()) {
                                        Ok(msg) => ui_state.logs.push(msg), Err(e) => ui_state.logs.push(format!("Notepad Parse Error: {}", e))
                                    }
                                    continue;
                                }
                                'C' => {
                                    let lines: Vec<String> = vec!["!".into(), "12 EDO".into(), "12".into(), "!".into(), "100.0".into(), "200.0".into(), "300.0".into(), "400.0".into(), "500.0".into(), "600.0".into(), "700.0".into(), "800.0".into(), "900.0".into(), "1000.0".into(), "1100.0".into(), "2/1".into()];
                                    ui_state.scl_textarea = ratatui_textarea::TextArea::new(lines);
                                    ui_state.active_file_tab = 0;
                                    ui_state.active_tuning_mode = "file".to_string();
                                    match crate::tuning::sync_notepad_tuning(state_mutex.clone(), ui_state.scl_textarea.lines(), ui_state.kbm_textarea.lines()) {
                                        Ok(msg) => ui_state.logs.push(format!("Cleared SCL. {}", msg)), Err(e) => ui_state.logs.push(format!("Parse Error: {}", e))
                                    }
                                    continue;
                                }
                                'K' => {
                                    let lines: Vec<String> = vec!["! Default KBM".into() , "!Number of Mapped Notes".into(), "0".into(), "!Bottom Note".into(), "0".into(), "!Top Note".into(), "127 ".into(), "!MIDI note for mapping".into(), "69 ".into(), "!MIDI note for Tuning".into(), "69 ".into(), "!Tuning Frequency".into(), "440.0 ".into(), "!Formal Octave".into(), "12 ".into(), "!Mapping list:".into()];
                                    ui_state.kbm_textarea = ratatui_textarea::TextArea::new(lines);
                                    ui_state.active_file_tab = 1;
                                    ui_state.active_tuning_mode = "file".to_string();
                                    match crate::tuning::sync_notepad_tuning(state_mutex.clone(), ui_state.scl_textarea.lines(), ui_state.kbm_textarea.lines()) {
                                        Ok(msg) => ui_state.logs.push(format!("Cleared KBM. {}", msg)), Err(e) => ui_state.logs.push(format!("Parse Error: {}", e))
                                    }
                                    continue;
                                }
                                _ => {}
                            }

                            let new_focus = match c {
                                'i' => Some(Focus::Input),
                                'o' => Some(Focus::Output),
                                't' => Some(Focus::Mode),
                                'p' => Some(Focus::PitchBend),
                                'c' => Some(Focus::Channel(0)),
                                'd' => Some(Focus::Divisions),
                                'n' => Some(Focus::Interval),
                                'm' => Some(Focus::GridModeToggle),
                                'e' => if ui_state.grid_is_ji { None } else { Some(Focus::GridEdo) },
                                'r' => Some(Focus::GridRefMidi),
                                'z' => Some(Focus::GridRefPitch),
                                'h' => Some(Focus::GridHoriz),
                                'a' => Some(Focus::GridCapo),
                                'v' => Some(Focus::GridOctave),
                                'u' => Some(Focus::GridUnequalToggle),
                                's' => Some(Focus::GridOpen(0)),
                                'f' => Some(Focus::Notepad), 
                                'q' if ui_state.focus != Focus::CommandInput => Some(Focus::CommandInput), 
                                _ => None,
                            };

                            if let Some(f) = new_focus { ui_state.focus = f; continue; }
                        }
                    } else if key.code == KeyCode::Esc && ui_state.pending_action.is_some() {
                        ui_state.logs.push("Cancelled.".to_string());
                        ui_state.pending_action = None;
                        continue;
                    }

                    if ui_state.is_menu_active {
                        match key.code {
                            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                                let idx = (c.to_digit(10).unwrap() - 1) as usize;
                                
                                let action = if ui_state.focus == Focus::Input && idx < ui_state.in_ports.len() {
                                    Some(MenuAction::SelectInput(idx))
                                } else if ui_state.focus == Focus::Output && idx < ui_state.out_ports.len() {
                                    Some(MenuAction::SelectOutput(idx))
                                } else {
                                    None
                                };

                                if let Some(act) = action {
                                    ui_state.is_menu_active = false;
                                    ui_state.menu_state.reset();
                                    match act {
                                        MenuAction::SelectInput(i) => return Ok(UiAction::ChangeInput(i)),
                                        MenuAction::SelectOutput(i) => return Ok(UiAction::ChangeOutput(i)),
                                    }
                                }
                            }
                            KeyCode::Up => ui_state.menu_state.up(),
                            KeyCode::Down => ui_state.menu_state.down(),
                            KeyCode::Left => ui_state.menu_state.left(),
                            KeyCode::Right => ui_state.menu_state.right(),
                            KeyCode::Enter => ui_state.menu_state.select(),
                            KeyCode::Esc => { 
                                ui_state.is_menu_active = false;
                                ui_state.menu_state.reset(); 
                            },
                            _ => {}
                        }

                        for MenuEvent::Selected(action) in ui_state.menu_state.drain_events() {
                            ui_state.is_menu_active = false;
                            ui_state.menu_state.reset();                                
                                match action {
                                    MenuAction::SelectInput(idx) => return Ok(UiAction::ChangeInput(idx)),
                                    MenuAction::SelectOutput(idx) => return Ok(UiAction::ChangeOutput(idx)),
                                }
                        }
                        continue;
                    } else if ui_state.is_editing_pb {
                        match key.code {
                            KeyCode::Char(c) if c.is_ascii_digit() => { 
                                if ui_state.clear_pb {
                                    ui_state.pb_input.clear();
                                    ui_state.clear_pb = false;
                                }
                                if ui_state.pb_input.len() < 3 { ui_state.pb_input.push(c); } 
                            }
                            KeyCode::Backspace => { 
                                ui_state.clear_pb = false;
                                ui_state.pb_input.pop(); 
                            }
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
                                ui_state.active_tuning_mode = "equal_division".to_string();
                                match apply_equal_division(state_mutex.clone(), &ui_state.divisions_input, &ui_state.interval_input) {
                                    Ok(msg) => ui_state.logs.push(msg), Err(e) => ui_state.logs.push(format!("ED Error: {}", e)),
                                }
                            }
                            KeyCode::Esc => { ui_state.is_editing_divisions = false; ui_state.is_editing_interval = false; }
                            _ => {}
                        }
                    } else if ui_state.is_typing_in_notepad {
                        match key.code {
                            KeyCode::Esc => { ui_state.is_typing_in_notepad = false; ui_state.logs.push("Exited Notepad.".to_string()); }
                            KeyCode::Tab => { ui_state.active_file_tab = if ui_state.active_file_tab == 0 { 1 } else { 0 }; }
                            KeyCode::Enter => {
                                if ui_state.active_file_tab == 0 { ui_state.scl_textarea.input(key); } else { ui_state.kbm_textarea.input(key); }
                                ui_state.active_tuning_mode = "file".to_string();
                                match crate::tuning::sync_notepad_tuning(state_mutex.clone(), ui_state.scl_textarea.lines(), ui_state.kbm_textarea.lines()) {
                                    Ok(msg) => ui_state.logs.push(msg), Err(e) => ui_state.logs.push(format!("Notepad Parse Error: {}", e)),
                                }
                            }
                            _ => { if ui_state.active_file_tab == 0 { ui_state.scl_textarea.input(key); } else { ui_state.kbm_textarea.input(key); } }

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
                                ui_state.active_tuning_mode = "guitar_grid".to_string();
                                let horiz = if ui_state.grid_unequal_toggle { ui_state.grid_unequal.to_vec() } else { vec![ui_state.grid_horiz.clone()] };
                                
                                match apply_grid_tuning(state_mutex.clone(), ui_state.grid_is_ji, ui_state.grid_unequal_toggle, &ui_state.grid_edo, &ui_state.grid_ref_midi, &ui_state.grid_ref_pitch, &ui_state.grid_open, &horiz, &ui_state.grid_capo, &ui_state.grid_octave) {
                                    Ok(msg) => ui_state.logs.push(msg), Err(e) => ui_state.logs.push(format!("Grid Error: {}", e)),
                                }

                                // --- Auto-advance flow for spreadsheet entry ---
                                match ui_state.focus {
                                    Focus::GridOpen(i) if i < 7 => {
                                        ui_state.focus = Focus::GridOpen(i + 1);
                                        ui_state.clear_grid = true; 
                                    }
                                    Focus::GridUnequal(i) if i < 7 => {
                                        ui_state.focus = Focus::GridUnequal(i + 1);
                                        ui_state.clear_grid = true;
                                    }
                                    _ => {
                                        // Exit edit mode for the last items or any other grid field
                                        ui_state.is_editing_grid = false; 
                                    }
                                }
                            }
                            KeyCode::Esc => { ui_state.is_editing_grid = false; }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Down => ui_state.focus = move_focus(ui_state.focus, "Down", ui_state),
                            KeyCode::Up   => ui_state.focus = move_focus(ui_state.focus, "Up", ui_state),
                            KeyCode::Left => ui_state.focus = move_focus(ui_state.focus, "Left", ui_state),
                            KeyCode::Right=> ui_state.focus = move_focus(ui_state.focus, "Right", ui_state),
                            KeyCode::Enter => {
                                match ui_state.focus {
                                    Focus::Input => { 
                                        ui_state.is_menu_active = true;
                                        
                                        let items: Vec<MenuItem<MenuAction>> = ui_state.in_ports.iter().enumerate()
                                            .map(|(i, name)| MenuItem::item(format!("{}. {}", i + 1, name), MenuAction::SelectInput(i)))
                                            .collect();
                                            
                                        ui_state.menu_state = MenuState::new(vec![MenuItem::group("Select Input", items)]);
                                        ui_state.menu_state.down(); // Auto-open the dropdown
                                        
                                        for _ in 0..ui_state.selected_in+1 {
                                            ui_state.menu_state.down();
                                        }
                                    }
                                    Focus::Output => { 
                                        ui_state.is_menu_active = true;
                                        
                                        let items: Vec<MenuItem<MenuAction>> = ui_state.out_ports.iter().enumerate()
                                            .map(|(i, name)| MenuItem::item(format!("{}. {}", i + 1, name), MenuAction::SelectOutput(i)))
                                            .collect();
                                            
                                        ui_state.menu_state = MenuState::new(vec![MenuItem::group("Select Output", items)]);
                                        ui_state.menu_state.down(); // Auto-open the dropdown
                                        
                                        // Auto-highlight currently selected item
                                        for _ in 0..ui_state.selected_out+1 {
                                            ui_state.menu_state.down();
                                        }
                                    }
                                    Focus::PitchBend => { 
                                        ui_state.pb_input = state_mutex.lock().unwrap().pitch_bend_range.to_string(); 
                                        ui_state.is_editing_pb = true; 
                                        ui_state.clear_pb = true; 
                                    }
                                    Focus::Divisions => { ui_state.is_editing_divisions = true; ui_state.clear_divisions = true; }
                                    Focus::Interval => { ui_state.is_editing_interval = true; ui_state.clear_interval = true; }
                                    Focus::GridModeToggle => {
                                        ui_state.grid_is_ji = !ui_state.grid_is_ji;
                                        ui_state.active_tuning_mode = "guitar_grid".to_string();
                                        let horiz = if ui_state.grid_unequal_toggle { ui_state.grid_unequal.to_vec() } else { vec![ui_state.grid_horiz.clone()] };
                                        match apply_grid_tuning(state_mutex.clone(), ui_state.grid_is_ji, ui_state.grid_unequal_toggle, &ui_state.grid_edo, &ui_state.grid_ref_midi, &ui_state.grid_ref_pitch, &ui_state.grid_open, &horiz, &ui_state.grid_capo, &ui_state.grid_octave) {
                                            Ok(msg) => ui_state.logs.push(msg), Err(e) => ui_state.logs.push(format!("Grid Error: {}", e)),
                                        }
                                    }
                                    Focus::GridEdo | Focus::GridRefMidi | Focus::GridRefPitch | Focus::GridHoriz | Focus::GridCapo | Focus::GridOctave | Focus::GridOpen(_) | Focus::GridUnequal(_) => {
                                        ui_state.is_editing_grid = true; ui_state.clear_grid = true;
                                    }
                                    Focus::GridUnequalToggle => {
                                        ui_state.grid_unequal_toggle = !ui_state.grid_unequal_toggle;
                                        ui_state.active_tuning_mode = "guitar_grid".to_string();                                        
                                        let horiz = if ui_state.grid_unequal_toggle { ui_state.grid_unequal.to_vec() } else { vec![ui_state.grid_horiz.clone()] };
                                        match apply_grid_tuning(state_mutex.clone(), ui_state.grid_is_ji, ui_state.grid_unequal_toggle, &ui_state.grid_edo, &ui_state.grid_ref_midi, &ui_state.grid_ref_pitch, &ui_state.grid_open, &horiz, &ui_state.grid_capo, &ui_state.grid_octave) {
                                            Ok(msg) => ui_state.logs.push(msg), Err(e) => ui_state.logs.push(format!("Grid Error: {}", e)),
                                        }
                                    }
                                    Focus::Mode => {
                                        let mut s = state_mutex.lock().unwrap(); s.is_mpe = !s.is_mpe;
                                        if s.is_mpe { s.pitch_bend_range = 48; if let Some(conn) = &mut s.out_conn { send_mpe_configuration(conn, 15); } ui_state.logs.push("Switched to MPE. Pitch bend range locked to 48.".to_string()); } 
                                        else { s.pitch_bend_range = 12; ui_state.logs.push("Switched to Multi-timbral. Pitch bend range reset to 12.".to_string()); }
                                    }
                                    Focus::Channel(i) => { let mut s = state_mutex.lock().unwrap(); if s.is_mpe && i == 0 { ui_state.logs.push("Channel 1 is MPE Master (cannot disable).".to_string()); } else { s.channel_enabled[i] = !s.channel_enabled[i]; } }
                                    Focus::Notepad => { ui_state.is_typing_in_notepad = true; ui_state.logs.push("Entered Notepad. Press Esc to exit. Press Tab to switch files.".to_string()); }
                                    Focus::CommandInput => {
                                        let cmd = ui_state.input.trim().to_string(); ui_state.input.clear();
                                        if cmd == "q" { return Ok(UiAction::Quit); }
                                        if update_tuning(state_mutex.clone(), &cmd) { ui_state.logs.push(format!("Preset {} loaded.", cmd)); } else { ui_state.logs.push(format!("Unknown command: {}", cmd)); }
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