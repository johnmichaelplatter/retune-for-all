use midir::{MidiInput, MidiOutput, MidiOutputConnection};
use std::error::Error;
use std::io::{stdin, stdout, Write};

struct MidiState {
    out_conn: MidiOutputConnection,
    num_channels: u8,
    note_to_channel: [Option<(u8, u8)>; 128],
    channel_busy: Vec<bool>,
    last_allocated: u8,
    
    tuning: [f32; 128],
    pitch_bend_range: u8,
    pitch_center: f32,
    pitch_reference_note: u8,
}

fn main() -> Result<(), Box<dyn Error>> {
    let midi_in = MidiInput::new("Poly Router Input")?;
    let midi_out = MidiOutput::new("Poly Router Output")?;

    let in_port = select_port(&midi_in, "input")?;
    let out_port = select_port(&midi_out, "output")?;
    let num_channels = get_num_channels()?;
    let pitch_bend_range = get_pitch_bend_range()?;

    println!("\nConnecting...");
    let out_conn = midi_out.connect(&out_port, "poly-router-out")?;

    // 1. Initialize Standard Equal Temperament (A4 = 440Hz, Note 69)
    let pitch_center = 440.0;
    let pitch_reference_note = 69;
    let mut tuning = [0.0; 128];
    for i in 0..128 {
        tuning[i] = pitch_center * 2.0_f32.powf((i as f32 - pitch_reference_note as f32) / 12.0);
    }

    // 2. Apply Custom Offsets
    // E (4, 16, 28, ...), Bb (10, 22, 34, ...), G (7, 19, 31, ...), F# (6, 18, 30, ...)
    for i in 0..128 {
        let note_class = i % 12;
        let offset_cents = match note_class {
            4 => -14.0, // E
            10 => -34.0, // Bb
            7 => 2.0,   // G
            6 => -49.0, // F#
            _ => 0.0,
        };
        
        if offset_cents != 0.0 {
            tuning[i] *= 2.0_f32.powf(offset_cents / 1200.0);
        }
    }

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

    let _in_conn = midi_in.connect(
        &in_port,
        "poly-router-in",
        move |_stamp, message, state| process_midi(message, state),
        state,
    )?;

    println!("Processing MIDI. Press Enter to stop.");
    let mut input = String::new();
    stdin().read_line(&mut input)?;
    Ok(())
}

fn process_midi(message: &[u8], state: &mut MidiState) {
    if message.is_empty() { return; }
    let status = message[0];
    let msg_type = status & 0xF0;
    let mut out_msg = message.to_vec();

    if message.len() >= 3 && (msg_type == 0x90 || msg_type == 0x80) {
        let input_note = message[1] as usize;
        let velocity = message[2];
        let is_note_on = msg_type == 0x90 && velocity > 0;

        if is_note_on {
            let mut assigned_chan = None;
            for i in 1..=state.num_channels {
                let chan = (state.last_allocated + i) % state.num_channels;
                if !state.channel_busy[chan as usize] {
                    assigned_chan = Some(chan);
                    break;
                }
            }

            if let Some(chan) = assigned_chan {
                let target_hz = state.tuning[input_note];
                let exact_note = state.pitch_reference_note as f32 
                               + 12.0 * (target_hz / state.pitch_center).log2();
                
                let nearest_note = exact_note.round().clamp(0.0, 127.0) as u8;
                let semitone_diff = exact_note - nearest_note as f32;
                
                let pb_val_f = 8192.0 + (semitone_diff / state.pitch_bend_range as f32) * 8192.0;
                let pb_val = pb_val_f.round().clamp(0.0, 16383.0) as u16;
                
                state.channel_busy[chan as usize] = true;
                state.note_to_channel[input_note] = Some((chan, nearest_note));
                state.last_allocated = chan;
                
                let _ = state.out_conn.send(&[0xE0 | chan, (pb_val & 0x7F) as u8, (pb_val >> 7) as u8]);
                out_msg[0] = msg_type | chan;
                out_msg[1] = nearest_note;
                let _ = state.out_conn.send(&out_msg);
            }
        } else {
            if let Some((chan, actual_sent_note)) = state.note_to_channel[input_note] {
                state.channel_busy[chan as usize] = false;
                state.note_to_channel[input_note] = None;
                out_msg[0] = msg_type | chan;
                out_msg[1] = actual_sent_note;
                let _ = state.out_conn.send(&out_msg);
            }
        }
    } else {
        let _ = state.out_conn.send(&out_msg);
    }
}

fn select_port<T: midir::MidiIO>(midi_io: &T, port_type: &str) -> Result<T::Port, Box<dyn Error>> {
    let ports = midi_io.ports();
    if ports.is_empty() { return Err("No ports available".into()); }
    for (i, p) in ports.iter().enumerate() { println!("{}: {}", i, midi_io.port_name(p)?); }
    print!("Select {} port: ", port_type); stdout().flush()?;
    let mut s = String::new(); stdin().read_line(&mut s)?;
    Ok(ports.into_iter().nth(s.trim().parse()?).ok_or("Invalid")?)
}

fn get_num_channels() -> Result<u8, Box<dyn Error>> {
    print!("Number of channels: "); stdout().flush()?;
    let mut s = String::new(); stdin().read_line(&mut s)?;
    Ok(s.trim().parse()?)
}

fn get_pitch_bend_range() -> Result<u8, Box<dyn Error>> {
    print!("Pitch bend range (1-48): "); stdout().flush()?;
    let mut s = String::new(); stdin().read_line(&mut s)?;
    Ok(s.trim().parse()?)
}