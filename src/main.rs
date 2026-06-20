mod midi;
mod tuning;
mod ui;

use midir::{MidiInput, MidiOutput};
use std::error::Error;
use std::io;
use std::sync::{Arc, Mutex};
use crossterm::{execute, terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen}};
use ratatui::{backend::CrosstermBackend, Terminal};

use midi::{MidiState, process_midi, send_mpe_configuration};
use ui::{UiState, Focus, UiAction, run_tui};
use tuning::update_tuning;

fn main() -> Result<(), Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let state = Arc::new(Mutex::new(MidiState {
        out_conn: None,
        note_to_channel: [None; 128], 
        channel_busy: vec![false; 16], 
        channel_enabled: [true; 16], 
        last_allocated: 0, 
        tuning: [0.0; 128], 
        pitch_bend_range: 12, 
        synth_pitch_center: 440.0, 
        synth_ref_note: 69, 
        input_pitch_bend: 8192, 
        is_mpe: false, 
        input_flash: 0,
        output_flash: [0; 16],
    }));
    update_tuning(state.clone(), "1");

    let mut ui_state = UiState {
        focus: Focus::CommandInput,
        is_editing_dropdown: false,
        is_editing_pb: false,
        pb_input: String::new(),
        
        is_editing_divisions: false,
        clear_divisions: false,
        divisions_input: "12".to_string(),
        
        is_editing_interval: false,
        clear_interval: false,
        interval_input: "2/1".to_string(),

        // --- NEW GRID INITIALIZERS ---
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
        // ------------------------------

        dropdown_index: 0,
        in_ports: vec![],
        out_ports: vec![],
        selected_in: 0,
        selected_out: 0,
        input: String::new(),
        logs: vec!["Welcome to Poly-Router!".into(), "Navigate to Settings with Arrow Keys to Configure.".into()],
    };

    let mut active_in_conn = None;

    loop {
        let midi_in = MidiInput::new("Poly Router Input")?;
        let midi_out = MidiOutput::new("Poly Router Output")?;
        
        ui_state.in_ports = midi_in.ports().iter().map(|p| midi_in.port_name(p).unwrap_or_default()).collect();
        ui_state.out_ports = midi_out.ports().iter().map(|p| midi_out.port_name(p).unwrap_or_default()).collect();

        if active_in_conn.is_none() && !ui_state.in_ports.is_empty() && ui_state.selected_in < ui_state.in_ports.len() {
            let port = &midi_in.ports()[ui_state.selected_in];
            let state_for_callback = state.clone();
            active_in_conn = Some(midi_in.connect(port, "router-in", move |_, message, _| {
                process_midi(message, &mut state_for_callback.lock().unwrap());
            }, ())?);
        }

        if state.lock().unwrap().out_conn.is_none() && !ui_state.out_ports.is_empty() && ui_state.selected_out < ui_state.out_ports.len() {
            let port = &midi_out.ports()[ui_state.selected_out];
            let mut out_conn = midi_out.connect(port, "router-out")?;
            
            if state.lock().unwrap().is_mpe { send_mpe_configuration(&mut out_conn, 15); }
            state.lock().unwrap().out_conn = Some(out_conn);
        }

        let action = run_tui(&mut terminal, &mut ui_state, state.clone())?;

        match action {
            UiAction::Quit => break,
            UiAction::ChangeInput(idx) => { ui_state.selected_in = idx; active_in_conn = None; }
            UiAction::ChangeOutput(idx) => { ui_state.selected_out = idx; state.lock().unwrap().out_conn = None; }
            UiAction::None => {}
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}