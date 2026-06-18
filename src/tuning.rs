use std::error::Error;
use std::fs;
use std::io::{stdin, stdout, Write};
use std::sync::{Arc, Mutex};
use crate::midi::MidiState;

pub struct Kbm {
    pub map_size: i32, pub first_note: i32, pub last_note: i32,
    pub middle_note: i32, pub ref_note: i32, pub ref_freq: f32,
    pub formal_octave: i32, pub mapping: Vec<Option<i32>>, 
}

pub fn prompt_input(prompt: &str) -> String {
    print!("{}", prompt);
    stdout().flush().unwrap();
    let mut s = String::new();
    stdin().read_line(&mut s).unwrap();
    s.trim().to_string()
}

pub fn setup_grid_tuning(state_mutex: Arc<Mutex<MidiState>>) -> Result<String, Box<dyn Error>> {
    println!("\n--- Launchpad S Grid Microtuning ---");
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
    Ok(format!("Mapped Launchpad S grid to {} EDO!", edo))
}

pub fn apply_custom_tuning(state_mutex: Arc<Mutex<MidiState>>, multipliers: &[f32], kbm: &Kbm) -> Result<(), String> {
    let n = (multipliers.len() - 1) as i32; 
    let period = multipliers[n as usize];   
    let calc_ratio = |degree: i32| -> f32 { period.powi(degree.div_euclid(n)) * multipliers[degree.rem_euclid(n) as usize] };

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

pub fn parse_scl(path: &str) -> Result<Vec<f32>, Box<dyn Error>> {
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

pub fn parse_kbm(path: &str) -> Result<Kbm, Box<dyn Error>> {
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

pub fn update_tuning(state_mutex: Arc<Mutex<MidiState>>, choice: &str) -> bool {
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
        _ => return false,
    }
    true
}