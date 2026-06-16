use midir::{MidiInput, MidiOutput, MidiOutputConnection};
use std::error::Error;
use std::io::{stdin, stdout, Write};
use std::sync::{Arc, Mutex};

struct MidiState {
    out_conn: MidiOutputConnection,
    num_channels: u8,
    // Maps note to: (Active Channel, Nearest Sent Note, Base Retuning Semitone Diff)
    note_to_channel: [Option<(u8, u8, f32)>; 128], 
    channel_busy: Vec<bool>,
    last_allocated: u8,
    
    tuning: [f32; 128],
    pitch_bend_range: u8,
    pitch_center: f32,
    pitch_reference_note: u8,
    
    // NEW: Track the current position of the input pitch bend wheel (center is 8192)
    input_pitch_bend: u16, 
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

    let state = Arc::new(Mutex::new(MidiState {
        out_conn,
        num_channels,
        note_to_channel: [None; 128],
        channel_busy: vec![false; num_channels as usize],
        last_allocated: 0,
        tuning: [0.0; 128],
        pitch_bend_range,
        pitch_center: 440.0,
        pitch_reference_note: 69,
        input_pitch_bend: 8192, // Default to no pitch bend
    }));

    // Initialize to Preset 1 (Standard Pitch)
    update_tuning(state.clone(), '1');

    let state_for_callback = state.clone();
    let _in_conn = midi_in.connect(
        &in_port,
        "poly-router-in",
        move |_stamp, message, _| {
            let mut state = state_for_callback.lock().unwrap();
            process_midi(message, &mut state);
        },
        (),
    )?;

    println!("\nProcessing. Press 1-9 to switch presets, or 'q' to quit.");
    loop {
        let mut input = String::new();
        stdin().read_line(&mut input)?;
        let choice = input.trim().chars().next().unwrap_or(' ');
        if choice == 'q' { break; }
        update_tuning(state.clone(), choice);
    }
    Ok(())
}

fn update_tuning(state_mutex: Arc<Mutex<MidiState>>, choice: char) {
    let mut state = state_mutex.lock().unwrap();
    let pitch_ref = state.pitch_reference_note as f32;
    let pitch_center = state.pitch_center;

    match choice {
        '1' => { // 12-TET
            for i in 0..128 { state.tuning[i] = pitch_center * 2.0f32.powf((i as f32 - pitch_ref) / 12.0); }
        }
        '2' => { // 24-EDO (Quarter-tone)
            for i in 0..128 { state.tuning[i] = pitch_center * 2.0f32.powf((i as f32 - pitch_ref) / 24.0); }
        }
        '3' => { // Just Intonation
            let ratios = [1.0, 17.0/16.0, 9.0/8.0, 6.0/5.0, 5.0/4.0, 4.0/3.0, 11.0/8.0, 3.0/2.0, 13.0/8.0, 5.0/3.0, 7.0/4.0, 15.0/8.0];
            let base_c_freq = pitch_center * (3.0 / 5.0); // Calibrate so C is 264Hz, keeping A at 440Hz
            for i in 0..128 {
                let note_class = (i % 12) as usize;
                let octave = (i / 12) as i32 - 5; 
                state.tuning[i] = base_c_freq * ratios[note_class] * 2.0f32.powi(octave);
            }
        }
        '4'..='9' => { // N-EDO
            let n = match choice { '4'=>17, '5'=>19, '6'=>22, '7'=>31, '8'=>41, '9'=>53, _=>12 };
            for i in 0..128 { state.tuning[i] = pitch_center * 2.0f32.powf((i as f32 - pitch_ref) / n as f32); }
        }
        _ => println!("Invalid preset."),
    }
    println!("Preset {} loaded.", choice);
}

fn process_midi(message: &[u8], state: &mut MidiState) {
    if message.is_empty() { return; }
    let status = message[0];
    let msg_type = status & 0xF0;
    
    // We define how many semitones a fully pushed pitch wheel should span based on your math.
    let wheel_range_semitones = 1.0; 

    // --- 1. NOTE MESSAGES ---
    if message.len() >= 3 && (msg_type == 0x90 || msg_type == 0x80) {
        let input_note = message[1] as usize;
        let is_note_on = msg_type == 0x90 && message[2] > 0;

        if is_note_on {
            let mut assigned_chan = None;
            for i in 1..=state.num_channels {
                let chan = (state.last_allocated + i) % state.num_channels;
                if !state.channel_busy[chan as usize] { assigned_chan = Some(chan); break; }
            }

            if let Some(chan) = assigned_chan {
                let target_hz = state.tuning[input_note];
                let exact_note = state.pitch_reference_note as f32 + 12.0 * (target_hz / state.pitch_center).log2();
                let nearest_note = exact_note.round().clamp(0.0, 127.0) as u8;
                
                // Retuning error: How far the target pitch is from the nearest standard MIDI note
                let semitone_diff = exact_note - nearest_note as f32;
                
                // Add the input wheel bend
                let input_pb_norm = (state.input_pitch_bend as f32 - 8192.0) / 8192.0; // -1.0 to 1.0
                let total_semitones = semitone_diff + (input_pb_norm * wheel_range_semitones);
                
                // Translate total semitones into output synth pitch bend value
                let pb_val = (8192.0 + (total_semitones / state.pitch_bend_range as f32) * 8192.0).round().clamp(0.0, 16383.0) as u16;
                
                // Save the base tuning error into state so we can dynamically adjust it if the wheel moves
                state.channel_busy[chan as usize] = true;
                state.note_to_channel[input_note] = Some((chan, nearest_note, semitone_diff));
                state.last_allocated = chan;
                
                let _ = state.out_conn.send(&[0xE0 | chan, (pb_val & 0x7F) as u8, (pb_val >> 7) as u8]);
                let _ = state.out_conn.send(&[msg_type | chan, nearest_note, message[2]]);
            }
        } else if let Some((chan, actual_sent_note, _)) = state.note_to_channel[input_note] {
            state.channel_busy[chan as usize] = false;
            state.note_to_channel[input_note] = None;
            let _ = state.out_conn.send(&[msg_type | chan, actual_sent_note, message[2]]);
        }
        
    // --- 2. PITCH BEND MESSAGES ---
    } else if msg_type == 0xE0 && message.len() >= 3 {
        // Intercept Pitch Bend and update our master state
        let pb_in = (message[1] as u16) | ((message[2] as u16) << 7);
        state.input_pitch_bend = pb_in;
        
        let input_pb_norm = (pb_in as f32 - 8192.0) / 8192.0;
        let wheel_semitone_shift = input_pb_norm * wheel_range_semitones;
        
        // Loop through all playing voices and update their Pitch Bend on the fly
        for voice_state in state.note_to_channel.iter() {
            if let Some((chan, _, base_semitone_diff)) = voice_state {
                // Add the new wheel position to this voice's unique tuning requirement
                let total_semitones = base_semitone_diff + wheel_semitone_shift;
                let pb_val = (8192.0 + (total_semitones / state.pitch_bend_range as f32) * 8192.0).round().clamp(0.0, 16383.0) as u16;
                
                // Send the newly combined value specifically to this channel
                let _ = state.out_conn.send(&[0xE0 | *chan, (pb_val & 0x7F) as u8, (pb_val >> 7) as u8]);
            }
        }

    // --- 3. ALL OTHER MESSAGES ---
    } else {
        if status >= 0xF0 {
            // Pass System messages through unmodified
            let _ = state.out_conn.send(message);
        } else {
            // Broadcast CCs, Mod Wheel, Sustain to all allocated channels
            let mut out_msg = message.to_vec();
            for chan in 0..state.num_channels {
                out_msg[0] = msg_type | chan;
                let _ = state.out_conn.send(&out_msg);
            }
        }
    }
}

// --- Helper Functions ---
fn select_port<T: midir::MidiIO>(io: &T, pt: &str) -> Result<T::Port, Box<dyn Error>> {
    let ports = io.ports();
    for (i, p) in ports.iter().enumerate() { println!("{}: {}", i, io.port_name(p)?); }
    print!("Select {} port: ", pt); stdout().flush()?;
    let mut s = String::new(); stdin().read_line(&mut s)?;
    Ok(ports.into_iter().nth(s.trim().parse()?).ok_or("Invalid")?)
}

fn get_num_channels() -> Result<u8, Box<dyn Error>> { 
    print!("Channels: "); stdout().flush()?; 
    let mut s=String::new(); stdin().read_line(&mut s)?; Ok(s.trim().parse()?) 
}

fn get_pitch_bend_range() -> Result<u8, Box<dyn Error>> { 
    print!("Synthesizer PB Range (1-48): "); stdout().flush()?; 
    let mut s=String::new(); stdin().read_line(&mut s)?; Ok(s.trim().parse()?) 
}