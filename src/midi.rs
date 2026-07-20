use midir::MidiOutputConnection;

pub struct MidiState {
    pub out_conn: Option<MidiOutputConnection>, 
    pub note_to_channel: [Option<(u8, u8, f32)>; 128], 
    pub channel_busy: Vec<bool>, 
    pub channel_enabled: [bool; 16], // Tracks user-toggled active channels
    pub last_allocated: u8,
    
    pub tuning: [f32; 128],
    pub pitch_bend_range: u8,
    pub synth_pitch_center: f32,
    pub synth_ref_note: u8,
    pub input_pitch_bend: u16, 
    pub is_mpe: bool,

    // UI flash trackers
    pub input_flash: u8,
    pub output_flash: [u8; 16],
}

impl Default for MidiState {
    fn default() -> Self {
        Self {
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
        }
    }
}

pub fn send_mpe_configuration(out_conn: &mut MidiOutputConnection, member_channels: u8) {
    let messages = [
        [0xB0, 101, 0], [0xB0, 100, 6],               
        [0xB0, 6, member_channels], [0xB0, 38, 0],                
    ];
    for msg in messages.iter() {
        let _ = out_conn.send(msg);
    }
}

// Helper to DRY up all the 14-bit pitch bend math
fn calculate_pitch_bend(semitone_diff: f32, pb_range: u8) -> u16 {
    (8192.0 + (semitone_diff / pb_range as f32) * 8192.0)
        .round()
        .clamp(0.0, 16383.0) as u16
}

pub fn process_midi(message: &[u8], state: &mut MidiState) {
    if message.is_empty() { return; }
    
    // Trigger UI flash for incoming MIDI
    state.input_flash = 5;

    if state.is_mpe {
        process_mpe_mode(message, state);
    } else {
        process_multitimbral_mode(message, state);
    }
}

fn process_mpe_mode(message: &[u8], state: &mut MidiState) {
    let status = message[0];
    let msg_type = status & 0xF0;

    if message.len() >= 3 && (msg_type == 0x90 || msg_type == 0x80) {
        let input_note = message[1] as usize;
        let is_note_on = msg_type == 0x90 && message[2] > 0;

        if is_note_on {
            let target_hz = state.tuning[input_note];
            if target_hz <= 0.0 { return; } 

            let mut assigned_chan = None;
            for i in 0..15 {
                let last_offset = if state.last_allocated > 0 { state.last_allocated - 1 } else { 0 };
                let idx = (last_offset + i + 1) % 15;
                let chan = 1 + idx; // Maps 0-14 to Channels 2-16

                if !state.channel_busy[chan as usize] && state.channel_enabled[chan as usize] { 
                    assigned_chan = Some(chan); break; 
                }
            }

            if let Some(chan) = assigned_chan {
                let exact_note = state.synth_ref_note as f32 + 12.0 * (target_hz / state.synth_pitch_center).log2();
                let nearest_note = exact_note.round().clamp(0.0, 127.0) as u8;
                let semitone_diff = exact_note - nearest_note as f32;
                
                let pb_val = calculate_pitch_bend(semitone_diff, state.pitch_bend_range);
                
                state.channel_busy[chan as usize] = true;
                state.note_to_channel[input_note] = Some((chan, nearest_note, 0.0));
                state.last_allocated = chan;
                state.output_flash[chan as usize] = 5; 
                
                if let Some(conn) = &mut state.out_conn {
                    let _ = conn.send(&[msg_type | chan, nearest_note, message[2]]);
                    let _ = conn.send(&[0xE0 | chan, (pb_val & 0x7F) as u8, (pb_val >> 7) as u8]);
                }
            }
        } else if let Some((chan, actual_sent_note, _)) = state.note_to_channel[input_note] {
            state.channel_busy[chan as usize] = false;
            state.note_to_channel[input_note] = None;
            if let Some(conn) = &mut state.out_conn {
                let _ = conn.send(&[msg_type | chan, actual_sent_note, message[2]]);
            }
        }
    } else if status >= 0xF0 {
        if let Some(conn) = &mut state.out_conn { let _ = conn.send(message); }
    } else {
        let mut out_msg = message.to_vec();
        out_msg[0] = msg_type | 0x00; 
        state.output_flash[0] = 5; // Global messages flash Channel 1 dot
        if let Some(conn) = &mut state.out_conn { let _ = conn.send(&out_msg); }
    }
}

fn process_multitimbral_mode(message: &[u8], state: &mut MidiState) {
    let status = message[0];
    let msg_type = status & 0xF0;
    let wheel_range_semitones = 1.0; 

    if message.len() >= 3 && (msg_type == 0x90 || msg_type == 0x80) {
        let input_note = message[1] as usize;
        let is_note_on = msg_type == 0x90 && message[2] > 0;

        if is_note_on {
            let target_hz = state.tuning[input_note];
            if target_hz <= 0.0 { return; } 

            let mut assigned_chan = None;
            for i in 0..16 {
                let chan = (state.last_allocated + i as u8 + 1) % 16;
                if !state.channel_busy[chan as usize] && state.channel_enabled[chan as usize] { 
                    assigned_chan = Some(chan); break; 
                }
            }

            if let Some(chan) = assigned_chan {
                let exact_note = state.synth_ref_note as f32 + 12.0 * (target_hz / state.synth_pitch_center).log2();
                let nearest_note = exact_note.round().clamp(0.0, 127.0) as u8;
                
                let base_semitone_diff = exact_note - nearest_note as f32;
                let input_pb_norm = (state.input_pitch_bend as f32 - 8192.0) / 8192.0; 
                let total_semitone_diff = base_semitone_diff + (input_pb_norm * wheel_range_semitones);
                
                let pb_val = calculate_pitch_bend(total_semitone_diff, state.pitch_bend_range);
                
                state.channel_busy[chan as usize] = true;
                state.note_to_channel[input_note] = Some((chan, nearest_note, base_semitone_diff));
                state.last_allocated = chan;
                state.output_flash[chan as usize] = 5;
                
                if let Some(conn) = &mut state.out_conn {
                    let _ = conn.send(&[0xE0 | chan, (pb_val & 0x7F) as u8, (pb_val >> 7) as u8]);
                    let _ = conn.send(&[msg_type | chan, nearest_note, message[2]]);
                }
            }
        } else if let Some((chan, actual_sent_note, _)) = state.note_to_channel[input_note] {
            state.channel_busy[chan as usize] = false;
            state.note_to_channel[input_note] = None;
            if let Some(conn) = &mut state.out_conn {
                let _ = conn.send(&[msg_type | chan, actual_sent_note, message[2]]);
            }
        }
    } else if msg_type == 0xE0 && message.len() >= 3 {
        let pb_in = (message[1] as u16) | ((message[2] as u16) << 7);
        state.input_pitch_bend = pb_in;
        let wheel_semitone_shift = ((pb_in as f32 - 8192.0) / 8192.0) * wheel_range_semitones;
        
        for voice_state in state.note_to_channel.iter() {
            if let Some((chan, _, base_semitone_diff)) = voice_state {
                let total_semitone_diff = base_semitone_diff + wheel_semitone_shift;
                let pb_val = calculate_pitch_bend(total_semitone_diff, state.pitch_bend_range);
                
                if let Some(conn) = &mut state.out_conn {
                    let _ = conn.send(&[0xE0 | *chan, (pb_val & 0x7F) as u8, (pb_val >> 7) as u8]);
                }
            }
        }
    } else {
        if status >= 0xF0 {
            if let Some(conn) = &mut state.out_conn { let _ = conn.send(message); }
        } else {
            let mut out_msg = message.to_vec();
            for chan in 0..16 {
                if state.channel_enabled[chan] {
                    out_msg[0] = msg_type | chan as u8;
                    state.output_flash[chan] = 5;
                    if let Some(conn) = &mut state.out_conn { let _ = conn.send(&out_msg); }
                }
            }
        }
    }
}