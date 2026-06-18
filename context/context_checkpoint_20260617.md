# Technical Specification & Recreation Guide: Polyphonic Microtonal MIDI Router

## 1. Project Overview
This project is a high-performance, real-time polyphonic MIDI router built in Rust using the `midir` library. It acts as a middleware layer between a standard MIDI controller and a hardware/software synthesizer. 

**Primary Function:** To enable standard MIDI controllers to play polyphonically in microtonal, non-standard, and custom-defined tuning temperaments (e.g., Just Intonation, N-EDO scales, Scala files) on synthesizers that otherwise only support standard 12-TET.

**Key Features:**
* **Dynamic Voice Allocation:** Round-robin channel assignment for multi-timbral polyphony.
* **MPE Support:** Full support for MIDI Polyphonic Expression, including Master/Member channel routing and automatic MPE Configuration Message (MCM) initialization.
* **Pitch Bend Stacking:** Mathematically combines live physical pitch wheel input with microtonal offset requirements.
* **Hardware Quirks Handling:** Bypasses the "Note-On Reset" bug present in many synthesizers.
* **Custom Tunings:** Built-in presets, `.scl` (Scala scale) parsing, and `.kbm` (Scala keyboard mapping) parsing.
* **Grid Controller Mapping:** Native isomorphic "fretboard" layouts for grid controllers like the Launchpad S.

---

## 2. Core Architecture

### 2.1 Dependencies (`Cargo.toml`)
To recreate this project, initialize a new Rust binary (`cargo new poly-router`) and add `midir`:
```toml
[dependencies]
midir = "0.9"
```

### 2.2 Thread-Safe State Management
Because `midir` processes incoming MIDI events asynchronously inside a background callback thread, the application state is wrapped in `Arc<Mutex<MidiState>>`. This allows the main thread to listen for console commands while the callback thread processes real-time MIDI data.

### 2.3 `MidiState` Structure
```rust
struct MidiState {
    out_conn: MidiOutputConnection,
    num_channels: u8,
    note_to_channel: [Option<(u8, u8, f32)>; 128], // Maps input note to (Channel, Nearest Output Note, Base Semitone Offset)
    channel_busy: Vec<bool>, // ALWAYS initialized with 16 elements to prevent index out-of-bounds
    last_allocated: u8,
    
    tuning: [f32; 128], // Target frequency (Hz) for each MIDI note index. 0.0 means unmapped/ignored.
    pitch_bend_range: u8, // Set to 48 automatically in MPE mode, or user-defined in Multi-timbral mode.
    
    synth_pitch_center: f32, // Hardware calibration (usually 440.0)
    synth_ref_note: u8, // Hardware calibration (usually 69 for A4)
    
    input_pitch_bend: u16, // Tracks live position of physical pitch wheel
    is_mpe: bool, // Toggles routing logic
}
```

---

## 3. Core Logic & Math

### 3.1 The "Note-On Reset" Hardware Quirk
By official MIDI and MPE specs, pitch bend should be sent *before* Note On. However, many synthesizers automatically reset pitch bend to zero the moment they receive a Note On message, wiping out the microtonal tuning.
**The Fix:** The router intentionally sends the `Note On` message **first**, followed immediately by the `Pitch Bend` message. 

### 3.2 Pitch Bend Stacking
Because voices use Pitch Bend to reach their microtonal frequencies, the physical pitch wheel cannot just be passed through. 
* **Multi-Timbral Mode:** Physical PB is normalized (-1.0 to 1.0), multiplied by a predefined semitone range (e.g., 1.0), and added to the voice's base retuning offset. A unique PB message is sent to *every* active channel.
* **MPE Mode:** Physical PB is sent *only* to the Master Channel (Channel 1). The microtonal PB offset is sent *only* to the specific Member Channel. The synth natively adds them together.

### 3.3 MPE Channel Cycling Math
MIDI channels are 1-indexed (1-16), but arrays are 0-indexed. MPE uses Channel 1 as the Master, leaving Channels 2-16 for notes. The router uses modulo math to seamlessly cycle through these:
```rust
let last_offset = if state.last_allocated > 0 { state.last_allocated - 1 } else { 0 };
let idx = (last_offset + i + 1) % state.num_channels;
let chan = 1 + idx; // Maps to channels 2-16
```

### 3.4 Scala File Processing
* **`.scl` (Scale):** Parses cents (`.`) vs ratios (`/`). Uses **Euclidean Division** (`div_euclid` and `rem_euclid`) to tile the scale infinitely up and down the keyboard from the reference note.
* **`.kbm` (Mapping):** Reassigns physical keys to scale degrees. Handles unmapped keys (`x`) by assigning them a frequency of `0.0`. Drops any note-on events targeting `0.0`.

### 3.5 Grid / Isomorphic Layouts
Calculates Hz based on row/column coordinates on a grid (like a Launchpad where Row 0 is note 0, Row 1 is note 16, etc.). 
* User inputs "Open Strings" (leftmost column values).
* The array is **reversed** so the first user input maps to the *bottom* physical row of the controller (highest MIDI note index).

---

## 4. Complete Source Code (`src/main.rs`)

```rust
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
    
    synth_pitch_center: f32,
    synth_ref_note: u8,
    
    input_pitch_bend: u16, 
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

fn prompt_mpe_mode() -> Result<bool, Box<dyn Error>> {
    print!("Select MIDI Output Mode (1: Standard Multi-timbral, 2: MPE): ");
    stdout().flush()?;
    let mut s = String::new();
    stdin().read_line(&mut s)?;
    match s.trim() {
        "2" => Ok(true),
        _ => Ok(false),
    }
}

fn send_mpe_configuration(out_conn: &mut MidiOutputConnection, member_channels: u8) {
    let messages = [
        [0xB0, 101, 0],               
        [0xB0, 100, 6],               
        [0xB0, 6, member_channels],   
        [0xB0, 38, 0],                
    ];
    for msg in messages.iter() {
        let _ = out_conn.send(msg);
    }
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
    } else {
        get_pitch_bend_range()?
    };

    println!("Connecting...");
    let mut out_conn = midi_out.connect(&out_port, "poly-router-out")?;

    if is_mpe {
        send_mpe_configuration(&mut out_conn, num_channels);
        println!("MPE Configuration Message sent to Synth (Channel 1).");
    }

    let state = Arc::new(Mutex::new(MidiState {
        out_conn,
        num_channels,
        note_to_channel: [None; 128],
        channel_busy: vec![false; 16], // Size to 16 for safety
        last_allocated: 0,
        tuning: [0.0; 128],
        pitch_bend_range,
        synth_pitch_center: 440.0,
        synth_ref_note: 69,
        input_pitch_bend: 8192,
        is_mpe, 
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

    println!("Processing. Press 1-9 for presets, 0 for .scl/.kbm, 'grid' for Launchpad mapping, or 'q' to quit.");
    loop {
        let mut input = String::new();
        stdin().read_line(&mut input)?;
        let choice = input.trim();
        
        if choice == "q" { break; }
        if choice == "grid" {
            if let Err(e) = setup_grid_tuning(state.clone()) { println!("Error: {}", e); }
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
                            Err(e) => { println!("Error: {}", e); continue; }
                        }
                    };

                    match apply_custom_tuning(state.clone(), &multipliers, &kbm) {
                        Ok(_) => println!("Loaded SCL/KBM tuning!"),
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
    println!("--- Launchpad S Grid Microtuning ---");
    let edo: f32 = prompt_input("EDO (e.g., 41): ").parse()?;
    let ref_pitch: f32 = prompt_input("Reference pitch in Hz (e.g., 440.0): ").parse()?;
    let open_str_input = prompt_input("Open strings (8 integers offset from Ref, comma-separated, BOTTOM row first): ");
    
    let mut open_strings: Vec<i32> = open_str_input.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    if open_strings.len() != 8 { return Err("Provide exactly 8 integers.".into()); }
    open_strings.reverse();

    let steps_input = prompt_input("Horizontal step sizes (1 integer for uniform steps, or 9 comma-separated integers): ");
    let horiz_steps: Vec<i32> = steps_input.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    if horiz_steps.is_empty() { return Err("Provide at least 1 horizontal step size.".into()); }

    let scroll: i32 = prompt_input("Scroll offset (integer, e.g. 0): ").parse()?;

    let calc_horiz_offset = |fret: i32| -> i32 {
        let mut offset = 0;
        if fret > 0 { for i in 0..fret { offset += horiz_steps[i as usize % horiz_steps.len()]; } }
        else if fret < 0 { for i in fret..0 { offset -= horiz_steps[i.rem_euclid(horiz_steps.len() as i32) as usize]; } }
        offset
    };

    let mut new_tuning = [0.0; 128]; 
    for row in 0..8 {
        for col in 0..9 {
            let midi_note = row * 16 + col;
            if midi_note < 128 {
                let h_offset = calc_horiz_offset(col + scroll);
                let total_edo_steps = open_strings[row as usize] + h_offset;
                new_tuning[midi_note as usize] = ref_pitch * 2.0_f32.powf(total_edo_steps as f32 / edo);
            }
        }
    }

    let mut state = state_mutex.lock().unwrap();
    state.tuning = new_tuning;
    println!("Mapped Launchpad S grid to {} EDO!", edo);
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

    let ref_degree = if kbm.map_size == 0 { kbm.ref_note - kbm.middle_note } else {
        let diff = kbm.ref_note - kbm.middle_note;
        let index = diff.rem_euclid(kbm.map_size) as usize;
        if let Some(&Some(mapped_val)) = kbm.mapping.get(index) {
            mapped_val + diff.div_euclid(kbm.map_size) * kbm.formal_octave
        } else { return Err("Reference Note maps to 'x'.".to_string()); }
    };

    let base_freq = kbm.ref_freq / calc_ratio(ref_degree); 
    let mut new_tuning = [0.0; 128]; 
    
    for i in 0..128 {
        if i < kbm.first_note || i > kbm.last_note { continue; }
        let degree = if kbm.map_size == 0 { i - kbm.middle_note } else {
            let diff = i - kbm.middle_note;
            match kbm.mapping.get(diff.rem_euclid(kbm.map_size) as usize) {
                Some(&Some(mapped_val)) => mapped_val + diff.div_euclid(kbm.map_size) * kbm.formal_octave,
                _ => continue, 
            }
        };
        new_tuning[i as usize] = base_freq * calc_ratio(degree);
    }
    
    state_mutex.lock().unwrap().tuning = new_tuning;
    Ok(())
}

fn parse_scl(path: &str) -> Result<Vec<f32>, Box<dyn Error>> {
    let contents = fs::read_to_string(path)?;
    let mut lines = contents.lines().filter(|l| !l.trim().starts_with('!'));
    let _ = lines.next().ok_or("Missing description")?;
    let mut nn_line = lines.next().ok_or("Missing")?.trim();
    while nn_line.is_empty() { nn_line = lines.next().ok_or("Missing")?.trim(); }
    let num_notes: usize = nn_line.parse()?;
    
    if num_notes == 0 { return Err("0-note unsupported.".into()); }
    let mut multipliers = Vec::with_capacity(num_notes + 1);
    multipliers.push(1.0);

    let mut count = 0;
    for line in lines {
        if count >= num_notes { break; }
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; } 
        let token = trimmed.split_whitespace().next().unwrap_or("");
        let multiplier = if token.contains('.') { 2.0_f32.powf(token.parse::<f32>()? / 1200.0) } 
                         else if token.contains('/') { let mut p = token.split('/'); p.next().unwrap().parse::<f32>()? / p.next().unwrap().parse::<f32>()? } 
                         else { token.parse::<f32>()? };
        if multiplier <= 0.0 { return Err("Invalid".into()); }
        multipliers.push(multiplier);
        count += 1;
    }
    Ok(multipliers)
}

fn parse_kbm(path: &str) -> Result<Kbm, Box<dyn Error>> {
    let contents = fs::read_to_string(path)?;
    let mut lines = contents.lines().filter(|l| !l.trim().starts_with('!'));
    let mut next_val = || -> Result<&str, Box<dyn Error>> { loop { let l = lines.next().ok_or("EOF")?.trim(); if !l.is_empty() { return Ok(l); } } };

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
                if val.eq_ignore_ascii_case("x") { mapping.push(None); } else { mapping.push(Some(val.parse()?)); }
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
            for i in 0..128 { state.tuning[i] = base_c_freq * ratios[(i % 12) as usize] * 2.0f32.powi((i / 12) as i32 - 5); }
        }
        '4'..='9' => { 
            let n = match choice_char { '4'=>17, '5'=>19, '6'=>22, '7'=>31, '8'=>41, '9'=>53, _=>12 };
            for i in 0..128 { state.tuning[i] = pitch_center * 2.0f32.powf((i as f32 - pitch_ref) / n as f32); }
        }
        _ => return,
    }
    println!("Preset {} loaded.", choice);
}

fn process_midi(message: &[u8], state: &mut MidiState) {
    if message.is_empty() { return; }
    let status = message[0];
    let msg_type = status & 0xF0;
    let wheel_range_semitones = 1.0; 

    // --- MPE MODE ---
    if state.is_mpe {
        if message.len() >= 3 && (msg_type == 0x90 || msg_type == 0x80) {
            let input_note = message[1] as usize;
            let is_note_on = msg_type == 0x90 && message[2] > 0;

            if is_note_on {
                let target_hz = state.tuning[input_note];
                if target_hz <= 0.0 { return; } 

                let mut assigned_chan = None;
                for i in 0..state.num_channels {
                    let last_offset = if state.last_allocated > 0 { state.last_allocated - 1 } else { 0 };
                    let idx = (last_offset + i + 1) % state.num_channels;
                    let chan = 1 + idx; // Maps 0-14 to Channels 2-16

                    if !state.channel_busy[chan as usize] { assigned_chan = Some(chan); break; }
                }

                if let Some(chan) = assigned_chan {
                    let exact_note = state.synth_ref_note as f32 + 12.0 * (target_hz / state.synth_pitch_center).log2();
                    let nearest_note = exact_note.round().clamp(0.0, 127.0) as u8;
                    let pb_val = (8192.0 + ((exact_note - nearest_note as f32) / state.pitch_bend_range as f32) * 8192.0).round().clamp(0.0, 16383.0) as u16;
                    
                    state.channel_busy[chan as usize] = true;
                    state.note_to_channel[input_note] = Some((chan, nearest_note, 0.0));
                    state.last_allocated = chan;
                    
                    // IMPORTANT: Note On first, Pitch Bend second (bypasses Synth Note-On Quirk)
                    let _ = state.out_conn.send(&[msg_type | chan, nearest_note, message[2]]);
                    let _ = state.out_conn.send(&[0xE0 | chan, (pb_val & 0x7F) as u8, (pb_val >> 7) as u8]);
                }
            } else if let Some((chan, actual_sent_note, _)) = state.note_to_channel[input_note] {
                state.channel_busy[chan as usize] = false;
                state.note_to_channel[input_note] = None;
                let _ = state.out_conn.send(&[msg_type | chan, actual_sent_note, message[2]]);
            }
        } else if status >= 0xF0 {
            let _ = state.out_conn.send(message);
        } else {
            // MPE Expression: Send physical wheel/CCs ONLY to Master Channel (0x00)
            let mut out_msg = message.to_vec();
            out_msg[0] = msg_type | 0x00; 
            let _ = state.out_conn.send(&out_msg);
        }

    // --- STANDARD MULTI-TIMBRAL MODE ---
    } else {
        if message.len() >= 3 && (msg_type == 0x90 || msg_type == 0x80) {
            let input_note = message[1] as usize;
            let is_note_on = msg_type == 0x90 && message[2] > 0;

            if is_note_on {
                let target_hz = state.tuning[input_note];
                if target_hz <= 0.0 { return; } 

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
                    let pb_val = (8192.0 + ((semitone_diff + (input_pb_norm * wheel_range_semitones)) / state.pitch_bend_range as f32) * 8192.0).round().clamp(0.0, 16383.0) as u16;
                    
                    state.channel_busy[chan as usize] = true;
                    state.note_to_channel[input_note] = Some((chan, nearest_note, semitone_diff));
                    state.last_allocated = chan;
                    
                    // IMPORTANT: Note On first, Pitch Bend second
                    let _ = state.out_conn.send(&[msg_type | chan, nearest_note, message[2]]);
                    let _ = state.out_conn.send(&[0xE0 | chan, (pb_val & 0x7F) as u8, (pb_val >> 7) as u8]);
                }
            } else if let Some((chan, actual_sent_note, _)) = state.note_to_channel[input_note] {
                state.channel_busy[chan as usize] = false;
                state.note_to_channel[input_note] = None;
                let _ = state.out_conn.send(&[msg_type | chan, actual_sent_note, message[2]]);
            }
        } else if msg_type == 0xE0 && message.len() >= 3 {
            let pb_in = (message[1] as u16) | ((message[2] as u16) << 7);
            state.input_pitch_bend = pb_in;
            let wheel_semitone_shift = ((pb_in as f32 - 8192.0) / 8192.0) * wheel_range_semitones;
            
            for voice_state in state.note_to_channel.iter() {
                if let Some((chan, _, base_semitone_diff)) = voice_state {
                    let pb_val = (8192.0 + ((base_semitone_diff + wheel_semitone_shift) / state.pitch_bend_range as f32) * 8192.0).round().clamp(0.0, 16383.0) as u16;
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
```