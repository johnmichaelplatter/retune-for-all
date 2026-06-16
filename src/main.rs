use midir::{MidiInput, MidiInputPort, MidiOutput, MidiOutputConnection, MidiOutputPort};
use std::error::Error;
use std::io::{stdin, stdout, Write};

// State struct to hold our output connection and tracking variables
struct MidiState {
    out_conn: MidiOutputConnection,
    num_channels: u8,
    // Maps a note (0-127) to (active output channel, sent output note). None if not playing.
    // Tracking the exact sent note is critical because retuning might shift it to a different MIDI note.
    note_to_channel: [Option<(u8, u8)>; 128], 
    // Tracks whether a specific output channel (voice) is currently holding a note.
    channel_busy: Vec<bool>,            
    // Remembers the last used channel to maintain a round-robin feel among free voices.
    last_allocated: u8,                 
    
    // Tuning state variables
    tuning: [f32; 128],       // Hz value for each MIDI note
    pitch_bend_range: u8,     // Maximum semitone range of the synth (1-48)
    pitch_center: f32,        // Base reference frequency (e.g., 440.0)
    pitch_reference_note: u8, // Base reference MIDI note (e.g., 69)
}

fn main() -> Result<(), Box<dyn Error>> {
    let midi_in = MidiInput::new("Poly Router Input")?;
    let midi_out = MidiOutput::new("Poly Router Output")?;

    // 1. Select Input Device
    let in_port = select_port(&midi_in, "input")?;
    let in_port_name = midi_in.port_name(&in_port)?;

    // 2. Select Output Device
    let out_port = select_port(&midi_out, "output")?;
    let out_port_name = midi_out.port_name(&out_port)?;

    // 3. Select Number of Channels
    let num_channels = get_num_channels()?;

    // 4. Select Pitch Bend Range
    let pitch_bend_range = get_pitch_bend_range()?;

    println!("\nConnecting...");
    println!("Input:  {}", in_port_name);
    println!("Output: {}", out_port_name);
    println!("Routing multi-timbral round-robin across {} channels.", num_channels);

    // Open output connection
    let out_conn = midi_out.connect(&out_port, "poly-router-out")?;

    // Initialize default equal temperament tuning based on A4 = 440Hz
    let pitch_center = 440.0;
    let pitch_reference_note = 69;
    let mut tuning = [0.0; 128];
    for i in 0..128 {
        tuning[i] = pitch_center * 2.0_f32.powf((i as f32 - pitch_reference_note as f32) / 12.0);
    }

    // Initialize our processing state
    let state = MidiState {
        out_conn,
        num_channels,
        note_to_channel: [None; 128],
        channel_busy: vec![false; num_channels as usize],
        last_allocated: 0,
        tuning,
        pitch_bend_range,
        pitch_center,
        pitch_reference_note,
    };

    // Open input connection and pass the state into the callback
    let _in_conn = midi_in.connect(
        &in_port,
        "poly-router-in",
        move |_stamp, message, state| {
            process_midi(message, state);
        },
        state,
    )?;

    println!("\nProcessing MIDI. Press Enter to stop and exit.");
    let mut input = String::new();
    stdin().read_line(&mut input)?;

    Ok(())
}

/// The core routing, voice allocation, and multi-timbral tuning logic
fn process_midi(message: &[u8], state: &mut MidiState) {
    if message.is_empty() { return; }

    let status = message[0];
    let msg_type = status & 0xF0; // Mask out the channel to get the message type
    
    let mut out_msg = message.to_vec();

    // 0x90 is Note On, 0x80 is Note Off. 
    if message.len() >= 3 && (msg_type == 0x90 || msg_type == 0x80) {
        let input_note = message[1] as usize;
        let velocity = message[2];
        let is_note_on = msg_type == 0x90 && velocity > 0;

        if is_note_on {
            // 1. Search for a free channel
            let mut assigned_chan = None;
            for i in 1..=state.num_channels {
                let chan = (state.last_allocated + i) % state.num_channels;
                if !state.channel_busy[chan as usize] {
                    assigned_chan = Some(chan);
                    break;
                }
            }

            // 2. If we found a free channel, calculate exact tuning and send
            if let Some(chan) = assigned_chan {
                // Calculate precise float MIDI note required to hit target Hz
                let target_hz = state.tuning[input_note];
                let exact_note = state.pitch_reference_note as f32 
                               + 12.0 * (target_hz / state.pitch_center).log2();
                
                // Find the nearest actual MIDI note to send
                let nearest_note = exact_note.round().clamp(0.0, 127.0) as u8;
                
                // Calculate how much we need to bend the pitch from the nearest note
                let semitone_diff = exact_note - nearest_note as f32;
                
                // Calculate 14-bit pitch bend value. 8192 is no bend. 
                // Range maps from -pitch_bend_range to +pitch_bend_range
                let pb_val_f = 8192.0 + (semitone_diff / state.pitch_bend_range as f32) * 8192.0;
                let pb_val = pb_val_f.round().clamp(0.0, 16383.0) as u16;
                
                let pb_lsb = (pb_val & 0x7F) as u8;
                let pb_msb = ((pb_val >> 7) & 0x7F) as u8;

                // Update Voice Allocator State
                state.channel_busy[chan as usize] = true;
                state.note_to_channel[input_note] = Some((chan, nearest_note));
                state.last_allocated = chan;
                
                // SEND 1: Pitch Bend Message on the allocated channel
                let pb_msg = [0xE0 | chan, pb_lsb, pb_msb];
                let _ = state.out_conn.send(&pb_msg);

                // SEND 2: Note On Message on the allocated channel with the adjusted note
                out_msg[0] = msg_type | chan;
                out_msg[1] = nearest_note;
                let _ = state.out_conn.send(&out_msg);
            } else {
                println!("Polyphony maxed out: Note {} dropped.", input_note);
            }
        } else {
            // Note Off: Find the channel playing this note and free it
            if let Some((chan, actual_sent_note)) = state.note_to_channel[input_note] {
                state.channel_busy[chan as usize] = false;
                state.note_to_channel[input_note] = None;
                
                // Use the exact note value that was calculated during Note On
                out_msg[0] = msg_type | chan;
                out_msg[1] = actual_sent_note;
                let _ = state.out_conn.send(&out_msg);
            }
        }
    } else {
        // Non-note messages pass through unchanged.
        let _ = state.out_conn.send(&out_msg);
    }
}

// --- Helper Functions for CLI Prompts ---

fn select_port<T>(midi_io: &T, port_type: &str) -> Result<T::Port, Box<dyn Error>>
where
    T: midir::MidiIO,
{
    let ports = midi_io.ports();
    if ports.is_empty() {
        return Err(format!("No MIDI {} ports available.", port_type).into());
    }

    println!("\nAvailable MIDI {} ports:", port_type);
    for (i, port) in ports.iter().enumerate() {
        println!("{}: {}", i, midi_io.port_name(port)?);
    }

    print!("Please select an {} port (0-{}): ", port_type, ports.len() - 1);
    stdout().flush()?;

    let mut input = String::new();
    stdin().read_line(&mut input)?;
    let parsed: usize = input.trim().parse()?;
    
    if parsed >= ports.len() {
        return Err("Invalid port selection.".into());
    }

    Ok(ports.into_iter().nth(parsed).unwrap())
}

fn get_num_channels() -> Result<u8, Box<dyn Error>> {
    print!("\nEnter number of output channels for round-robin (e.g., 8): ");
    stdout().flush()?;
    
    let mut input = String::new();
    stdin().read_line(&mut input)?;
    let num = input.trim().parse::<u8>()?;
    
    if num == 0 || num > 16 {
        return Err("Channels must be between 1 and 16.".into());
    }
    
    Ok(num)
}

fn get_pitch_bend_range() -> Result<u8, Box<dyn Error>> {
    print!("\nEnter your target synthesizer's pitch bend range in semitones (1-48, typically 2, 24, or 48): ");
    stdout().flush()?;
    
    let mut input = String::new();
    stdin().read_line(&mut input)?;
    let num = input.trim().parse::<u8>()?;
    
    if num < 1 || num > 48 {
        return Err("Pitch bend range must be between 1 and 48.".into());
    }
    
    Ok(num)
}