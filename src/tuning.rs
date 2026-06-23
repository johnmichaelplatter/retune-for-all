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

pub fn apply_equal_division(state_mutex: Arc<Mutex<MidiState>>, divisions_str: &str, interval_str: &str) -> Result<String, String> {
    let divisions: f32 = divisions_str.parse().map_err(|_| "Invalid divisions integer")?;
    if divisions <= 0.0 { return Err("Divisions must be > 0".into()); }

    let ratio = if interval_str.contains('.') {
        let cents: f32 = interval_str.parse().map_err(|_| "Invalid cents")?;
        2.0_f32.powf(cents / 1200.0)
    } else if interval_str.contains('/') {
        let mut parts = interval_str.split('/');
        let num: f32 = parts.next().unwrap_or("").parse().map_err(|_| "Invalid numerator")?;
        let den: f32 = parts.next().unwrap_or("").parse().map_err(|_| "Invalid denominator")?;
        if den == 0.0 { return Err("Denominator cannot be 0".into()); }
        num / den
    } else {
        interval_str.parse::<f32>().map_err(|_| "Invalid interval ratio")?
    };

    if ratio <= 0.0 { return Err("Interval ratio must be > 0".into()); }

    let mut state = state_mutex.lock().unwrap();
    let pitch_ref = state.synth_ref_note as f32;
    let pitch_center = state.synth_pitch_center;

    for i in 0..128 {
        state.tuning[i] = pitch_center * ratio.powf((i as f32 - pitch_ref) / divisions);
    }

    Ok(format!("Applied Equal Division: {} steps of {}", divisions_str, interval_str))
}

pub fn apply_grid_tuning(
    state_mutex: Arc<Mutex<MidiState>>,
    edo_str: &str,
    _ref_midi_str: &str, 
    ref_pitch_str: &str,
    open_strings: &[String; 8],
    horiz_steps: &[String],
    capo_str: &str,
    octave_str: &str
) -> Result<String, String> {
    let edo: f32 = edo_str.parse().map_err(|_| "Invalid EDO")?;
    let ref_pitch: f32 = ref_pitch_str.parse().map_err(|_| "Invalid Ref Pitch")?;
    let capo: i32 = capo_str.parse().map_err(|_| "Invalid Capo")?;
    let octave: i32 = octave_str.parse().map_err(|_| "Invalid Octave")?;

    let mut parsed_open = [0; 8];
    for i in 0..8 { 
        parsed_open[i] = open_strings[i].parse().map_err(|_| format!("Invalid Open String {}", i+1))?; 
    }

    let mut parsed_horiz = Vec::new();
    for h in horiz_steps { 
        parsed_horiz.push(h.parse::<i32>().map_err(|_| "Invalid Horiz Step")?); 
    }
    if parsed_horiz.is_empty() { return Err("No horizontal steps provided".into()); }

    let calc_horiz_offset = |fret: i32| -> i32 {
        let mut offset = 0;
        if fret > 0 { for i in 0..fret { offset += parsed_horiz[i as usize % parsed_horiz.len()]; } }
        else if fret < 0 { for i in fret..0 { offset -= parsed_horiz[i.rem_euclid(parsed_horiz.len() as i32) as usize]; } }
        offset
    };

    let mut new_tuning = [0.0; 128]; 
    for row in 0..8 {
        for col in 0..9 {
            let midi_note = row * 16 + col;
            if midi_note < 128 {
                let h_offset = calc_horiz_offset(col + capo);
                let total_edo_steps = parsed_open[row as usize] + h_offset + (octave * edo as i32);
                new_tuning[midi_note as usize] = ref_pitch * 2.0_f32.powf(total_edo_steps as f32 / edo);
            }
        }
    }

    let mut state = state_mutex.lock().unwrap();
    state.tuning = new_tuning;
    Ok(format!("Mapped Guitar Grid to {} EDO!", edo))
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

// --- NEW NOTEPAD CONTENT PARSERS ---

pub fn parse_scl_content(lines: &[String]) -> Result<Vec<f32>, String> {
    let mut it = lines.iter().filter(|l| !l.trim().starts_with('!'));
    let _ = it.next().ok_or("Missing SCL description")?;
    let mut nn_line = it.next().ok_or("Missing SCL note count")?.trim();
    while nn_line.is_empty() { nn_line = it.next().ok_or("Missing SCL note count")?.trim(); }
    let num_notes: usize = nn_line.parse().map_err(|_| "Invalid SCL note count")?;
    
    if num_notes == 0 { return Err("0-note unsupported.".into()); }
    let mut multipliers = Vec::with_capacity(num_notes + 1);
    multipliers.push(1.0);

    for line in it {
        if multipliers.len() > num_notes { break; }
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; } 
        let token = trimmed.split_whitespace().next().unwrap_or("");
        let multiplier = if token.contains('.') { 
            2.0_f32.powf(token.parse::<f32>().map_err(|_| "Invalid cents")? / 1200.0) 
        } else if token.contains('/') { 
            let mut p = token.split('/'); 
            let num = p.next().unwrap_or("").parse::<f32>().map_err(|_| "Invalid numerator")?;
            let den = p.next().unwrap_or("").parse::<f32>().map_err(|_| "Invalid denominator")?;
            if den == 0.0 { return Err("Divide by zero".into()); }
            num / den
        } else { 
            token.parse::<f32>().map_err(|_| "Invalid ratio")? 
        };
        if multiplier <= 0.0 { return Err("Multiplier must be positive".into()); }
        multipliers.push(multiplier);
    }
    Ok(multipliers)
}

pub fn parse_kbm_content(lines: &[String]) -> Result<Kbm, String> {
    let mut it = lines.iter().filter(|l| !l.trim().starts_with('!'));
    let mut next_val = || -> Result<&str, String> { 
        loop { 
            let l = it.next().ok_or("Incomplete KBM data")?.trim(); 
            if !l.is_empty() { return Ok(l); } 
        } 
    };

    let map_size: i32 = next_val()?.parse().map_err(|_| "Invalid map size")?;
    let first_note: i32 = next_val()?.parse().map_err(|_| "Invalid first note")?;
    let last_note: i32 = next_val()?.parse().map_err(|_| "Invalid last note")?;
    let middle_note: i32 = next_val()?.parse().map_err(|_| "Invalid middle note")?;
    let ref_note: i32 = next_val()?.parse().map_err(|_| "Invalid reference note")?;
    let ref_freq: f32 = next_val()?.parse().map_err(|_| "Invalid reference frequency")?;
    let formal_octave: i32 = next_val()?.parse().map_err(|_| "Invalid formal octave")?;

    let mut mapping = Vec::new();
    if map_size > 0 {
        for _ in 0..map_size {
            if let Ok(l) = next_val() {
                let val = l.split_whitespace().next().unwrap_or("x");
                if val.eq_ignore_ascii_case("x") { mapping.push(None); } 
                else { mapping.push(Some(val.parse().map_err(|_| "Invalid map step")?)); }
            } else { break; }
        }
    }
    Ok(Kbm { map_size, first_note, last_note, middle_note, ref_note, ref_freq, formal_octave, mapping })
}

pub fn sync_notepad_tuning(state_mutex: Arc<Mutex<MidiState>>, scl_lines: &[String], kbm_lines: &[String]) -> Result<String, String> {
    let multipliers = parse_scl_content(scl_lines)?;
    let kbm = parse_kbm_content(kbm_lines).unwrap_or(Kbm { 
        map_size: 0, first_note: 0, last_note: 127, middle_note: 69, 
        ref_note: 69, ref_freq: 440.0, formal_octave: (multipliers.len() - 1) as i32, 
        mapping: vec![] 
    });
    apply_custom_tuning(state_mutex, &multipliers, &kbm)?;
    Ok(format!("Notepad tuned successfully ({} notes).", multipliers.len() - 1))
}

// Keep original file loaders for CommandInput fallback
pub fn parse_scl(path: &str) -> Result<Vec<f32>, Box<dyn Error>> {
    let contents = fs::read_to_string(path)?;
    let lines: Vec<String> = contents.lines().map(|s| s.to_string()).collect();
    parse_scl_content(&lines).map_err(|e| e.into())
}

pub fn parse_kbm(path: &str) -> Result<Kbm, Box<dyn Error>> {
    let contents = fs::read_to_string(path)?;
    let lines: Vec<String> = contents.lines().map(|s| s.to_string()).collect();
    parse_kbm_content(&lines).map_err(|e| e.into())
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