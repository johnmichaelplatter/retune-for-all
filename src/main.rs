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
    pitch_bend_range: u8,
    
    // Hardware synthesizer calibration (Assume standard 440Hz unless physically retuned)
    synth_pitch_center: f32,
    synth_ref_note: u8,
    
    input_pitch_bend: u16, 
}

// Structure to hold our parsed .kbm file data
struct Kbm {
    map_size: i32,
    first_note: i32,
    last_note: i32,
    middle_note: i32,
    ref_note: i32,
    ref_freq: f32,
    formal_octave: i32,
    mapping: Vec<Option<i32>>, // None represents an unmapped 'x'
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
        synth_pitch_center: 440.0,
        synth_ref_note: 69,
        input_pitch_bend: 8192,
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

    println!("\nProcessing. Press 1-9 for presets, 0 for .scl/.kbm file load, or 'q' to quit.");
    loop {
        let mut input = String::new();
        stdin().read_line(&mut input)?;
        let choice = input.trim().chars().next().unwrap_or(' ');
        
        if choice == 'q' { break; }
        
        if choice == '0' {
            print!("Enter path to .scl file: ");
            stdout().flush()?;
            let mut scl_path = String::new();
            stdin().read_line(&mut scl_path)?;
            let scl_path = scl_path.trim().trim_matches('"').trim_matches('\'');
            
            match parse_scl(scl_path) {
                Ok(multipliers) => {
                    print!("Enter path to .kbm file (or press Enter for standard linear mapping): ");
                    stdout().flush()?;
                    let mut kbm_path = String::new();
                    stdin().read_line(&mut kbm_path)?;
                    let kbm_path = kbm_path.trim().trim_matches('"').trim_matches('\'');

                    let kbm = if kbm_path.is_empty() {
                        // Standard default linear mapping if no .kbm is provided
                        Kbm {
                            map_size: 0,
                            first_note: 0,
                            last_note: 127,
                            middle_note: 69,
                            ref_note: 69,
                            ref_freq: 440.0,
                            formal_octave: (multipliers.len() - 1) as i32,
                            mapping: vec![],
                        }
                    } else {
                        match parse_kbm(kbm_path) {
                            Ok(parsed) => parsed,
                            Err(e) => {
                                println!("Error parsing .kbm file: {}", e);
                                continue;
                            }
                        }
                    };

                    // Apply the scale and mapping mathematically
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

fn apply_custom_tuning(state_mutex: Arc<Mutex<MidiState>>, multipliers: &[f32], kbm: &Kbm) -> Result<(), String> {
    let n = (multipliers.len() - 1) as i32; // Number of notes in the scale
    let period = multipliers[n as usize];   // Period of repetition (usually 2/1)
    
    // Helper to calculate the raw frequency ratio for any mathematical scale degree
    let calc_ratio = |degree: i32| -> f32 {
        let q = degree.div_euclid(n);
        let r = degree.rem_euclid(n) as usize;
        period.powi(q) * multipliers[r]
    };

    // Find the scale degree of the Reference Note to calibrate the base frequency
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
    let base_freq = kbm.ref_freq / ref_ratio; // Frequency of scale degree 0
    
    let mut new_tuning = [0.0; 128]; // 0.0 acts as a sentinel for 'unmapped'
    
    for i in 0..128 {
        if i < kbm.first_note || i > kbm.last_note {
            continue; // Leaves tuning at 0.0 (unmapped)
        }

        let degree = if kbm.map_size == 0 {
            i - kbm.middle_note
        } else {
            let diff = i - kbm.middle_note;
            let cycles = diff.div_euclid(kbm.map_size);
            let index = diff.rem_euclid(kbm.map_size) as usize;
            match kbm.mapping.get(index) {
                Some(&Some(mapped_val)) => mapped_val + cycles * kbm.formal_octave,
                _ => continue, // Unmapped 'x' or trailing omitted mappings
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
            } else {
                break; // Premature EOF is allowed for unmapped trailing keys
            }
        }
    }

    Ok(Kbm { map_size, first_note, last_note, middle_note, ref_note, ref_freq, formal_octave, mapping })
}

fn update_tuning(state_mutex: Arc<Mutex<MidiState>>, choice: char) {
    let mut state = state_mutex.lock().unwrap();
    let pitch_ref = state.synth_ref_note as f32;
    let pitch_center = state.synth_pitch_center;

    match choice {
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
            let n = match choice { '4'=>17, '5'=>19, '6'=>22, '7'=>31, '8'=>41, '9'=>53, _=>12 };
            for i in 0..128 { state.tuning[i] = pitch_center * 2.0f32.powf((i as f32 - pitch_ref) / n as f32); }
        }
        _ => { if choice != '0' { println!("Invalid preset."); } return; }
    }
    println!("Preset {} loaded.", choice);
}

fn process_midi(message: &[u8], state: &mut MidiState) {
    if message.is_empty() { return; }
    let status = message[0];
    let msg_type = status & 0xF0;
    
    let wheel_range_semitones = 1.0; 

    if message.len() >= 3 && (msg_type == 0x90 || msg_type == 0x80) {
        let input_note = message[1] as usize;
        let is_note_on = msg_type == 0x90 && message[2] > 0;

        if is_note_on {
            let target_hz = state.tuning[input_note];
            
            // If the note's target frequency is 0.0, it is an unmapped 'x' key from the KBM file.
            // We safely drop the note and do not allocate a voice.
            if target_hz <= 0.0 { return; }

            let mut assigned_chan = None;
            for i in 1..=state.num_channels {
                let chan = (state.last_allocated + i) % state.num_channels;
                if !state.channel_busy[chan as usize] { assigned_chan = Some(chan); break; }
            }

            if let Some(chan) = assigned_chan {
                // Calculate physical MIDI parameters relative to the hardware Synth's calibration
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