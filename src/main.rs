use midir::{MidiInput, MidiOutput, MidiOutputConnection};
use std::error::Error;
use std::fs;
use std::io::{stdin, stdout, Write};
use std::sync::{Arc, Mutex};

struct MidiState {
    out_conn: MidiOutputConnection,
    num_channels: u8,
    note_to_channel: [Option<(u8, u8, f32)>; 128], 
    channel_busy: Vec<bool>,
    last_allocated: u8,
    
    tuning: [f32; 128],
    pitch_bend_range: u8, // MUST be 48 for standard MPE
    
    synth_pitch_center: f32,
    synth_ref_note: u8,
    
    input_pitch_bend: u16, 
    
    // NEW: MPE Mode Toggle
    is_mpe: bool,
}

struct Kbm {
    map_size: i32,
    first_note: i32,
    last_note: i32,
    middle_note: i32,
    ref_note: i32,
    ref_freq: f32,
    formal_octave: i32,
    mapping: Vec<Option<i32>>, 
}

fn main() -> Result<(), Box<dyn Error>> {
    let midi_in = MidiInput::new("Poly Router Input")?;
    let midi_out = MidiOutput::new("Poly Router Output")?;

    let in_port = select_port(&midi_in, "input")?;
    let out_port = select_port(&midi_out, "output")?;
    
    // 1. Prompt for MPE mode first
    let is_mpe = prompt_mpe_mode()?;
    
    // 2. Ask for channels (For MPE, 15 is standard to cover channels 2-16)
    let num_channels = get_num_channels()?;
    
    // 3. Conditionally handle pitch bend range based on mode
    let pitch_bend_range = if is_mpe {
        println!("MPE Mode: Pitch Bend Range automatically locked to 48 semitones per specification.");
        48
    } else {
        get_pitch_bend_range()?
    };

    println!("\nConnecting...");
    let mut out_conn = midi_out.connect(&out_port, "poly-router-out")?;

    // 4. If MPE is selected, initialize the synth zone right away
    if is_mpe {
        send_mpe_configuration(&mut out_conn, num_channels);
        println!("MPE Configuration Message sent to Synth (Channel 1).");
    }

    let state = Arc::new(Mutex::new(MidiState {
        out_conn,
        num_channels,
        note_to_channel: [None; 128],
        channel_busy: vec![false; num_channels as usize],
        last_allocated: 0,
        tuning: [0.0; 128],
        pitch_bend_range,
        synth_pitch_center: 440.0,
        synth_ref_note: 69,
        input_pitch_bend: 8192,
        is_mpe, // Passes the chosen mode into our state
    }));

    update_tuning(state.clone(), "1");

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

    println!("\nProcessing. Press 1-9 for presets, 0 for .scl/.kbm file load, type 'grid' for Launchpad mapping, or 'q' to quit.");
    loop {
        let mut input = String::new();
        stdin().read_line(&mut input)?;
        let choice = input.trim();
        
        if choice == "q" { break; }
        
        if choice == "grid" {
            if let Err(e) = setup_grid_tuning(state.clone()) {
                println!("Grid setup error: {}", e);
            }
            continue;
        }
        
        if choice == "0" {
            let scl_path = prompt_input("Enter path to .scl file: ");
            let scl_path = scl_path.trim_matches('"').trim_matches('\'');
            
            match parse_scl(scl_path) {
                Ok(multipliers) => {
                    let kbm_path = prompt_input("Enter path to .kbm file (or press Enter for standard linear mapping): ");
                    let kbm_path = kbm_path.trim_matches('"').trim_matches('\'');

                    let kbm = if kbm_path.is_empty() {
                        Kbm {
                            map_size: 0, first_note: 0, last_note: 127,
                            middle_note: 69, ref_note: 69, ref_freq: 440.0,
                            formal_octave: (multipliers.len() - 1) as i32, mapping: vec![],
                        }
                    } else {
                        match parse_kbm(kbm_path) {
                            Ok(parsed) => parsed,
                            Err(e) => { println!("Error parsing .kbm file: {}", e); continue; }
                        }
                    };

                    match apply_custom_tuning(state.clone(), &multipliers, &kbm) {
                        Ok(_) => println!("Successfully loaded SCL/KBM tuning!"),
                        Err(e) => println!("Error applying tuning: {}", e),
                    }
                },
                Err(e) => println!("Error loading SCL file: {}", e),
            }
        } else {
            update_tuning(state.clone(), choice);
        }
    }
    Ok(())
}
fn prompt_input(prompt: &str) -> String {
    print!("{}", prompt);
    stdout().flush().unwrap();
    let mut s = String::new();
    stdin().read_line(&mut s).unwrap();
    s.trim().to_string()
}

fn setup_grid_tuning(state_mutex: Arc<Mutex<MidiState>>) -> Result<(), Box<dyn Error>> {
    println!("\n--- Launchpad S Grid Microtuning ---");
    let edo: f32 = prompt_input("EDO (e.g., 41): ").parse()?;
    let ref_midi: i32 = prompt_input("Reference MIDI number (e.g., 69 for A4): ").parse()?;
    let ref_pitch: f32 = prompt_input("Reference pitch in Hz (e.g., 440.0): ").parse()?;
    
    // Updated prompt text to remind the user of the bottom-to-top order
    let open_str_input = prompt_input("Open strings (8 integers offset from Ref, comma-separated, BOTTOM row first): ");
    
    // Changed to mut so we can reverse it
    let mut open_strings: Vec<i32> = open_str_input.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    if open_strings.len() != 8 {
        return Err("You must provide exactly 8 integers for the open strings.".into());
    }

    // --- NEW LOGIC ---
    // Reverse the array so index 0 (the user's first input) maps to the highest row index (Row 7 / bottom of the Launchpad)
    open_strings.reverse();

    let steps_input = prompt_input("Horizontal step sizes (1 integer for uniform steps, or 9 comma-separated integers): ");
    let horiz_steps: Vec<i32> = steps_input.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    if horiz_steps.is_empty() {
        return Err("You must provide at least 1 horizontal step size.".into());
    }

    let scroll: i32 = prompt_input("Scroll offset (integer, e.g. 0): ").parse()?;

    // Helper to calculate cumulative horizontal steps based on a cycling step array
    let calc_horiz_offset = |fret: i32| -> i32 {
        let mut offset = 0;
        if fret > 0 {
            for i in 0..fret {
                offset += horiz_steps[i as usize % horiz_steps.len()];
            }
        } else if fret < 0 {
            for i in fret..0 {
                let idx = i.rem_euclid(horiz_steps.len() as i32) as usize;
                offset -= horiz_steps[idx];
            }
        }
        offset
    };

    let mut new_tuning = [0.0; 128]; 
    
    for row in 0..8 {
        for col in 0..9 {
            let midi_note = row * 16 + col;
            if midi_note < 128 {
                let current_fret = col + scroll;
                let h_offset = calc_horiz_offset(current_fret);
                
                // Because we reversed the array, row 0 (top of the launchpad) now grabs the last element the user typed
                let total_edo_steps = open_strings[row as usize] + h_offset;
                
                // Calculate frequency
                let hz = ref_pitch * 2.0_f32.powf(total_edo_steps as f32 / edo);
                new_tuning[midi_note as usize] = hz;
            }
        }
    }

    let mut state = state_mutex.lock().unwrap();
    state.tuning = new_tuning;
    println!("Successfully mapped Launchpad S grid to {} EDO!", edo);
    
    Ok(())
}

fn apply_custom_tuning(state_mutex: Arc<Mutex<MidiState>>, multipliers: &[f32], kbm: &Kbm) -> Result<(), String> {
    let n = (multipliers.len() - 1) as i32; 
    let period = multipliers[n as usize];   
    
    let calc_ratio = |degree: i32| -> f32 {
        let q = degree.div_euclid(n);
        let r = degree.rem_euclid(n) as usize;
        period.powi(q) * multipliers[r]
    };

    let ref_degree = if kbm.map_size == 0 {
        kbm.ref_note - kbm.middle_note
    } else {
        let diff = kbm.ref_note - kbm.middle_note;
        let cycles = diff.div_euclid(kbm.map_size);
        let index = diff.rem_euclid(kbm.map_size) as usize;
        if let Some(&Some(mapped_val)) = kbm.mapping.get(index) {
            mapped_val + cycles * kbm.formal_octave
        } else {
            return Err("The specified Reference Note maps to an unmapped 'x' key. This is invalid.".to_string());
        }
    };

    let ref_ratio = calc_ratio(ref_degree);
    let base_freq = kbm.ref_freq / ref_ratio; 
    let mut new_tuning = [0.0; 128]; 
    
    for i in 0..128 {
        if i < kbm.first_note || i > kbm.last_note { continue; }

        let degree = if kbm.map_size == 0 {
            i - kbm.middle_note
        } else {
            let diff = i - kbm.middle_note;
            let cycles = diff.div_euclid(kbm.map_size);
            let index = diff.rem_euclid(kbm.map_size) as usize;
            match kbm.mapping.get(index) {
                Some(&Some(mapped_val)) => mapped_val + cycles * kbm.formal_octave,
                _ => continue, 
            }
        };
        new_tuning[i as usize] = base_freq * calc_ratio(degree);
    }
    
    let mut state = state_mutex.lock().unwrap();
    state.tuning = new_tuning;
    Ok(())
}

fn parse_scl(path: &str) -> Result<Vec<f32>, Box<dyn Error>> {
    let contents = fs::read_to_string(path)?;
    let mut lines = contents.lines().filter(|l| !l.trim().starts_with('!'));

    let _description = lines.next().ok_or("Missing description line")?;
    let mut num_notes_line = lines.next().ok_or("Missing number of notes")?.trim();
    while num_notes_line.is_empty() { num_notes_line = lines.next().ok_or("Missing")?.trim(); }
    let num_notes: usize = num_notes_line.parse()?;
    
    if num_notes == 0 { return Err("0-note scales are not currently supported.".into()); }

    let mut multipliers = Vec::with_capacity(num_notes + 1);
    multipliers.push(1.0);

    let mut count = 0;
    for line in lines {
        if count >= num_notes { break; }
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; } 
        
        let token = trimmed.split_whitespace().next().unwrap_or("");
        let multiplier = if token.contains('.') {
            2.0_f32.powf(token.parse::<f32>()? / 1200.0)
        } else if token.contains('/') {
            let mut parts = token.split('/');
            parts.next().unwrap().parse::<f32>()? / parts.next().unwrap().parse::<f32>()?
        } else {
            token.parse::<f32>()?
        };

        if multiplier <= 0.0 { return Err("Invalid ratio".into()); }
        multipliers.push(multiplier);
        count += 1;
    }
    Ok(multipliers)
}

fn parse_kbm(path: &str) -> Result<Kbm, Box<dyn Error>> {
    let contents = fs::read_to_string(path)?;
    let mut lines = contents.lines().filter(|l| !l.trim().starts_with('!'));

    let mut next_val = || -> Result<&str, Box<dyn Error>> {
        loop {
            let l = lines.next().ok_or("Unexpected EOF in .kbm file")?.trim();
            if !l.is_empty() { return Ok(l); }
        }
    };

    let map_size: i32 = next_val()?.parse()?;
    let first_note: i32 = next_val()?.parse()?;
    let last_note: i32 = next_val()?.parse()?;
    let middle_note: i32 = next_val()?.parse()?;
    let ref_note: i32 = next_val()?.parse()?;
    let ref_freq: f32 = next_val()?.parse()?;
    let formal_octave: i32 = next_val()?.parse()?;

    let mut mapping = Vec::new();
    if map_size > 0 {
        for _ in 0..map_size {
            if let Ok(l) = next_val() {
                let val = l.split_whitespace().next().unwrap_or("x");
                if val.eq_ignore_ascii_case("x") {
                    mapping.push(None);
                } else {
                    mapping.push(Some(val.parse()?));
                }
            } else { break; }
        }
    }
    Ok(Kbm { map_size, first_note, last_note, middle_note, ref_note, ref_freq, formal_octave, mapping })
}

fn update_tuning(state_mutex: Arc<Mutex<MidiState>>, choice: &str) {
    let choice_char = choice.chars().next().unwrap_or(' ');
    let mut state = state_mutex.lock().unwrap();
    let pitch_ref = state.synth_ref_note as f32;
    let pitch_center = state.synth_pitch_center;

    match choice_char {
        '1' => { for i in 0..128 { state.tuning[i] = pitch_center * 2.0f32.powf((i as f32 - pitch_ref) / 12.0); } }
        '2' => { for i in 0..128 { state.tuning[i] = pitch_center * 2.0f32.powf((i as f32 - pitch_ref) / 24.0); } }
        '3' => { 
            let ratios = [1.0, 17.0/16.0, 9.0/8.0, 6.0/5.0, 5.0/4.0, 4.0/3.0, 11.0/8.0, 3.0/2.0, 13.0/8.0, 5.0/3.0, 7.0/4.0, 15.0/8.0];
            let base_c_freq = pitch_center * (3.0 / 5.0); 
            for i in 0..128 {
                let octave = (i / 12) as i32 - 5; 
                state.tuning[i] = base_c_freq * ratios[(i % 12) as usize] * 2.0f32.powi(octave);
            }
        }
        '4'..='9' => { 
            let n = match choice_char { '4'=>17, '5'=>19, '6'=>22, '7'=>31, '8'=>41, '9'=>53, _=>12 };
            for i in 0..128 { state.tuning[i] = pitch_center * 2.0f32.powf((i as f32 - pitch_ref) / n as f32); }
        }
        _ => { return; }
    }
    println!("Preset {} loaded.", choice);
}

fn process_midi(message: &[u8], state: &mut MidiState) {
    if message.is_empty() { return; }
    let status = message[0];
    let msg_type = status & 0xF0;
    let wheel_range_semitones = 1.0;
    
    // --- MPE MODE ROUTING ---
    if state.is_mpe {
        if message.len() >= 3 && (msg_type == 0x90 || msg_type == 0x80) {
            let input_note = message[1] as usize;
            let is_note_on = msg_type == 0x90 && message[2] > 0;

            if is_note_on {
                let target_hz = state.tuning[input_note];
                if target_hz <= 0.0 { return; } 

                // 1. Allocate channels from 1 to num_channels (Channel 0 is strictly the Master)
                let mut assigned_chan = None;
                for i in 1..=state.num_channels {
                    // Cycles strictly between 1 and num_channels inclusive
                    let chan = 1 + (state.last_allocated + i) % state.num_channels;
                    if !state.channel_busy[chan as usize] { assigned_chan = Some(chan); break; }
                }

                if let Some(chan) = assigned_chan {
                    let exact_note = state.synth_ref_note as f32 + 12.0 * (target_hz / state.synth_pitch_center).log2();
                    let nearest_note = exact_note.round().clamp(0.0, 127.0) as u8;
                    
                    // 2. Microtonal Pitch Bend ONLY
                    let semitone_diff = exact_note - nearest_note as f32;
                    let pb_val = (8192.0 + (semitone_diff / state.pitch_bend_range as f32) * 8192.0).round().clamp(0.0, 16383.0) as u16;
                    
                    state.channel_busy[chan as usize] = true;
                    // We no longer need to save the semitone_diff in state for dynamic updates!
                    state.note_to_channel[input_note] = Some((chan, nearest_note, 0.0));
                    state.last_allocated = chan;
                    
                    let _ = state.out_conn.send(&[0xE0 | chan, (pb_val & 0x7F) as u8, (pb_val >> 7) as u8]);
                    let _ = state.out_conn.send(&[msg_type | chan, nearest_note, message[2]]);
                }
            } else if let Some((chan, actual_sent_note, _)) = state.note_to_channel[input_note] {
                state.channel_busy[chan as usize] = false;
                state.note_to_channel[input_note] = None;
                let _ = state.out_conn.send(&[msg_type | chan, actual_sent_note, message[2]]);
            }
            
        } else if status >= 0xF0 {
            // System Messages (Clock, SysEx) -> Pass through
            let _ = state.out_conn.send(message);
        } else {
            // 3. ALL OTHER MESSAGES (Pitch Wheel, CCs, Sustain, Channel Pressure)
            // Force the channel nibble to 0 (Master Channel) and send EXACTLY ONCE.
            // The MPE Synthesizer will automatically apply these to all active member notes.
            let mut out_msg = message.to_vec();
            out_msg[0] = msg_type | 0x00; 
            let _ = state.out_conn.send(&out_msg);
        }

    // --- STANDARD MULTI-TIMBRAL MODE ROUTING (Legacy) ---
    } else {
        if message.len() >= 3 && (msg_type == 0x90 || msg_type == 0x80) {
            let input_note = message[1] as usize;
            let is_note_on = msg_type == 0x90 && message[2] > 0;

            if is_note_on {
                let target_hz = state.tuning[input_note];
                if target_hz <= 0.0 { return; } // Unmapped or off-grid key

                let mut assigned_chan = None;
                for i in 1..=state.num_channels {
                    let chan = (state.last_allocated + i) % state.num_channels;
                    if !state.channel_busy[chan as usize] { assigned_chan = Some(chan); break; }
                }

                if let Some(chan) = assigned_chan {
                    let exact_note = state.synth_ref_note as f32 + 12.0 * (target_hz / state.synth_pitch_center).log2();
                    let nearest_note = exact_note.round().clamp(0.0, 127.0) as u8;
                    
                    let semitone_diff = exact_note - nearest_note as f32;
                    let input_pb_norm = (state.input_pitch_bend as f32 - 8192.0) / 8192.0; 
                    let total_semitones = semitone_diff + (input_pb_norm * wheel_range_semitones);
                    let pb_val = (8192.0 + (total_semitones / state.pitch_bend_range as f32) * 8192.0).round().clamp(0.0, 16383.0) as u16;
                    
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
            
        } else if msg_type == 0xE0 && message.len() >= 3 {
            let pb_in = (message[1] as u16) | ((message[2] as u16) << 7);
            state.input_pitch_bend = pb_in;
            let input_pb_norm = (pb_in as f32 - 8192.0) / 8192.0;
            let wheel_semitone_shift = input_pb_norm * wheel_range_semitones;
            
            for voice_state in state.note_to_channel.iter() {
                if let Some((chan, _, base_semitone_diff)) = voice_state {
                    let total_semitones = base_semitone_diff + wheel_semitone_shift;
                    let pb_val = (8192.0 + (total_semitones / state.pitch_bend_range as f32) * 8192.0).round().clamp(0.0, 16383.0) as u16;
                    let _ = state.out_conn.send(&[0xE0 | *chan, (pb_val & 0x7F) as u8, (pb_val >> 7) as u8]);
                }
            }

        } else {
            if status >= 0xF0 {
                let _ = state.out_conn.send(message);
            } else {
                let mut out_msg = message.to_vec();
                for chan in 0..state.num_channels {
                    out_msg[0] = msg_type | chan;
                    let _ = state.out_conn.send(&out_msg);
                }
            }
        }
    }
}

fn send_mpe_configuration(out_conn: &mut MidiOutputConnection, member_channels: u8) {
    // Send RPN 06 to Channel 1 (0x00) to configure MPE Zone 1
    let messages = [
        [0xB0, 101, 0],               // RPN MSB: 0
        [0xB0, 100, 6],               // RPN LSB: 6 (MPE Configuration)
        [0xB0, 6, member_channels],   // Data Entry MSB: Number of member channels (e.g., 15)
        [0xB0, 38, 0],                // Data Entry LSB: 0
    ];
    for msg in messages.iter() {
        let _ = out_conn.send(msg);
    }
}

fn prompt_mpe_mode() -> Result<bool, Box<dyn Error>> {
    print!("Select MIDI Output Mode (1: Standard Multi-timbral, 2: MPE): ");
    stdout().flush()?;
    let mut s = String::new();
    stdin().read_line(&mut s)?;
    match s.trim() {
        "2" => Ok(true),
        _ => Ok(false), // Default to standard multi-timbral
    }
}

fn send_mpe_configuration(out_conn: &mut MidiOutputConnection, member_channels: u8) {
    // Send RPN 06 to Channel 1 (0xB0) to configure MPE Zone 1
    let messages = [
        [0xB0, 101, 0],               // RPN MSB: 0
        [0xB0, 100, 6],               // RPN LSB: 6 (MPE Configuration)
        [0xB0, 6, member_channels],   // Data Entry MSB: Number of member channels (e.g., 15)
        [0xB0, 38, 0],                // Data Entry LSB: 0
    ];
    for msg in messages.iter() {
        let _ = out_conn.send(msg);
    }
}

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