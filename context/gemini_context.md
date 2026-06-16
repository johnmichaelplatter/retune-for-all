# Polyphonic MIDI Router in Rust

This document provides all the necessary context and code to recreate the lightweight, cross-platform polyphonic MIDI processing tool written in Rust. Currently, the tool handles MIDI input/output device selection, channel routing configuration, and implements a voice allocator to route notes round-robin to free channels while accurately tracking Note On/Off messages.

## Prerequisites

* **Rust:** Ensure you have Rust installed (via [rustup](https://rustup.rs/)).
* **Virtual MIDI Ports:** For testing without external hardware, you will need virtual MIDI ports (e.g., loopMIDI on Windows, or the IAC Driver on macOS).

## Step 1: Project Setup

1. Initialize a new Rust project:
   ```bash
   cargo new poly_midi_router
   cd poly_midi_router
   ```

2. Update your `Cargo.toml` to include the `midir` crate for cross-platform MIDI I/O:
   ```toml
   [package]
   name = "poly_midi_router"
   version = "0.1.0"
   edition = "2021"

   [dependencies]
   midir = "0.9"
   ```

## Step 2: Source Code

Replace the contents of `src/main.rs` with the following code. This includes the `MidiState` tracker, the CLI setup prompts, and the `process_midi` function with voice allocation logic.

```rust
use midir::{MidiInput, MidiInputPort, MidiOutput, MidiOutputConnection, MidiOutputPort};
use std::error::Error;
use std::io::{stdin, stdout, Write};

// State struct to hold our output connection and tracking variables
struct MidiState {
    out_conn: MidiOutputConnection,
    num_channels: u8,
    // Maps a note (0-127) to its active output channel. None if not playing.
    note_to_channel: [Option<u8>; 128], 
    // Tracks whether a specific output channel (voice) is currently holding a note.
    channel_busy: Vec<bool>,            
    // Remembers the last used channel to maintain a round-robin feel among free voices.
    last_allocated: u8,                 
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

    println!("\nConnecting...");
    println!("Input:  {}", in_port_name);
    println!("Output: {}", out_port_name);
    println!("Routing round-robin across {} channels.", num_channels);

    // Open output connection
    let out_conn = midi_out.connect(&out_port, "poly-router-out")?;

    // Initialize our processing state
    let state = MidiState {
        out_conn,
        num_channels,
        note_to_channel: [None; 128],
        channel_busy: vec![false; num_channels as usize],
        last_allocated: 0,
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

/// The core routing and voice allocation logic
fn process_midi(message: &[u8], state: &mut MidiState) {
    if message.is_empty() { return; }

    let status = message[0];
    let msg_type = status & 0xF0; // Mask out the channel to get the message type
    
    let mut out_msg = message.to_vec();

    // 0x90 is Note On, 0x80 is Note Off. 
    // (Note: A Note On with 0 velocity is functionally a Note Off)
    if message.len() >= 3 && (msg_type == 0x90 || msg_type == 0x80) {
        let note = message[1] as usize;
        let velocity = message[2];
        let is_note_on = msg_type == 0x90 && velocity > 0;

        if is_note_on {
            // 1. Search for a free channel, starting just after the last allocated one
            let mut assigned_chan = None;
            for i in 1..=state.num_channels {
                let chan = (state.last_allocated + i) % state.num_channels;
                if !state.channel_busy[chan as usize] {
                    assigned_chan = Some(chan);
                    break;
                }
            }

            // 2. If we found a free channel, route the note
            if let Some(chan) = assigned_chan {
                state.channel_busy[chan as usize] = true;
                state.note_to_channel[note] = Some(chan);
                state.last_allocated = chan;
                
                out_msg[0] = msg_type | chan;
                let _ = state.out_conn.send(&out_msg);
            } else {
                // All voices are currently held! 
                // For now, we simply ignore the new note.
                println!("Polyphony maxed out: Note {} dropped.", note);
            }
        } else {
            // Note Off: Find the channel playing this note and free it
            if let Some(chan) = state.note_to_channel[note] {
                state.channel_busy[chan as usize] = false;
                state.note_to_channel[note] = None;
                
                out_msg[0] = msg_type | chan;
                let _ = state.out_conn.send(&out_msg);
            }
        }
    } else {
        // Non-note messages (Pitch Bend, CCs) pass through unchanged.
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
```

## Step 3: Running and Testing

1. Run the application: `cargo run`
2. Follow the interactive prompts to select your hardware/virtual keyboard as the **Input**.
3. Select a virtual MIDI port or a multi-timbral hardware synthesizer as the **Output**.
4. Set the channels (e.g., `8`).
5. Play notes on your input. Open a MIDI monitor tool connected to the output port to observe the voice allocation in action.