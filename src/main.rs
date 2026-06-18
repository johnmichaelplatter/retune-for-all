mod midi;
mod tuning;
mod ui;

use midir::{MidiInput, MidiOutput};
use std::error::Error;
use std::io::{stdin, stdout, Write};
use std::sync::{Arc, Mutex};

use midi::{MidiState, process_midi, send_mpe_configuration};
use tuning::update_tuning;

fn prompt_mpe_mode() -> Result<bool, Box<dyn Error>> {
    print!("Select MIDI Output Mode (1: Standard Multi-timbral, 2: MPE): ");
    stdout().flush()?;
    let mut s = String::new();
    stdin().read_line(&mut s)?;
    match s.trim() { "2" => Ok(true), _ => Ok(false) }
}

fn select_port<T: midir::MidiIO>(io: &T, pt: &str) -> Result<T::Port, Box<dyn Error>> {
    let ports = io.ports();
    for (i, p) in ports.iter().enumerate() { println!("{}: {}", i, io.port_name(p)?); }
    print!("Select {} port: ", pt); stdout().flush()?;
    let mut s = String::new(); stdin().read_line(&mut s)?;
    Ok(ports.into_iter().nth(s.trim().parse()?).ok_or("Invalid")?)
}

fn get_num_channels() -> Result<u8, Box<dyn Error>> { 
    print!("Channels (15 recommended for MPE): "); stdout().flush()?; 
    let mut s=String::new(); stdin().read_line(&mut s)?; Ok(s.trim().parse()?) 
}

fn get_pitch_bend_range() -> Result<u8, Box<dyn Error>> { 
    print!("Synthesizer PB Range (1-48): "); stdout().flush()?; 
    let mut s=String::new(); stdin().read_line(&mut s)?; Ok(s.trim().parse()?) 
}

fn main() -> Result<(), Box<dyn Error>> {
    let midi_in = MidiInput::new("Poly Router Input")?;
    let midi_out = MidiOutput::new("Poly Router Output")?;

    let in_port = select_port(&midi_in, "input")?;
    let out_port = select_port(&midi_out, "output")?;
    
    let is_mpe = prompt_mpe_mode()?;
    let num_channels = get_num_channels()?;
    
    let pitch_bend_range = if is_mpe {
        println!("MPE Mode: Pitch Bend Range automatically locked to 48 semitones.");
        48
    } else { get_pitch_bend_range()? };

    println!("Connecting...");
    let mut out_conn = midi_out.connect(&out_port, "poly-router-out")?;

    if is_mpe {
        send_mpe_configuration(&mut out_conn, num_channels);
        println!("MPE Configuration Message sent to Synth (Channel 1).");
    }

    let state = Arc::new(Mutex::new(MidiState {
        out_conn, num_channels, note_to_channel: [None; 128], channel_busy: vec![false; 16], 
        last_allocated: 0, tuning: [0.0; 128], pitch_bend_range, synth_pitch_center: 440.0, 
        synth_ref_note: 69, input_pitch_bend: 8192, is_mpe, 
    }));

    update_tuning(state.clone(), "1");

    let state_for_callback = state.clone();
    
    // The background MIDI thread is spawned here and will live for as long as `_in_conn` does.
    let _in_conn = midi_in.connect(
        &in_port, "poly-router-in",
        move |_stamp, message, _| {
            let mut state = state_for_callback.lock().unwrap();
            process_midi(message, &mut state);
        }, ()
    )?;

    // Run the TUI placeholder on the main thread
    ui::run_tui(state)?;

    Ok(())
}