mod midi;
mod tuning;
mod ui;

use midir::{MidiInput, MidiOutput};
use std::error::Error;
use std::io;
use std::sync::{Arc, Mutex};
use crossterm::{execute, terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen}};
use ratatui::{backend::CrosstermBackend, Terminal};
use ratatui::widgets::{Block, Borders}; // Added for default text area block formatting

use midi::{MidiState, process_midi, send_mpe_configuration};
use ui::{UiState, UiAction, run_tui};
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

    // --- PRE-CONFIGURE THE NOTEPAD TEXTAREAS ---
    let mut scl_textarea = ratatui_textarea::TextArea::default();
    scl_textarea.set_block(Block::default().borders(Borders::TOP));
    scl_textarea.insert_str("  12 EDO\n12\n!\n100.0\n200.0\n300.0\n"); 

    let mut kbm_textarea = ratatui_textarea::TextArea::default();
    kbm_textarea.set_block(Block::default().borders(Borders::TOP));
    kbm_textarea.insert_str("! Template for a keyboard mapping\nSize of map:\n12\n...\n");

let mut ui_state = UiState::default();

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
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}