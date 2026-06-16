# Project Summary: Polyphonic Microtonal MIDI Router

## Overview
The goal of this project is to build a high-performance, real-time polyphonic MIDI router in Rust using the `midir` library. It acts as a middleware layer between a standard MIDI controller and a synthesizer. By employing a round-robin voice allocation system and utilizing channel-specific pitch-bend adjustments, the application enables standard polyphonic instruments to play fluidly in microtonal, non-standard, and custom-defined tuning temperaments.

---

## 1. Core Architecture & Voice Allocation

### Thread-Safe State Management
Because `midir` processes incoming MIDI events asynchronously inside a decoupled background callback thread, the core state of the router is wrapped in an atomic reference counter and a mutual exclusion lock: `Arc<Mutex<MidiState>>`. This allows the background processing thread to read and write active voice properties while the main thread simultaneously listens for manual console commands or preset adjustments.

### Round-Robin Voice Distribution
Standard MIDI channels can typically only accept one pitch bend value at a time, meaning a polyphonic chord cannot have unique microtonal tunings per note on a single channel. To solve this, the router maintains an internal pool of accessible MIDI channels (`0` to `num_channels`). 
* **Note-On**: When a key is pressed, the router finds the next available (idle) channel using a round-robin rotation (`(last_allocated + i) % num_channels`). It calculates the precise microtonal frequency target, issues a 14-bit Pitch Bend message to that target channel, and immediately follows it with the Note-On message on that same channel.
* **Note-Off**: The router tracks active notes using an internal lookup array `note_to_channel: [Option<(u8, u8, f32)>; 128]`. Upon receiving a Note-Off, it targets the specific channel assigned to that note, turns off the note, and marks the channel as idle (`false`).

---

## 2. Advanced MIDI Routing & Filtering

To maintain expressive parity with a standard MIDI setup, the router bifurcates traffic into three explicit categories:

1. **Note Events (`0x90` / `0x80`)**: Intercepted and rerouted using the round-robin channel allocation map to achieve microtonal decoupling.
2. **System Messages ($\ge$ `0xF0`)**: Clock pulses, SysEx, active sensing, and start/stop triggers do not contain channel bytes. To prevent stream corruption or data redundancy, these are forwarded exactly once without modification.
3. **Channel-wide Expressions (CC Changes, Mod Wheel, Sustain Pedal)**: Expression controllers (like a sustain pedal `CC 64` or Mod Wheel `CC 1`) affect globally held notes. The router duplicates these incoming commands and broadcasts them simultaneously across *all* allocated channels so that the sonic modification impacts every active voice equally.

---

## 3. Dynamic Hardware Pitch Bend Stacking

A major nuance arises when combining static microtonal retuning with a physical hardware pitch bend wheel. Because the synth channel is *already* using a pitch bend offset just to step into the microtonal frequency, simply passing through raw pitch wheel data would instantly overwrite and break the scale's tuning.

### The Solution: Additive Stacking
The router intercepts physical pitch bend messages (`0xE0`), normalizes them into a float range of `-1.0` to `1.0`, and converts them into an explicit user-intent semitone modifier (e.g., fixed at $\pm1.0$ semitone total range). 

When a note is active, its final pitch bend value is dynamically calculated as:
$$\text{Total Semitones} = \text{Base Tuning Semitone Difference} + (\text{Normalized Wheel Position} \times \text{Wheel Range})$$

This value is translated into an output 14-bit MIDI integer ($0$ to $16383$) relative to the master user-specified **Synthesizer Pitch Bend Range**. If the wheel moves while notes are held down, the system iterates over all active voices and updates their respective channels dynamically on the fly.

---

## 4. Built-in Tuning Presets

The router features a responsive menu (`1`-`9`) allowing instantaneous shifting between several built-in tuning environments:

* **`1` (12-TET)**: Standard Western equal temperament ($2^{1/12}$).
* **`2` (24-EDO / Quarter-Tone)**: Splits the octave into 24 steps ($2^{1/24}$), establishing a strict 50-cents interval step size while keeping MIDI 69 locked exactly to 440Hz.
* **`3` (Just Intonation)**: A pure-ratio tuning scaled relative to C. To keep standard A4 locked to 440Hz, the baseline frequency of C4 is calibrated to $264\text{ Hz}$ (since A's ratio is $5/3$). Octave scaling is handled via unsigned division `(i / 12) as i32 - 5` to circumvent Rust's integer division truncation bug, ensuring perfect doubling across all octaves.
* **`4` through `9` (N-EDO Scales)**: Custom equal divisions of the octave mapping onto **17, 19, 22, 31, 41, and 53 EDO** configurations respectively using the formula:
  $$f(n) = f(69) \times 2^{\frac{n - 69}{N}}$$

---

## 5. Scala (`.scl`) File Parser Architecture

By typing mode **`0`**, the user can feed a local filepath to an external `.scl` file to experience any historical, regional, or experimental scale configuration.

### Parsing Compliance Features:
* Strips leading/trailing quote characters often introduced via command-line file drag-and-drop.
* Ignores lines beginning with `!` as comments.
* Safely processes the scale description line and extracts the expected `num_notes` ceiling.
* Contextually differentiates between **Cents** values (detected by the presence of a decimal point `.`) and **Ratios** (fractions containing a `/` or standalone integers). Cents are converted using $2^{\frac{\text{cents}}{1200.0}}$.
* Automatically injects the implicit $1/1$ base ratio at index $0$.

### Infinite Multi-Octave Tiling via Euclidean Division
The `.scl` file format maps sequentially upward from a reference note (A4 / MIDI 69). The final entry in the parsed scale array acts as the looping formal **Period** (typically an octave, $2/1$). 

To map the finite scale choices indefinitely upward to MIDI note 127 and backward down to MIDI note 0, the program utilizes Euclidean quotient and remainder operations:
```rust
let k = i as i32 - 69; // Step distance from A4
let q = k.div_euclid(n as i32); // What period bracket we are in
let r = k.rem_euclid(n as i32) as usize; // Position within the current scale loop

state.tuning[i] = pitch_center * period.powi(q) * multipliers[r];```
This guarantees that negative indices wrap around seamlessly (e.g., a note just below A4 correctly fetches the highest scale degrees of the previous period block), mapping the tuning seamlessly across the entire keyboard layout.

Future Horizons
- Keyboard Mapping (.kbm) Integration: Designing support to break away from the rigid A4=440Hz default constraint, allowing users to define arbitrary MIDI key roots, reference frequencies, and selective formal intervals mapping across structural scales.
