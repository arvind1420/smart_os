/// Voice-First Kernel Interface (NPU Speech-to-Text)
///
/// Phase 20: Native audio input pipeline streaming directly to the AI NPU 
/// for real-time speech recognition.

use crate::serial_println;

pub fn init() {
    serial_println!("[speech] NPU Speech-to-Text pipeline initialized.");
}

/// System-wide Text-to-Speech (TTS) output.
pub fn say(text: &str) {
    // In Phase 29, this will be hooked to the HDA audio driver.
    // For now, we simulate by logging to serial.
    serial_println!("[TTS] Say: {}", text);
}

pub fn handle_audio_stream(_pcm_data: &[i16]) {
    // In a full implementation, this streams PCM data from the HDA driver
    // into the NPU's inference queue for a Whisper-like or simpler STT model.
    // serial_println!("[speech] Processing audio frame...");
}
